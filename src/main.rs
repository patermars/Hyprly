mod api;
mod audio;
mod config;
mod context;
mod gui;
mod input;
mod mobile;
mod pipewire;
mod response;
mod screen;
mod text;
mod topic;
mod transcript;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::process::Command;
use std::{env, fs, os::unix::fs::PermissionsExt};
use tokio::runtime::Runtime;

#[derive(Parser)]
#[command(name = "hyprly", about = "GTK4 AI meeting assistant", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(hide = true)]
    Daemon,
    /// Run the private on-screen answer window.
    #[command(hide = true)]
    AnswerOverlay,
    /// Ask Groq to solve the main question visible on the current screen.
    AskScreen,
    /// Print the Hyprly version.
    Version,
    /// Inspect Hyprly configuration.
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    /// Check configuration and desktop dependencies.
    Doctor,
    /// Show PipeWire devices reported by WirePlumber.
    AudioSources,
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Print the configuration file path.
    Path,
    /// Print configuration with API keys redacted.
    Show,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Daemon) => run_daemon()?,
        Some(Commands::AnswerOverlay) => gui::run_answer_overlay(),
        Some(Commands::AskScreen) => {
            let answer = Runtime::new()?.block_on(screen::ask_screen())?;
            println!("{answer}");
        }
        Some(Commands::Version) => println!("hyprly {}", env!("CARGO_PKG_VERSION")),
        Some(Commands::Config { command }) => match command {
            ConfigCommands::Path => {
                println!("{}", config::Config::config_path().display());
                Ok(())
            }
            ConfigCommands::Show => print_config(),
        }?,
        Some(Commands::Doctor) => run_doctor()?,
        Some(Commands::AudioSources) => list_audio_devices()?,
        None => gui::run(),
    }
    Ok(())
}

fn print_config() -> Result<()> {
    let mut config = config::Config::load()?;
    let configured_key_count = config.api.api_keys.len();
    let has_key = !config.api.api_key.is_empty() || configured_key_count > 0;
    config.api.api_key = if has_key {
        "<redacted>".into()
    } else {
        String::new()
    };
    config.api.api_keys = vec!["<redacted>".into(); configured_key_count];
    print!("{}", toml::to_string_pretty(&config)?);
    Ok(())
}

fn run_doctor() -> Result<()> {
    let config_path = config::Config::config_path();
    let config = config::Config::load().context("Could not load Hyprly configuration")?;
    let has_api_key = !config.api.api_keys.is_empty();
    let pipewire_ready = wpctl_status().is_ok_and(|output| output.status.success());
    let grim_available = executable_in_path("grim");

    println!("Configuration: {}", config_path.display());
    print_check("Groq API key", has_api_key, "not configured");
    print_check(
        "PipeWire session",
        pipewire_ready,
        "wpctl could not query PipeWire",
    );
    print_check(
        "grim screenshot tool",
        grim_available,
        "not installed (screen questions need it)",
    );

    if !has_api_key || !pipewire_ready {
        bail!("Hyprly is missing a required setting or audio service; see the checks above.");
    }
    Ok(())
}

