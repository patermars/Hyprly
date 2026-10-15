<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/logo_text_white.png">
  <img src="assets/logo_text_white.png" alt="Hyprly" width="360">
</picture>

### Private meeting audio, with answers on your phone.

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![PipeWire](https://img.shields.io/badge/Audio-PipeWire-5b8.svg)](https://pipewire.org/)

</div>

Hyprly is a Linux desktop app that captures meeting audio through PipeWire, transcribes it with Groq Whisper, and streams AI answers to a phone browser. Audio is processed in memory and is not saved to disk.

<p align="center">
  <img src="assets/hyprly-suite.png" alt="Hyprly desktop app showing API setup, system checks, and phone pairing" width="100%">
</p>

## How it works

![Hyprly audio and response workflow](assets/workflow.png)

## Features

- GTK4 desktop app
- PipeWire audio capture
- Groq Whisper transcription
- Streaming AI answers
- QR code and six-digit phone pairing
- Local HTTPS connection
- Midnight, Paper, and Aurora themes
- PDF and DOCX interview context
- System tray controls
- No audio saved to disk

## Requirements

- Linux, GTK4, PipeWire, and WirePlumber
- Rust and Cargo to build from source
- A [Groq API key](https://console.groq.com/keys)
- A phone browser on the same local network

For an Ubuntu or Debian build environment, install `libgtk-4-dev`, `libssl-dev`, and `pkg-config`. On Arch Linux, install `gtk4`, `openssl`, and `pkgconf`.

## Build and run

```bash
git clone https://github.com/patermars/Hyprly.git
cd Hyprly
cargo run --release
```

This runs Hyprly from the project directory; it does not install it system-wide. Prebuilt packages and release archives, when available, are listed on [GitHub Releases](https://github.com/patermars/Hyprly/releases).

## Pair your phone

1. Enter your Groq API key in the desktop app, then select **Save key** and **Connect**.
2. On your phone, scan the QR code or open the setup URL shown in the app.
3. Follow the setup page to install Hyprly’s local CA certificate. Check its fingerprint against the one shown on the desktop.
4. Open the secure pairing link and enter the six-digit code if prompted.

The desktop app also provides PipeWire and optional PDF/DOCX tool checks. Closing the window keeps Hyprly available from the system tray.

## Clone history

![Hyprly clone history](assets/repo-growth-clones.svg)

## License

[MIT](LICENSE) — © 2026 Aryan Kumar Sinha
