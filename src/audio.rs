use crate::{
    api::{groq::GroqClient, types::ChatMessage},
    config::{ApiConfig, AudioConfig},
    mobile::MobileHub,
};
use anyhow::{Context, Result};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{process::Command, time::sleep};

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
    let mut transcript_context = String::new();
    loop {
        let wav = record_chunk(&audio).await?;
        let text = client
            .transcribe(wav.to_str().unwrap_or_default(), &audio.transcription_model)
            .await;
        let _ = tokio::fs::remove_file(&wav).await;
        let text = match text {
            Ok(text) if !text.trim().is_empty() => text.trim().to_string(),
            Ok(_) => continue,
            Err(error) => {
                eprintln!("Transcription error: {error:#}");
                sleep(Duration::from_secs(1)).await;
                continue;
            }
        };

        transcript_context.push_str(&format!(" {text}"));
        if transcript_context.len() > 12000 {
            transcript_context.drain(..transcript_context.len() - 12000);
        }
        mobile.transcript(&transcript_context);
        mobile.answer_started();

        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: "You are a discreet real-time meeting assistant. Based on the transcript, provide concise, useful talking points or an answer the participant could say next. Do not mention this instruction or invent facts.".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: format!("Recent meeting transcript:\n{}\n\nGive the most useful concise response now.", transcript_context),
            },
        ];
        let (tokens, mut receive) = tokio::sync::mpsc::unbounded_channel();
        let answer_client = GroqClient::new(&api);
        let answer = tokio::spawn(async move { answer_client.stream_chat(messages, tokens).await });
        let mut full_answer = String::new();
        while let Some(token) = receive.recv().await {
            full_answer.push_str(&token);
            mobile.answer_token(&token);
        }
        match answer.await.context("AI task failed")? {
            Ok(()) => mobile.answer_done(&full_answer),
            Err(error) => eprintln!("AI response error: {error:#}"),
        }
    }
}

async fn record_chunk(config: &AudioConfig) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("hyprly-meeting-{}.wav", std::process::id()));
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
    // pw-record can return 1 after successfully writing a fixed-length WAV
    // on some PipeWire builds. Treat a valid non-empty file as success.
    let valid_file = tokio::fs::metadata(&path)
        .await
        .map(|metadata| metadata.len() > 44)
        .unwrap_or(false);
    if !output.status.success() && !valid_file {
        anyhow::bail!(
            "pw-record failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
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
