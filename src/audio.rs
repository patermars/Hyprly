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
    collections::{BTreeMap, VecDeque},
    io::Cursor,
    process::Stdio,
    sync::atomic::Ordering,
    time::Instant as StdInstant,
};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    sync::{mpsc, Semaphore},
    time::{sleep, Duration, Instant},
};
use tokio_util::sync::CancellationToken;

pub(crate) const AUDIO_SAMPLE_RATE: u32 = 16_000;
pub(crate) const AUDIO_CHANNELS: u16 = 1;
pub(crate) const AUDIO_PCM_FORMAT: &str = "s16";
pub(crate) const AUDIO_FRAME_DURATION_MS: u32 = 100;
const MILLISECONDS_PER_SECOND: u32 = 1_000;
pub(crate) const AUDIO_FRAME_SAMPLES: usize =
    (AUDIO_SAMPLE_RATE * AUDIO_FRAME_DURATION_MS / MILLISECONDS_PER_SECOND) as usize;
const MAX_TRANSCRIPT_CHARS: usize = 12_000;
const MIN_ENDPOINT_SILENCE_MS: u64 = 100;

struct RecordedChunk {
    sequence: u64,
    wav: Vec<u8>,
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
    transcript: String,
    debounce_deadline: Option<Instant>,
    pending_ai: Option<CancellationToken>,
    clean_config: CleanConfig,
}

