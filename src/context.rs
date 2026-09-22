use crate::capture;
use crate::config::{CaptureConfig, CaptureMode};
use anyhow::Result;
use serde::Serialize;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct ScreenContext {
    pub active_window_title: String,
    pub active_window_class: String,
    pub screen_text: String,
    pub clipboard: String,
    pub user_prompt: String,
}

async fn get_active_window() -> Result<(String, String)> {
    let output = Command::new("hyprctl")
        .args(["-j", "activewindow"])
        .output()
        .await?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let title = json["title"].as_str().unwrap_or("").to_string();
    let class = json["class"].as_str().unwrap_or("").to_string();
    Ok((title, class))
}

async fn get_clipboard() -> Result<String> {
    let output = Command::new("wl-paste")
        .args(["--no-newline"])
        .output()
        .await?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub async fn gather_context(config: &CaptureConfig, user_prompt: &str) -> Result<ScreenContext> {
    let (active_window_title, active_window_class) = match config.mode {
        CaptureMode::ClipboardOnly => (String::new(), String::new()),
        _ => get_active_window().await.unwrap_or_default(),
    };

    let clipboard = if config.include_clipboard {
        get_clipboard().await.unwrap_or_default()
    } else {
        String::new()
    };

    let screen_text = if config.ocr && !matches!(config.mode, CaptureMode::ClipboardOnly) {
        let img = capture::capture_screen(&config.mode).await?;
        capture::run_ocr(&img).await.unwrap_or_default()
    } else {
        String::new()
    };

    Ok(ScreenContext {
        active_window_title,
        active_window_class,
        screen_text,
        clipboard,
        user_prompt: user_prompt.to_string(),
    })
}

impl ScreenContext {
    pub fn format_for_api(&self) -> String {
        format!(
            "Active Window: {} ({})\nScreen Text:\n{}\nClipboard:\n{}\nUser Prompt: {}",
            self.active_window_title,
            self.active_window_class,
            self.screen_text,
            self.clipboard,
            self.user_prompt
        )
    }
}
