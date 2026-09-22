use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;

#[derive(Debug, Clone)]
pub enum DaemonCommand {
    Toggle,
    Quit,
}

pub fn socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(dir).join("hyprly.sock")
    } else {
        PathBuf::from("/tmp/hyprly.sock")
    }
}

pub fn start_socket_listener(sender: Sender<DaemonCommand>) -> Result<std::thread::JoinHandle<()>> {
    let path = socket_path();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    let listener = UnixListener::bind(&path)?;

    let handle = std::thread::spawn(move || {
        for stream in listener.incoming() {
            if let Ok(stream) = stream {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                if reader.read_line(&mut line).is_ok() {
                    let cmd = line.trim();
                    if cmd == "toggle" {
                        let _ = sender.send(DaemonCommand::Toggle);
                    } else if cmd == "quit" {
                        let _ = sender.send(DaemonCommand::Quit);
                    }
                }
            }
        }
    });

    Ok(handle)
}

pub fn send_trigger() -> Result<()> {
    let path = socket_path();
    let mut stream = UnixStream::connect(path)?;
    stream.write_all(b"toggle\n")?;
    Ok(())
}