fn list_audio_devices() -> Result<()> {
    let output = wpctl_status()
        .context("Could not run wpctl. Install WirePlumber and start the PipeWire user session.")?;
    if !output.status.success() {
        bail!("wpctl could not query the PipeWire session");
    }
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

fn wpctl_status() -> std::io::Result<std::process::Output> {
    Command::new(pipewire::WPCTL)
        .args(pipewire::STATUS_ARGS)
        .output()
}

fn executable_in_path(name: &str) -> bool {
    env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(name))
        .any(|candidate| {
            fs::metadata(candidate).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

fn print_check(label: &str, ok: bool, detail: &str) {
    if ok {
        println!("[OK]   {label}");
    } else {
        println!("[FAIL] {label}: {detail}");
    }
}

fn run_daemon() -> Result<()> {
    // A directly launched daemon opens a tiny test window so the compositor's
    // no_screen_share rule can be checked without starting the full app.
    // The normal GUI sets HYPRLY_BACKEND_MODE when it launches its real backend.
    if env::var_os("HYPRLY_BACKEND_MODE").is_none() {
        gui::run_screen_test_overlay();
        return Ok(());
    }

    let config = config::Config::load()?;
    let context = context::ContextStore::load(&config.context, &config::Config::config_path());
    let mobile_hub = mobile::MobileHub::new(&config.screen.answer_target)?;
    println!("{}", mobile_hub.pairing_info());
    use std::io::Write;
    std::io::stdout().flush()?;
    mobile::start(mobile_hub.clone());
    let runtime = Runtime::new()?;
    let screen_hub = mobile_hub.clone();
    runtime.block_on(async move {
        let mut connection = mobile_hub.subscribe_connection();
        let connection_hub = mobile_hub.clone();
        tokio::spawn(async move {
            use std::io::Write;
            loop {
                println!(
                    "{}",
                    serde_json::json!({"event":"connection", "connected":*connection.borrow(), "local":connection_hub.local_is_connected()})
                );
                let _ = std::io::stdout().flush();
                if connection.changed().await.is_err() {
                    break;
                }
            }
        });
        let mut published_events = mobile_hub.subscribe_events();
        tokio::spawn(async move {
            use std::io::Write;
            while let Ok(event) = published_events.recv().await {
                println!("{event}");
                let _ = std::io::stdout().flush();
            }
        });
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                match event.get("type").and_then(|value| value.as_str()) {
                    Some("screen_started") => {
                        screen_hub.answer_started();
                        screen_hub.status("thinking", "Reading the screen and preparing an answer");
                    }
                    Some("screen_answer") => {
                        if let Some(answer) = event.get("text").and_then(|value| value.as_str()) {
                            screen_hub.set_last_answer(answer);
                            screen_hub.answer_done(answer);
                            screen_hub.status("ready", "Ready");
                        }
                    }
                    Some("screen_error") => {
                        let message = event
                            .get("message")
                            .and_then(|value| value.as_str())
                            .unwrap_or("Screen question failed");
                        screen_hub.status("error", message);
                    }
                    Some("answer_target") => {
                        if let Some(target) = event.get("target").and_then(|value| value.as_str()) {
                            screen_hub.set_answer_target(target);
                        }
                    }
                    Some("pair_local") => {
                        let code = event.get("code").and_then(|value| value.as_str()).unwrap_or("");
                        if screen_hub.pair_local(code) {
                            screen_hub.status("ready", "Floating session connected");
                        } else {
                            screen_hub.status("error", "That pairing code is incorrect");
                        }
                    }
                    Some("pause") => screen_hub.command(mobile::MobileCommand::Pause),
                    Some("resume") => screen_hub.command(mobile::MobileCommand::Resume),
                    Some("clear_transcript") => {
                        screen_hub.command(mobile::MobileCommand::ClearTranscript)
                    }
                    Some("transcribe_recent_audio") => {
                        screen_hub.command(mobile::MobileCommand::TranscribeRecentAudio)
                    }
                    Some("update_context") => {
                        let context = [
                            ("Resume", event.get("resume").and_then(|v| v.as_str()).unwrap_or("")),
                            ("Project descriptions", event.get("projects").and_then(|v| v.as_str()).unwrap_or("")),
                            ("Job description", event.get("job_description").and_then(|v| v.as_str()).unwrap_or("")),
                            ("Company", event.get("company").and_then(|v| v.as_str()).unwrap_or("")),
                        ]
                        .into_iter()
                        .filter(|(_, value)| !value.trim().is_empty())
                        .map(|(label, value)| format!("### {label}\n{}", value.trim()))
                        .collect::<Vec<_>>()
                        .join("\n\n");
                        screen_hub.command(mobile::MobileCommand::UpdateContext(context));
                    }
                    _ => {}
                }
            }
        });
        audio::run(config.audio, config.api, context, mobile_hub).await
    })?;
    Ok(())
}
