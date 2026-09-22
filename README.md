# Hyprly

Hyprly is a headless AI meeting assistant for Linux. It captures meeting audio through PipeWire, transcribes it with Groq Whisper, generates concise assistance, and streams the transcript and answers to a paired mobile web app.

The project has no graphical interface on the computer; all assistant output is delivered to the phone.

## Requirements

- Rust and Cargo
- PipeWire and WirePlumber
- `pw-record` and `wpctl`
- A Groq API key
- A phone on the same local network

## Run from the checkout

```bash
cd /home/patermars/Documents/Hyprly
GROQ_API_KEY="your_groq_key" cargo run -- daemon
```

The daemon prints a pairing code and serves the mobile app on port `8765`:

```text
http://YOUR_COMPUTER_LAN_IP:8765
```

Open that address on your phone and enter the pairing code. Keep the daemon terminal running.

## Configuration

Copy the example config:

```bash
mkdir -p ~/.config/hyprly
cp config.example.toml ~/.config/hyprly/config.toml
```

Audio is enabled in the example config. An empty `audio.source` automatically selects the current default PipeWire output, which is appropriate for Meet/system audio. To select a specific node, inspect:

```bash
wpctl status -n
```

Then set, for example:

```toml
[audio]
enabled = true
source = "62"
chunk_seconds = 8
transcription_model = "whisper-large-v3-turbo"
```

## Project layout

- `src/audio.rs` — PipeWire recording and transcription loop
- `src/api/` — Groq transcription and chat clients
- `src/mobile.rs` — local HTTP server, pairing, and WebSocket events
- `mobile/` — phone interface

The mobile app is served directly by the Rust daemon; no separate frontend build is required.
