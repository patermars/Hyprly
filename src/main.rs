mod api;
mod audio;
mod config;
mod mobile;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tokio::runtime::Runtime;

#[derive(Parser)]
#[command(name = "hyprly", about = "Headless AI meeting assistant")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Daemon,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Daemon => run_daemon()?,
    }
    Ok(())
}

fn run_daemon() -> Result<()> {
    let config = config::Config::load()?;
    let mobile_hub = mobile::MobileHub::new();
    mobile::start(mobile_hub.clone());
    let runtime = Runtime::new()?;
    runtime.block_on(audio::run(config.audio, config.api, mobile_hub))?;
    Ok(())
}
