use crate::{
    api::{groq::GroqClient, types::ChatMessage},
    config::{ApiConfig, AudioConfig},
    context::ContextStore,
    mobile::{MobileCommand, MobileHub},
    response, topic,
    transcript::{self, CleanConfig},
};
use anyhow::Result;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::Instant as StdInstant,
};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    sync::{mpsc, Semaphore},
    time::{sleep, Duration, Instant},
};
use tokio_util::sync::CancellationToken;

static CHUNK_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct RecordedChunk {
    sequence: u64,
    path: PathBuf,
    recorded_at: StdInstant,
    recording_ms: u128,
}

struct CompletedTranscription {
    sequence: u64,
    recorded_at: StdInstant,
    text: Result<Option<crate::api::groq::TranscriptionResult>, String>,
    transcription_ms: u128,
    recording_ms: u128,
}

struct PipelineState {
    raw_transcript: String,
    clean_transcript: String,
    debounce_deadline: Option<Instant>,
    pending_ai: Option<CancellationToken>,
    clean_config: CleanConfig,
}

impl PipelineState {
    fn new(audio_config: &AudioConfig) -> Self {
        Self {
            raw_transcript: String::new(),
            clean_transcript: String::new(),
            debounce_deadline: None,
            pending_ai: None,
            clean_config: CleanConfig {
                remove_fillers: audio_config.filler_filter,
                collapse_repeats: true,
                remove_false_starts: true,
                remove_incomplete_fragments: true,
                custom_fillers: Vec::new(),
            },
        }
    }
}

