mod api;
mod audio;
mod config;
mod context;
mod gui;
mod input;
mod mobile;
mod response;
mod screen;
mod topic;
mod transcript;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tokio::runtime::Runtime;

#[derive(Parser)]
#[command(name = "hyprly", about = "GTK4 AI meeting assistant")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(hide = true)]
    Daemon,
    /// Ask Groq to solve the main question visible on the current screen.
    AskScreen,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Daemon) => run_daemon()?,
        Some(Commands::AskScreen) => {
            let answer = Runtime::new()?.block_on(screen::ask_screen())?;
            println!("{answer}");
        }
        None => gui::run(),
    }
    Ok(())
}

fn run_daemon() -> Result<()> {
    let config = config::Config::load()?;
    let context = context::ContextStore::load(&config.context, &config::Config::config_path())?;
    let mobile_hub = mobile::MobileHub::new()?;
    println!("{}", mobile_hub.pairing_info());
    use std::io::Write;
    std::io::stdout().flush()?;
    mobile::start(mobile_hub.clone());
    let runtime = Runtime::new()?;
    let screen_hub = mobile_hub.clone();
    runtime.block_on(async move {
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
                    _ => {}
                }
            }
        });
        audio::run(config.audio, config.api, context, mobile_hub).await
    })?;
    Ok(())
}
