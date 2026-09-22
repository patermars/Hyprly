use crate::config::CaptureMode;
use anyhow::{Context, Result};
use std::fs;
use tokio::process::Command;

pub async fn capture_screen(mode: &CaptureMode) -> Result<Vec<u8>> {
    match mode {
        CaptureMode::Fullscreen => {
            let output = Command::new("grim").arg("-").output().await?;
            if !output.status.success() {
                anyhow::bail!("grim failed");
            }
            Ok(output.stdout)
        }
        CaptureMode::ActiveWindow => {
            let output = Command::new("hyprctl")
                .args(["-j", "activewindow"])
                .output()
                .await?;
            let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
            let at = json["at"].as_array().context("missing at")?;
            let size = json["size"].as_array().context("missing size")?;
            let x = at[0].as_i64().unwrap_or(0);
            let y = at[1].as_i64().unwrap_or(0);
            let w = size[0].as_i64().unwrap_or(0);
            let h = size[1].as_i64().unwrap_or(0);
            let geom = format!("{},{} {}x{}", x, y, w, h);
            
            let grim_out = Command::new("grim")
                .args(["-g", &geom, "-"])
                .output()
                .await?;
            Ok(grim_out.stdout)
        }
        CaptureMode::Region => {
            let slurp_out = Command::new("slurp").output().await?;
            let geom = String::from_utf8_lossy(&slurp_out.stdout).trim().to_string();
            let grim_out = Command::new("grim")
                .args(["-g", &geom, "-"])
                .output()
                .await?;
            Ok(grim_out.stdout)
        }
        CaptureMode::ClipboardOnly => Ok(Vec::new()),
    }
}

pub async fn run_ocr(image_data: &[u8]) -> Result<String> {
    if image_data.is_empty() {
        return Ok(String::new());
    }
    let tmp_file = "/tmp/hyprly_capture.png";
    fs::write(tmp_file, image_data)?;
    
    let output = Command::new("tesseract")
        .args([tmp_file, "stdout"])
        .output()
        .await?;
        
    let _ = fs::remove_file(tmp_file);
    
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if text.len() > 2000 {
        text.truncate(2000);
    }
    Ok(text)
}
