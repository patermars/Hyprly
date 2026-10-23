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

<div align="center">
  <h1>⚠️ Hyprland floating overlay warning ⚠️</h1>
</div>

Hyprly's floating overlay can stay visible on your desktop while remaining hidden from screen shares with the Hyprland patch enabled. This behavior is confirmed on **Hyprland 0.56.2**, commit `efb50993780079460b0cbed1363e2166a2de1d9f`.

The Arch installer, [`install-arch.sh`](install-arch.sh), can apply [Hyprland PR #16061](https://github.com/hyprwm/Hyprland/commit/eb4edf1cd20ae02cae8c83b819cd8e59100572c). It patches a local Hyprland source checkout; you must build, install, and restart Hyprland separately. Future Hyprland updates may break the behavior or prevent the patch from applying. Rerun `./install-arch.sh` and select the patch option to try applying it again. If that does not work, ask your AI agent to adapt the patch for your Hyprland version using:

## How it works

![Hyprly audio and response workflow](assets/workflow.png)

![Hyprly floating overlay visible on the desktop while hidden from the screen share](assets/no_screen_share.gif)

## Features

- GTK4 desktop app
- PipeWire audio capture
- Groq Whisper transcription
- Streaming AI answers
- QR code and six-digit phone pairing
- Local HTTPS connection
- Midnight, Paper, and Aurora themes
- PDF and DOCX interview context
- On-demand screen reading with Groq vision
- System tray controls
- No audio saved to disk

## Requirements

- Linux with GTK4, PipeWire, WirePlumber, and `grim`
- Rust and Cargo to build from source (Cargo is included with Rust)
- A [Groq API key](https://console.groq.com/keys)
- A phone browser on the same local network

Install the dependencies and build tools with your distribution's package manager:

**Arch Linux**

```bash
sudo pacman -S --needed base-devel git gtk4 pipewire wireplumber grim rust openssl pkgconf
```

**Debian / Ubuntu**

```bash
sudo apt update
sudo apt install build-essential git libgtk-4-dev libssl-dev pkg-config pipewire wireplumber grim rustc cargo
```

## Build and run


```bash
git clone https://github.com/patermars/Hyprly.git
cd Hyprly
```
To build and temporarily run Hyprly from the project directory:
```bash
cargo run --release
```

`cargo run --release` does not install Hyprly. 

To install it as a native Arch package, run this from the project root:

```bash
./install-arch.sh
```

## Clone history

![Hyprly clone history](assets/repo-growth-clones.svg)

## License

[MIT](LICENSE) — © 2026 Aryan Kumar Sinha