impl PipelineState {
    fn new(audio_config: &AudioConfig) -> Self {
        Self {
            transcript: String::new(),
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

fn is_silent(wav: &[u8], threshold: f32) -> bool {
    let reader = match hound::WavReader::new(Cursor::new(wav)) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let spec = reader.spec();
    if spec.sample_format == hound::SampleFormat::Int && spec.bits_per_sample == 16 {
        root_mean_square(
            reader
                .into_samples::<i16>()
                .filter_map(|sample| sample.ok())
                .map(|sample| sample as f32 / 32_768.0),
        )
        .map(|rms| rms < threshold)
        .unwrap_or(true)
    } else {
        false
    }
}

fn root_mean_square(samples: impl Iterator<Item = f32>) -> Option<f32> {
    let (sum_squares, count) = samples.fold((0.0, 0), |(sum, count), sample| {
        (sum + sample * sample, count + 1)
    });
    (count > 0).then(|| (sum_squares / count as f32).sqrt())
}

fn encode_wav(pcm: &[u8]) -> Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: AUDIO_CHANNELS,
        sample_rate: AUDIO_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut output = Cursor::new(Vec::with_capacity(pcm.len() + 44));
    {
        let mut writer = hound::WavWriter::new(&mut output, spec)?;
        for sample in pcm.as_chunks::<2>().0 {
            writer.write_sample(i16::from_le_bytes(*sample))?;
        }
        writer.finalize()?;
    }
    Ok(output.into_inner())
}

pub async fn run(
    audio: AudioConfig,
    api: ApiConfig,
    mut context: ContextStore,
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

    let groq = std::sync::Arc::new(GroqClient::new(&api));
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
    let mut connection_state = mobile.subscribe_connection();
    let mut connection = connection_state.clone();
    let paused = mobile.paused.clone();
    let recorder_shutdown = shutdown.clone();
    let recorder = tokio::spawn(async move {
        let mut sequence = 0_u64;
        loop {
            if recorder_shutdown.is_cancelled() {
                break;
            }
            if !*connection.borrow() {
                mobile_clone.status("ready", "Waiting for phone pairing");
                tokio::select! {
                    changed = connection.changed() => {
                        if changed.is_err() { break; }
                    }
                    _ = recorder_shutdown.cancelled() => break,
                }
                continue;
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
            match record_chunk(&audio_clone, recorder_shutdown.clone(), connection.clone()).await {
                Ok(Some(wav)) => {
                    let chunk = RecordedChunk {
                        sequence,
                        wav,
                        recorded_at,
                        recording_ms: recorded_at.elapsed().as_millis(),
                    };
                    sequence = sequence.saturating_add(1);
                    if chunk_tx.send(chunk).await.is_err() {
                        break;
                    }
                }
                Ok(None) if recorder_shutdown.is_cancelled() => break,
                Ok(None) => continue,
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
    let mut next_sequence = 0;
    let mut recent_audio = VecDeque::<u8>::with_capacity(AUDIO_SAMPLE_RATE as usize * 20);
    let recent_audio_limit = AUDIO_SAMPLE_RATE as usize * 10 * std::mem::size_of::<i16>();

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            Some(chunk) = chunk_rx.recv() => {
                if chunk.wav.len() > 44 {
                    recent_audio.extend(&chunk.wav[44..]);
                    if recent_audio.len() > recent_audio_limit {
                        let excess = recent_audio.len() - recent_audio_limit;
                        recent_audio.drain(..excess);
                    }
                }
                let result_tx = transcription_tx.clone();
                let audio_for_task = audio.clone();
                let groq_for_task = groq.clone();
                let mobile_for_task = mobile.clone();
                let connection_for_task = mobile.subscribe_connection();
                let transcription_limit_for_task = transcription_limit.clone();
                transcription_tasks.spawn(async move {
                    let permit = transcription_limit_for_task.acquire_owned().await;
                    let _permit = permit;
                    let started = StdInstant::now();
                    let mut connection_for_task = connection_for_task;
                    let result = if !*connection_for_task.borrow()
                        || (audio_for_task.vad_enabled
                            && is_silent(&chunk.wav, audio_for_task.silence_threshold))
                    {
                        Ok(None)
                    } else {
                        mobile_for_task.status("transcribing", "Transcribing");
                        tokio::select! {
                            biased;
                            _ = connection_for_task.changed() => Ok(None),
                            result = groq_for_task.transcribe(
                                &chunk.wav,
                                &audio_for_task.transcription_model,
                                &audio_for_task.language,
                            ) => result.map(Some).map_err(|error| format!("{error:#}")),
                        }
                    };
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
                if !*connection_state.borrow() {
                    continue;
                }
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
                if topic::is_new_topic(&state.transcript, &cleaned_text) {
                    state.transcript.clear();
                    if let Some(token) = state.pending_ai.take() {
                        token.cancel();
                    }
                    mobile.topic_changed();
                }

                if !append_transcript(&mut state.transcript, &cleaned_text) {
                    continue;
                }
                if state.transcript.len() > MAX_TRANSCRIPT_CHARS {
                    state
                        .transcript
                        .drain(..state.transcript.len() - MAX_TRANSCRIPT_CHARS);
                }

                mobile.transcript(&state.transcript);

                state.debounce_deadline =
                    Some(Instant::now() + Duration::from_millis(audio.debounce_ms));
                }
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Ok(MobileCommand::ClearTranscript) => {
                        state.transcript.clear();
                        mobile.transcript("");
                    }
                    Ok(MobileCommand::TranscribeRecentAudio) => {
                        if recent_audio.is_empty() {
                            mobile.status("ready", "No recent audio available yet");
                            continue;
                        }
                        mobile.status("transcribing", "Transcribing the last 10 seconds");
                        let wav = encode_wav(&recent_audio.iter().copied().collect::<Vec<_>>())?;
                        match groq
                            .transcribe(&wav, &audio.transcription_model, &audio.language)
                            .await
                        {
                            Ok(result) if !result.text.trim().is_empty() => {
                                if let Some(confidence) = result.confidence {
                                    if confidence < audio.confidence_threshold {
                                        mobile.status("ready", "Low-confidence transcription skipped");
                                        continue;
                                    }
                                }
                                let cleaned =
                                    transcript::clean(result.text.trim(), &state.clean_config);
                                if !cleaned.trim().is_empty() {
                                    if topic::is_new_topic(&state.transcript, &cleaned) {
                                        state.transcript.clear();
                                        if let Some(token) = state.pending_ai.take() {
                                            token.cancel();
                                        }
                                        mobile.topic_changed();
                                    }
                                    append_transcript(&mut state.transcript, &cleaned);
                                    mobile.transcript(&state.transcript);
                                    state.debounce_deadline = Some(Instant::now());
                                } else {
                                    mobile.status("ready", "No speech found in recent audio");
                                }
                            }
                            Ok(_) => mobile.status("ready", "No speech found in recent audio"),
                            Err(error) => {
                                eprintln!("Recent audio transcription error: {error:#}");
                                mobile.status("error", "Recent audio transcription failed");
                            }
                        }
                    }
                    Ok(MobileCommand::UpdateContext(value)) => {
                        context.set_web_context(value);
                        mobile.status("ready", "Interview context saved");
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
            changed = connection_state.changed() => {
                if changed.is_err() {
                    break;
                }
                if !*connection_state.borrow() {
                    state.transcript.clear();
                    state.debounce_deadline = None;
                    if let Some(token) = state.pending_ai.take() {
                        token.cancel();
                    }
                    mobile.transcript("");
                    mobile.status("ready", "Waiting for phone pairing");
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

                if !*connection_state.borrow() || state.transcript.trim().is_empty() {
                    continue;
                }

                if let Some(token) = state.pending_ai.take() {
                    token.cancel();
                }

                let cancel = CancellationToken::new();
                state.pending_ai = Some(cancel.clone());

                mobile.status("thinking", "Thinking");
                mobile.answer_started();

                let transcript_snapshot = state.transcript.clone();
                let context_snapshot = context.prompt_section();
                let mobile_ai = mobile.clone();
                let groq_for_ai = groq.clone();
                let mut ai_connection = mobile.subscribe_connection();

                let ai_started = StdInstant::now();
                let handle = tokio::spawn(async move {
                    if !*ai_connection.borrow() {
                        return;
                    }
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
                        let stream_client = groq_for_ai.clone();
                        async move { stream_client.stream_chat(messages, token_tx).await }
                    });

                    let mut full_answer = String::new();
                    loop {
                        tokio::select! {
                            _ = cancel.cancelled() => {
                                stream_task.abort();
                                return;
                            }
                            changed = ai_connection.changed() => {
                                if changed.is_err() || !*ai_connection.borrow() {
                                    stream_task.abort();
                                    return;
                                }
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

                    if cancel.is_cancelled() || !*ai_connection.borrow() {
                        return;
                    }
                    match stream_task.await {
                        Ok(Ok(())) => {
                            match response::parse(&full_answer) {
                                Ok(parsed) => {
                                    let delivery_started = StdInstant::now();
                                    mobile_ai.structured_answer(&parsed);
                                    eprintln!("[latency] delivery_ms={}", delivery_started.elapsed().as_millis());
                                    eprintln!("[latency] ai_ms={}", ai_started.elapsed().as_millis());
                                }
                                Err(_) => {
                                    let retry_msgs = response::retry_messages(&full_answer);
                                    let retry_client = groq_for_ai.clone();
                                    let (retry_tx, mut retry_rx) = mpsc::unbounded_channel();
                                    let retry_task = tokio::spawn(async move {
                                        retry_client.stream_chat(retry_msgs, retry_tx).await
                                    });
                                    let mut retry_answer = String::new();
                                    loop {
                                        tokio::select! {
                                            _ = cancel.cancelled() => {
                                                retry_task.abort();
                                                return;
                                            }
                                            changed = ai_connection.changed() => {
                                                if changed.is_err() || !*ai_connection.borrow() {
                                                    retry_task.abort();
                                                    return;
                                                }
                                            }
                                            token = retry_rx.recv() => match token {
                                                Some(token) => retry_answer.push_str(&token),
                                                None => break,
                                            }
                                        }
                                    }
                                    if let Ok(Ok(())) = retry_task.await {
                                        if cancel.is_cancelled() || !*ai_connection.borrow() {
                                            return;
                                        }
                                        if let Ok(parsed) = response::parse(&retry_answer) {
                                            let delivery_started = StdInstant::now();
                                            mobile_ai.structured_answer(&parsed);
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
    mut connection: tokio::sync::watch::Receiver<bool>,
) -> Result<Option<Vec<u8>>> {
    let mut command = Command::new("pw-record");
    command
        .arg("--format")
        .arg(AUDIO_PCM_FORMAT)
        .arg("--rate")
        .arg(AUDIO_SAMPLE_RATE.to_string())
        .arg("--channels")
        .arg(AUDIO_CHANNELS.to_string())
        .arg("--raw")
        .arg("-");
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

    let sample_rate = AUDIO_SAMPLE_RATE as usize;
    let frame_bytes = AUDIO_FRAME_SAMPLES * std::mem::size_of::<i16>();
    let preferred_samples = config.chunk_seconds.max(1) as usize * sample_rate;
    let min_samples = config.min_utterance_seconds.max(1) as usize * sample_rate;
    let max_samples = config
        .max_utterance_seconds
        .max(config.min_utterance_seconds)
        .max(1) as usize
        * sample_rate;
    let endpoint_silence_samples =
        (config.endpoint_silence_ms.max(MIN_ENDPOINT_SILENCE_MS) as usize * sample_rate)
            / MILLISECONDS_PER_SECOND as usize;
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
            changed = connection.changed() => {
                if changed.is_err() || !*connection.borrow() {
                    cancelled = true;
                    break;
                }
                continue;
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

        let samples = frame.as_chunks::<2>().0;
        let rms = root_mean_square(
            samples
                .iter()
                .map(|bytes| i16::from_le_bytes(*bytes) as f32 / 32_768.0),
        )
        .unwrap_or(0.0);
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
        anyhow::bail!("pw-record produced no audio: {status}");
    }

    if !status.success() && !stop_requested {
        anyhow::bail!("pw-record stopped unexpectedly: {status}");
    }

    Ok(Some(encode_wav(&pcm)?))
}

#[cfg(test)]
mod tests {
    use super::{append_transcript, encode_wav, AUDIO_CHANNELS, AUDIO_SAMPLE_RATE};
    use std::io::Cursor;

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

    #[test]
    fn encodes_pcm_as_a_mono_16khz_wav_in_memory() {
        let samples = [0_i16, 1_000, -1_000];
        let pcm: Vec<u8> = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        let wav = encode_wav(&pcm).unwrap();
        let reader = hound::WavReader::new(Cursor::new(wav)).unwrap();

        assert_eq!(reader.spec().channels, AUDIO_CHANNELS);
        assert_eq!(reader.spec().sample_rate, AUDIO_SAMPLE_RATE);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(
            reader
                .into_samples::<i16>()
                .collect::<Result<Vec<_>, _>>()
                .unwrap(),
            samples
        );
    }
}

async fn default_output_source() -> Option<String> {
    let output = Command::new(crate::pipewire::WPCTL)
        .args(crate::pipewire::STATUS_ARGS)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    crate::pipewire::default_sink(&String::from_utf8_lossy(&output.stdout))
}
