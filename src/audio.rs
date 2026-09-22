use crate::{
    api::{groq::GroqClient, types::ChatMessage},
    config::{ApiConfig, AudioConfig},
    mobile::{MobileCommand, MobileHub},
    response,
    transcript::{self, CleanConfig},
};
use anyhow::Result;
use std::{
    path::PathBuf,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::{process::Command, sync::mpsc, time::{sleep, Duration, Instant}};
use tokio_util::sync::CancellationToken;

static CHUNK_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct PipelineState {
    raw_transcript: String,
    clean_transcript: String,
    last_answer: Option<String>,
    last_chunk_hash: u64,
    debounce_deadline: Option<Instant>,
    pending_ai: Option<CancellationToken>,
    clean_config: CleanConfig,
}

impl PipelineState {
    fn new(audio_config: &AudioConfig) -> Self {
        Self {
            raw_transcript: String::new(),
            clean_transcript: String::new(),
            last_answer: None,
            last_chunk_hash: 0,
            debounce_deadline: None,
            pending_ai: None,
            clean_config: CleanConfig {
                remove_fillers: audio_config.filler_filter,
                collapse_repeats: true,
                remove_false_starts: true,
                custom_fillers: Vec::new(),
            },
        }
    }
}

fn simple_hash(s: &str) -> u64 {
    let mut h: u64 = 0;
    for b in s.bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as u64);
    }
    h
}

fn is_duplicate(new_text: &str, last_hash: u64) -> bool {
    if new_text.trim().is_empty() {
        return true;
    }
    simple_hash(new_text.trim()) == last_hash
}

fn has_sentence_boundary(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.ends_with('.') || trimmed.ends_with('?') || trimmed.ends_with('!')
}

fn is_silent(path: &std::path::Path, threshold: f32) -> bool {
    let reader = match hound::WavReader::open(path) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let spec = reader.spec();
    if spec.sample_format == hound::SampleFormat::Int && spec.bits_per_sample == 16 {
        let samples: Vec<f32> = reader
            .into_samples::<i16>()
            .filter_map(|s| s.ok())
            .map(|s| s as f32 / 32768.0)
            .collect();
        if samples.is_empty() {
            return true;
        }
        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
        rms < threshold
    } else {
        false
    }
}

