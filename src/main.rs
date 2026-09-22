mod api;
mod capture;
mod config;
mod context;
mod daemon;
mod ui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use gtk4 as gtk;
use gtk4::glib;
use gtk::prelude::*;
use libadwaita as adw;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

#[derive(Parser)]
#[command(name = "hyprly", about = "AI overlay for Hyprland")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Daemon,
    Trigger,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Trigger => {
            daemon::send_trigger()?;
        }
        Commands::Daemon => {
            run_daemon()?;
        }
    }
    Ok(())
}

fn run_daemon() -> Result<()> {
    let cfg = config::Config::load()?;
    let config = Arc::new(cfg);

    let rt = Runtime::new()?;
    let runtime = Arc::new(rt);

    let app = adw::Application::builder()
        .application_id("dev.hyprly.app")
        .build();

    let config_clone = config.clone();
    let runtime_clone = runtime.clone();

    app.connect_activate(move |app| {
        let (window, components) = ui::window::setup_overlay_window(app, &config_clone.ui);

        let (cmd_sender, cmd_receiver) = std::sync::mpsc::channel::<daemon::DaemonCommand>();

        let _ = daemon::start_socket_listener(cmd_sender);

        let input = components.input.clone();
        let response_view = components.response_view.clone();
        let status_label = components.status_label.clone();
        let spinner = components.spinner.clone();

        let window_toggle = window.clone();
        let input_toggle = input.clone();
        let app_quit = app.clone();

        glib::timeout_add_local(Duration::from_millis(50), move || {
            while let Ok(cmd) = cmd_receiver.try_recv() {
                match cmd {
                    daemon::DaemonCommand::Toggle => {
                        if window_toggle.is_visible() {
                            ui::window::hide_overlay(&window_toggle);
                        } else {
                            ui::window::show_overlay(&window_toggle);
                            input_toggle.grab_focus();
                        }
                    }
                    daemon::DaemonCommand::Quit => {
                        app_quit.quit();
                    }
                }
            }
            glib::ControlFlow::Continue
        });

        let cfg_entry = config_clone.clone();
        let rt_entry = runtime_clone.clone();
        let buf = response_view.buffer();
        let status = status_label.clone();
        let spin = spinner.clone();

        input.connect_activate(move |entry| {
            let text = entry.text().to_string();
            if text.is_empty() {
                return;
            }
            entry.set_text("");

            status.set_text("Thinking...");
            spin.start();
            buf.set_text("");

            let cfg_async = cfg_entry.clone();
            let buf_clone = buf.clone();
            let status_clone = status.clone();
            let spin_clone = spin.clone();
            let rt_clone = rt_entry.clone();

            glib::MainContext::default().spawn_local(async move {
                let text_clone = text.clone();
                let cfg_cap = cfg_async.capture.clone();
                
                let (tx, rx) = tokio::sync::oneshot::channel();
                rt_clone.spawn(async move {
                    let res = context::gather_context(&cfg_cap, &text_clone).await;
                    let _ = tx.send(res);
                });

                let ctx_res = rx.await.expect("tokio task panicked");
                let sys_msg = match ctx_res {
                    Ok(ctx) => ctx.format_for_api(),
                    Err(e) => {
                        spin_clone.stop();
                        status_clone.set_text("Error");
                        buf_clone.set_text(&e.to_string());
                        return;
                    }
                };

                let messages = vec![
                    api::types::ChatMessage {
                        role: "system".to_string(),
                        content: sys_msg,
                    },
                    api::types::ChatMessage {
                        role: "user".to_string(),
                        content: text,
                    },
                ];

                let client = api::groq::GroqClient::new(&cfg_async.api);
                let (token_tx, mut token_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                let (done_tx, done_rx) = tokio::sync::oneshot::channel();
                
                rt_clone.spawn(async move {
                    let res = client.stream_chat(messages, token_tx).await;
                    let _ = done_tx.send(res);
                });

                let mut full_text = String::new();
                while let Some(token) = token_rx.recv().await {
                    full_text.push_str(&token);
                    let buf_c = buf_clone.clone();
                    let markup = ui::markdown::markdown_to_pango(&full_text);
                    glib::idle_add_local_once(move || {
                        buf_c.set_text("");
                        let mut iter = buf_c.start_iter();
                        buf_c.insert_markup(&mut iter, &markup);
                    });
                }

                match done_rx.await.expect("tokio task panicked") {
                    Ok(_) => {
                        spin_clone.stop();
                        status_clone.set_text("Done");
                    }
                    Err(e) => {
                        spin_clone.stop();
                        status_clone.set_text("Error");
                        let mut iter = buf_clone.end_iter();
                        buf_clone.insert(&mut iter, &format!("\nError: {}", e.to_string()));
                    }
                }
            });
        });
    });

    app.run_with_args::<String>(&[]);
    Ok(())
}