fn append_transcript(existing: &mut String, incoming: &str) -> bool {
    let incoming_words: Vec<&str> = incoming.split_whitespace().collect();
    if incoming_words.is_empty() {
        return false;
    }

    let existing_words: Vec<&str> = existing.split_whitespace().collect();
    let max_overlap = existing_words.len().min(incoming_words.len());
    let mut overlap = 0;
    for size in (1..=max_overlap).rev() {
        let left = &existing_words[existing_words.len() - size..];
        let right = &incoming_words[..size];
        if left.iter().zip(right).all(|(a, b)| {
            a.trim_matches(|c: char| !c.is_alphanumeric())
                .eq_ignore_ascii_case(b.trim_matches(|c: char| !c.is_alphanumeric()))
        }) {
            overlap = size;
            break;
        }
    }

    if overlap == incoming_words.len() {
        return false;
    }
    let suffix = incoming_words[overlap..].join(" ");
    if !existing.trim().is_empty() {
        existing.push(' ');
    }
    existing.push_str(&suffix);
    true
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

pub async fn run(
    audio: AudioConfig,
    api: ApiConfig,
    context: ContextStore,
    mobile: MobileHub,
) -> Result<()> {
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

    let mut state = PipelineState::new(&audio);
    let mut cmd_rx = mobile.subscribe_commands();
    let shutdown = CancellationToken::new();
    let shutdown_signal = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        shutdown_signal.cancel();
    });

    let (chunk_tx, mut chunk_rx) = mpsc::channel::<RecordedChunk>(4);

    let audio_clone = audio.clone();
    let mobile_clone = mobile.clone();
    let paused = mobile.paused.clone();
    let recorder_shutdown = shutdown.clone();
    let recorder = tokio::spawn(async move {
        loop {
            if recorder_shutdown.is_cancelled() {
                break;
            }
            if paused.load(Ordering::Relaxed) {
                tokio::select! {
                    _ = sleep(Duration::from_millis(200)) => {}
                    _ = recorder_shutdown.cancelled() => break,
                }
                continue;
            }
            mobile_clone.status("recording", "Recording");
            let recorded_at = StdInstant::now();
            match record_chunk(&audio_clone, recorder_shutdown.clone()).await {
                Ok(Some(wav)) => {
                    let chunk = RecordedChunk {
                        sequence: CHUNK_SEQUENCE.load(Ordering::Relaxed).saturating_sub(1),
                        path: wav,
                        recorded_at,
                        recording_ms: recorded_at.elapsed().as_millis(),
                    };
                    if chunk_tx.send(chunk).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("Recording error: {e:#}");
                    mobile_clone.status("error", "Recording failed");
                    sleep(Duration::from_secs(1)).await;
                }
            }
        }
    });

    let (transcription_tx, mut transcription_rx) = mpsc::channel::<CompletedTranscription>(8);
    let transcription_limit =
        std::sync::Arc::new(Semaphore::new(audio.max_concurrent_transcriptions.max(1)));
    let mut transcription_tasks = tokio::task::JoinSet::new();
    let mut pending_transcriptions = BTreeMap::new();
    let mut next_sequence = CHUNK_SEQUENCE.load(Ordering::Relaxed);

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            Some(chunk) = chunk_rx.recv() => {
                let result_tx = transcription_tx.clone();
                let audio_for_task = audio.clone();
                let api_for_task = api.clone();
                let mobile_for_task = mobile.clone();
                let transcription_limit_for_task = transcription_limit.clone();
                transcription_tasks.spawn(async move {
                    let permit = transcription_limit_for_task.acquire_owned().await;
                    let _permit = permit;
                    let started = StdInstant::now();
                    let result = if audio_for_task.vad_enabled
                        && is_silent(&chunk.path, audio_for_task.silence_threshold)
                    {
                        Ok(None)
                    } else {
                        mobile_for_task.status("transcribing", "Transcribing");
                        GroqClient::new(&api_for_task)
                            .transcribe(
                                chunk.path.to_str().unwrap_or_default(),
                                &audio_for_task.transcription_model,
                                &audio_for_task.language,
                            )
                            .await
                            .map(Some)
                            .map_err(|error| format!("{error:#}"))
                    };
                    let _ = tokio::fs::remove_file(&chunk.path).await;
                    let _ = result_tx.send(CompletedTranscription {
                        sequence: chunk.sequence,
                        recorded_at: chunk.recorded_at,
                        text: result,
                        transcription_ms: started.elapsed().as_millis(),
                        recording_ms: chunk.recording_ms,
                    }).await;
                });
            }
            Some(result) = transcription_rx.recv() => {
                pending_transcriptions.insert(result.sequence, result);
                while let Some(result) = pending_transcriptions.remove(&next_sequence) {
                    next_sequence = next_sequence.saturating_add(1);
                eprintln!(
                    "[latency] chunk={} transcription_ms={} end_to_end_before_ai_ms={}",
                    result.sequence,
                    result.transcription_ms,
                    result.recorded_at.elapsed().as_millis()
                );
                eprintln!("[latency] chunk={} recording_ms={}", result.sequence, result.recording_ms);
                let text = match result.text {
                    Ok(Some(result))
                        if !result.text.trim().is_empty()
                            && result.confidence
                                .map(|confidence| confidence >= audio.confidence_threshold)
                                .unwrap_or(true) => result.text.trim().to_string(),
                    Ok(Some(result)) if !result.text.trim().is_empty() => {
                        mobile.status("ready", "Low-confidence transcription skipped");
                        continue;
                    }
                    Ok(_) => continue,
                    Err(error) => {
                        eprintln!("Transcription error: {error}");
                        mobile.status("error", "Transcription failed");
                        continue;
                    }
                };

                let cleaned_text = transcript::clean(&text, &state.clean_config);
                if cleaned_text.trim().is_empty() {
                    continue;
                }
                if topic::is_new_topic(&state.clean_transcript, &cleaned_text) {
                    state.raw_transcript.clear();
                    state.clean_transcript.clear();
                    if let Some(token) = state.pending_ai.take() {
                        token.cancel();
                    }
                    mobile.topic_changed();
                }

                if !append_transcript(&mut state.raw_transcript, &cleaned_text) {
                    continue;
                }
                if state.raw_transcript.len() > 12000 {
                    state.raw_transcript.drain(..state.raw_transcript.len() - 12000);
                }

                state.clean_transcript = state.raw_transcript.clone();
                mobile.transcript(&state.clean_transcript);

                state.debounce_deadline =
                    Some(Instant::now() + Duration::from_millis(audio.debounce_ms));
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Ok(MobileCommand::ClearTranscript) => {
                        state.raw_transcript.clear();
                        state.clean_transcript.clear();
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
                let context_snapshot = context.prompt_section();
                let mobile_ai = mobile.clone();
                let api_clone = api.clone();

                let ai_started = StdInstant::now();
                let handle = tokio::spawn(async move {
                    let messages = vec![
                        ChatMessage {
                            role: "system".to_string(),
                            content: response::system_prompt(),
                        },
                        ChatMessage {
                            role: "user".to_string(),
                            content: format!(
                                "{}\n\nCurrent topic context:\n{}\n\nAnswer the user's latest request using only this current topic. Do not carry context from an earlier topic. Give the most useful concise response now.",
                                context_snapshot,
                                transcript_snapshot,
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
                                    mobile_ai.set_last_answer(&parsed.answer);
                                    let delivery_started = StdInstant::now();
                                    mobile_ai.structured_answer(&parsed.answer, &parsed);
                                    eprintln!("[latency] delivery_ms={}", delivery_started.elapsed().as_millis());
                                    eprintln!("[latency] ai_ms={}", ai_started.elapsed().as_millis());
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
                                            mobile_ai.set_last_answer(&parsed.answer);
                                            let delivery_started = StdInstant::now();
                                            mobile_ai.structured_answer(&parsed.answer, &parsed);
                                            eprintln!("[latency] delivery_ms={} retry=true", delivery_started.elapsed().as_millis());
                                            eprintln!("[latency] ai_ms={} retry=true", ai_started.elapsed().as_millis());
                                            return;
                                        }
                                    }
                                    if !mobile_ai.restore_last_answer() {
                                        mobile_ai.answer_done(&full_answer);
                                    }
                                    eprintln!("[latency] ai_ms={} fallback=true", ai_started.elapsed().as_millis());
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            eprintln!("AI response error: {e:#}");
                            mobile_ai.status("error", "AI failed");
                            mobile_ai.restore_last_answer();
                        }
                        Err(e) => {
                            eprintln!("AI task failed: {e:#}");
                            mobile_ai.status("error", "AI task failed");
                            mobile_ai.restore_last_answer();
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

    shutdown.cancel();
    let _ = recorder.await;
    while let Ok(chunk) = chunk_rx.try_recv() {
        let _ = tokio::fs::remove_file(chunk.path).await;
    }
    drop(transcription_tx);
    while !transcription_tasks.is_empty() {
        tokio::select! {
            Some(_) = transcription_rx.recv() => {}
            joined = transcription_tasks.join_next() => {
                if joined.is_none() {
                    break;
                }
            }
        }
    }
    if let Some(token) = state.pending_ai.take() {
        token.cancel();
    }
    mobile.status("ready", "Stopped");
    Ok(())
}

async fn record_chunk(
    config: &AudioConfig,
    shutdown: CancellationToken,
) -> Result<Option<PathBuf>> {
    let sequence = CHUNK_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "hyprly-meeting-{}-{}.wav",
        std::process::id(),
        sequence
    ));
    let mut command = Command::new("pw-record");
    command.args([
        "--format",
        "s16",
        "--rate",
        "16000",
        "--channels",
        "1",
        "--raw",
        "-",
    ]);
    let source = if config.source.trim().is_empty() {
        default_output_source().await.unwrap_or_default()
    } else {
        config.source.trim().to_string()
    };
    if !source.is_empty() {
        command.args(["--target", &source]);
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("pw-record did not provide audio output"))?;

    const FRAME_SAMPLES: usize = 1_600; // 100 ms at 16 kHz.
    let frame_bytes = FRAME_SAMPLES * std::mem::size_of::<i16>();
    let preferred_samples = config.chunk_seconds.max(1) as usize * 16_000;
    let min_samples = config.min_utterance_seconds.max(1) as usize * 16_000;
    let max_samples = config
        .max_utterance_seconds
        .max(config.min_utterance_seconds)
        .max(1) as usize
        * 16_000;
    let endpoint_silence_samples = (config.endpoint_silence_ms.max(100) as usize * 16_000) / 1_000;
    let mut pcm = Vec::with_capacity(preferred_samples * std::mem::size_of::<i16>());
    let mut frame = vec![0_u8; frame_bytes];
    let mut speech_seen = false;
    let mut silent_samples = 0;
    let mut stop_requested = false;

    let mut cancelled = false;
    while pcm.len() / std::mem::size_of::<i16>() < max_samples {
        match tokio::select! {
            _ = shutdown.cancelled() => {
                cancelled = true;
                break;
            }
            result = stdout.read_exact(&mut frame) => result,
        } {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                anyhow::bail!("failed reading audio from pw-record: {error}");
            }
        }

        let samples = frame
            .chunks_exact(2)
            .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32_768.0);
        let samples: Vec<f32> = samples.collect();
        let rms = (samples.iter().map(|sample| sample * sample).sum::<f32>()
            / samples.len() as f32)
            .sqrt();
        pcm.extend_from_slice(&frame);

        if rms >= config.silence_threshold {
            speech_seen = true;
            silent_samples = 0;
        } else if speech_seen {
            silent_samples += samples.len();
            if pcm.len() / std::mem::size_of::<i16>() >= min_samples
                && silent_samples >= endpoint_silence_samples
            {
                stop_requested = true;
                break;
            }
        }
    }

    if pcm.len() / std::mem::size_of::<i16>() >= max_samples {
        stop_requested = true;
    }

    let _ = child.kill().await;
    let status = child.wait().await?;
    if cancelled {
        return Ok(None);
    }
    if pcm.is_empty() {
        let _ = tokio::fs::remove_file(&path).await;
        anyhow::bail!("pw-record produced no audio: {status}");
    }

    if !status.success() && !stop_requested {
        let _ = tokio::fs::remove_file(&path).await;
        anyhow::bail!("pw-record stopped unexpectedly: {status}");
    }

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let write_result = (|| -> Result<()> {
        let mut writer = hound::WavWriter::create(&path, spec)?;
        for bytes in pcm.chunks_exact(2) {
            writer.write_sample(i16::from_le_bytes([bytes[0], bytes[1]]))?;
        }
        writer.finalize()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(error);
    }

    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::append_transcript;

    #[test]
    fn drops_duplicate_and_overlapping_chunks() {
        let mut transcript = "We should review the numbers".to_string();
        assert!(!append_transcript(&mut transcript, "review the numbers"));
        assert!(append_transcript(
            &mut transcript,
            "the numbers again tomorrow"
        ));
        assert_eq!(transcript, "We should review the numbers again tomorrow");
    }
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