pub async fn run(audio: AudioConfig, api: ApiConfig, mobile: MobileHub) -> Result<()> {
    if !audio.enabled {
        eprintln!(
            "Audio capture disabled. Set [audio].enabled = true in ~/.config/hyprly/config.toml"
        );
        std::future::pending::<()>().await;
        return Ok(());
    }
    if api.api_key.is_empty() {
        anyhow::bail!("GROQ_API_KEY is required when audio capture is enabled");
    }

    let client = GroqClient::new(&api);
    let mut state = PipelineState::new(&audio);
    let mut cmd_rx = mobile.subscribe_commands();

    let (chunk_tx, mut chunk_rx) = mpsc::channel::<PathBuf>(4);

    let audio_clone = audio.clone();
    let mobile_clone = mobile.clone();
    let paused = mobile.paused.clone();
    tokio::spawn(async move {
        loop {
            if paused.load(Ordering::Relaxed) {
                sleep(Duration::from_millis(200)).await;
                continue;
            }
            mobile_clone.status("recording", "Recording");
            match record_chunk(&audio_clone).await {
                Ok(wav) => {
                    if chunk_tx.send(wav).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("Recording error: {e:#}");
                    mobile_clone.status("error", "Recording failed");
                    sleep(Duration::from_secs(1)).await;
                }
            }
        }
    });

    loop {
        tokio::select! {
            Some(wav) = chunk_rx.recv() => {
                if audio.vad_enabled && is_silent(&wav, audio.silence_threshold) {
                    let _ = tokio::fs::remove_file(&wav).await;
                    continue;
                }

                mobile.status("transcribing", "Transcribing");
                let text = client
                    .transcribe(wav.to_str().unwrap_or_default(), &audio.transcription_model)
                    .await;
                let _ = tokio::fs::remove_file(&wav).await;

                let text = match text {
                    Ok(text) if !text.trim().is_empty() => text.trim().to_string(),
                    Ok(_) => continue,
                    Err(error) => {
                        eprintln!("Transcription error: {error:#}");
                        mobile.status("error", "Transcription failed");
                        continue;
                    }
                };

                if is_duplicate(&text, state.last_chunk_hash) {
                    continue;
                }
                state.last_chunk_hash = simple_hash(text.trim());

                state.raw_transcript.push_str(&format!(" {text}"));
                if state.raw_transcript.len() > 12000 {
                    state.raw_transcript.drain(..state.raw_transcript.len() - 12000);
                }

                state.clean_transcript = transcript::clean(&state.raw_transcript, &state.clean_config);
                mobile.transcript(&state.clean_transcript);

                if has_sentence_boundary(&text) {
                    state.debounce_deadline = Some(Instant::now());
                } else {
                    state.debounce_deadline = Some(Instant::now() + Duration::from_millis(audio.debounce_ms));
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Ok(MobileCommand::ClearTranscript) => {
                        state.raw_transcript.clear();
                        state.clean_transcript.clear();
                        state.last_chunk_hash = 0;
                        mobile.transcript("");
                    }
                    Ok(MobileCommand::Pause) => {
                        mobile.status("ready", "Paused");
                    }
                    Ok(MobileCommand::Resume) => {
                        mobile.status("ready", "Resumed");
                    }
                    Err(_) => {}
                }
            }
            _ = async {
                if let Some(deadline) = state.debounce_deadline {
                    sleep(deadline.saturating_duration_since(Instant::now())).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {
                state.debounce_deadline = None;

                if state.clean_transcript.trim().is_empty() {
                    continue;
                }

                if let Some(token) = state.pending_ai.take() {
                    token.cancel();
                }

                let cancel = CancellationToken::new();
                state.pending_ai = Some(cancel.clone());

                mobile.status("thinking", "Thinking");
                mobile.answer_started();

                let transcript_snapshot = state.clean_transcript.clone();
                let mobile_ai = mobile.clone();
                let api_clone = api.clone();

                let handle = tokio::spawn(async move {
                    let messages = vec![
                        ChatMessage {
                            role: "system".to_string(),
                            content: response::system_prompt(),
                        },
                        ChatMessage {
                            role: "user".to_string(),
                            content: format!(
                                "Recent meeting transcript:\n{}\n\nGive the most useful concise response now.",
                                transcript_snapshot
                            ),
                        },
                    ];

                    let (token_tx, mut token_rx) = mpsc::unbounded_channel();
                    let stream_task = tokio::spawn({
                        let ai_client2 = GroqClient::new(&api_clone);
                        async move { ai_client2.stream_chat(messages, token_tx).await }
                    });

                    let mut full_answer = String::new();
                    loop {
                        tokio::select! {
                            _ = cancel.cancelled() => {
                                stream_task.abort();
                                return;
                            }
                            token = token_rx.recv() => {
                                match token {
                                    Some(t) => {
                                        full_answer.push_str(&t);
                                        mobile_ai.answer_token(&t);
                                    }
                                    None => break,
                                }
                            }
                        }
                    }

                    match stream_task.await {
                        Ok(Ok(())) => {
                            match response::parse(&full_answer) {
                                Ok(parsed) => {
                                    mobile_ai.structured_answer(&parsed.answer, &parsed);
                                }
                                Err(_) => {
                                    let retry_msgs = response::retry_messages(&full_answer);
                                    let retry_client = GroqClient::new(&api_clone);
                                    let (retry_tx, mut retry_rx) = mpsc::unbounded_channel();
                                    let retry_task = tokio::spawn(async move {
                                        retry_client.stream_chat(retry_msgs, retry_tx).await
                                    });
                                    let mut retry_answer = String::new();
                                    while let Some(t) = retry_rx.recv().await {
                                        retry_answer.push_str(&t);
                                    }
                                    if let Ok(Ok(())) = retry_task.await {
                                        if let Ok(parsed) = response::parse(&retry_answer) {
                                            mobile_ai.structured_answer(&parsed.answer, &parsed);
                                            return;
                                        }
                                    }
                                    mobile_ai.answer_done(&full_answer);
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            eprintln!("AI response error: {e:#}");
                            mobile_ai.status("error", "AI failed");
                        }
                        Err(e) => {
                            eprintln!("AI task failed: {e:#}");
                            mobile_ai.status("error", "AI task failed");
                        }
                    }
                });

                let mobile_done = mobile.clone();
                tokio::spawn(async move {
                    let _ = handle.await;
                    mobile_done.status("ready", "Ready");
                });
            }
        }
    }
}

async fn record_chunk(config: &AudioConfig) -> Result<PathBuf> {
    let sequence = CHUNK_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "hyprly-meeting-{}-{}.wav",
        std::process::id(),
        sequence
    ));
    let sample_count = config.chunk_seconds.max(1) * 16_000;
    let mut command = Command::new("pw-record");
    command.args([
        "--format",
        "s16",
        "--rate",
        "16000",
        "--channels",
        "1",
        "--container",
        "wav",
        "--sample-count",
        &sample_count.to_string(),
    ]);
    let source = if config.source.trim().is_empty() {
        default_output_source().await.unwrap_or_default()
    } else {
        config.source.trim().to_string()
    };
    if !source.is_empty() {
        command.args(["--target", &source]);
    }
    let output = command
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await?;
    let valid_file = tokio::fs::metadata(&path)
        .await
        .map(|metadata| metadata.len() > 44)
        .unwrap_or(false);
    if !output.status.success() && !valid_file {
        let _ = tokio::fs::remove_file(&path).await;
        anyhow::bail!(
            "pw-record failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    if !valid_file {
        let _ = tokio::fs::remove_file(&path).await;
        anyhow::bail!("pw-record produced an empty or invalid WAV file");
    }
    Ok(path)
}

async fn default_output_source() -> Option<String> {
    let output = Command::new("wpctl")
        .args(["status", "-n"])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut in_sinks = false;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.contains("├─ Sinks:") {
            in_sinks = true;
            continue;
        }
        if line.contains("├─ Sources:") {
            break;
        }
        if in_sinks && line.contains('*') {
            let after_star = line.split_once('*')?.1.trim_start();
            let node_id = after_star.split_once('.')?.0.trim();
            if node_id.parse::<u32>().is_ok() {
                return Some(node_id.to_string());
            }
        }
    }
    None
}
