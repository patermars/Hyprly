<div align="center">

<br />

<img src="assets/logo_text.png" alt="Hyprly" width="420">

<br />

```
██╗  ██╗██╗   ██╗██████╗ ██████╗ ██╗  ██╗   ██╗
██║  ██║╚██╗ ██╔╝██╔══██╗██╔══██╗██║  ╚██╗ ██╔╝
███████║ ╚████╔╝ ██████╔╝██████╔╝██║   ╚████╔╝ 
██╔══██║  ╚██╔╝  ██╔═══╝ ██╔══██╗██║    ╚██╔╝  
██║  ██║   ██║   ██║     ██║  ██║███████╗██║   
╚═╝  ╚═╝   ╚═╝   ╚═╝     ╚═╝  ╚═╝╚══════╝╚═╝   
```

**Open-source Cluely for Linux.**  
Your computer listens. Your phone shows the answers. Nobody sees a thing.

<br />

[![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![PipeWire](https://img.shields.io/badge/PipeWire-0A0A0A?style=for-the-badge&logo=linux&logoColor=white)](https://pipewire.org/)
[![Groq](https://img.shields.io/badge/Groq-F55036?style=for-the-badge)](https://console.groq.com/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue?style=for-the-badge)](LICENSE)

<br />

</div>

---

Tools like Cluely are powerful — but closed, expensive, and macOS-only. Hyprly is the Linux answer. It's a single Rust binary, free to use, and built on open infrastructure. The phone UI is compiled right into the binary — no frontend server, no Node.js, no deploy step. Open a URL on your phone and you're live.

Hyprly runs silently in the background during your meetings. It captures your system audio through PipeWire, transcribes it with Groq Whisper, cleans up the transcript, and streams a structured AI response to your phone over WebSocket — in near real-time.

No screen overlays. No desktop clutter. Your display stays completely clean. You glance at your phone.

<br />

## Clones over time

![Hyprly clone history](assets/repo-growth-clones.svg)

<br />

## How it works

![Hyprly workflow](assets/workflow.png)

<br />

## Features

<table>
<tr>
<td width="50%">

**🎙 Near real-time audio**  
Records in 2-second chunks. A built-in VAD skips silent frames. Speech endpoint detection cuts chunks early when silence follows speech — so you don't wait for the full window to expire.

**🧠 Groq Whisper transcription**  
Uses `whisper-large-v3-turbo` for fast, accurate speech-to-text. Temporary WAV files are deleted immediately after transcription — nothing persists on disk.

**🧹 Smart transcript cleanup**  
Before the AI sees anything, the transcript is scrubbed: fillers (`uh`, `um`, `er`, `like`, `hmm`), consecutive repeats, and false starts (`no wait`, `i mean`, `sorry`, `actually`) are removed. The model gets clean, dense speech.

**📱 Embedded mobile UI**  
The HTML, CSS, and JS are compiled into the Rust binary with `include_str!`. Open the local setup page at `http://<your-ip>:8765` to install Hyprly’s local CA certificate, then use the HTTPS app at `https://<your-ip>:8766` — no app install.

</td>
<td width="50%">

**⚡ Structured AI responses**  
Every answer is a validated JSON object with a `type`, a `confidence` score, optional `bullets`, and an optional syntax-highlighted `code` block. Not a blob of text — a structured response the UI renders properly.

**📡 Streamed to your phone**  
Tokens arrive over WebSocket as they're generated. The phone renders the `answer` field incrementally, word-by-word. Markdown and code blocks are rendered in real-time as the stream fills them in.

**🔢 Six-digit pairing**  
A code is printed at startup. Enter it on your phone once. After that, the phone reconnects automatically on drop — with exponential back-off so it never hammers the server.

**🎛 Full phone control**  
Pause and resume recording. Clear the transcript. Watch the live pipeline state (`Recording → Transcribing → Thinking → Ready`) — all from your phone without touching the computer.

</td>
</tr>
</table>

<br />

## Quick Start

**Requirements:** Linux with GTK4, PipeWire + WirePlumber, Rust, a Groq API key, and a phone on the same network.

```bash
git clone https://github.com/patermars/hyprly.git
cd hyprly

# Build and launch the GTK4 app
cargo run --release
```

On first launch, enter your Groq API key and use **Test Groq API** to check it. Press **Save key** to store it in `~/.config/hyprly/config.toml` with private file permissions, then press **Connect** to start Hyprly. **Test PipeWire audio** records a short sample to verify capture; another button checks optional PDF and DOCX tools. The app starts automatically on later launches when a key is configured.

The main window displays the pairing code, setup page, secure pairing URL, certificate fingerprint, and QR code. On first use, open the setup page on your phone and install Hyprly’s local CA certificate. Verify it with the fingerprint shown in the app, then open the secure URL or scan the QR code. Closing the window leaves Hyprly in the system tray; the tray menu can connect, disconnect, reopen the window, or exit.

The local setup page includes certificate installation guidance. The HTTPS page’s camera button can scan the QR code shown in the desktop app.

<br />

## Configuration

Config lives at `~/.config/hyprly/config.toml`. All fields have sensible defaults — the only required value is your Groq API key.

```toml
[api]
model       = "openai/gpt-oss-20b"
max_tokens  = 1024
# api_key is read from GROQ_API_KEY env var if left blank here

[audio]
enabled               = true
source                = ""       # empty = default PipeWire output sink
chunk_seconds         = 2
min_utterance_seconds = 2
max_utterance_seconds = 10
transcription_model   = "whisper-large-v3-turbo"
language              = "en"
endpoint_silence_ms   = 900      # finalize after this much trailing silence
debounce_ms           = 300      # grace period after an utterance endpoint
```

### Interview context

You can paste your resume, job description, and company notes into the **Interview context** section of the phone webpage after pairing, or attach PDF and DOCX files to have their text extracted into those fields. Press **Save context** to use them for new answers. The page keeps these fields in that browser's local storage for the next visit. They are not added to the live transcript or topic history. PDF uploads require `pdftotext` from Poppler; DOCX uploads require `unzip`. Uploaded files are removed after extraction.

The context is sent to the configured AI provider when generating answers, so avoid entering secrets that should not leave the machine.

**Capture a specific audio device** (e.g. a virtual meeting cable, browser tab, or headset mic):

```bash
# List all PipeWire nodes
wpctl status -n

# Find the node ID for your target source or sink, then:
```

```toml
[audio]
source = "54"   # node ID from wpctl status
```

<br />

## Project Structure

```
hyprly/
├── src/
│   ├── main.rs          — GTK4 app launcher and hidden daemon entry point
│   ├── gui.rs           — Desktop controls, checks, pairing QR, and tray actions
│   ├── audio.rs         — PipeWire recording, VAD, speech detection, AI pipeline
│   ├── transcript.rs    — Filler removal, repeat collapse, false-start stripping
│   ├── response.rs      — AiResponse types, JSON parser, retry logic, system prompt
│   ├── config.rs        — TOML config loading with GROQ_API_KEY env var fallback
│   ├── mobile.rs        — HTTP setup page, HTTPS app server, pairing, broadcast hub
│   └── api/
│       ├── groq.rs      — Groq Whisper transcription + streaming chat client
│       └── types.rs     — Serde types for Groq API requests and responses
├── mobile/
    ├── index.html       — Phone UI shell
    ├── styles.css       — Mobile-first dark theme
    ├── app.js           — WebSocket client, streaming Markdown renderer, syntax highlighter
│   └── manifest.webmanifest
└── data/
    └── hyprly.desktop   — Application launcher entry
```

<br />

## Architecture notes

**State machine in a Tokio task**

The audio loop runs in a dedicated async task. `PipelineState` holds the rolling raw transcript (capped at 12 KB to bound memory), the cleaned transcript, a hash of the last chunk for deduplication, and a debounce deadline. When a sentence boundary (`.`, `?`, `!`) is detected, the AI call fires immediately. Otherwise it waits a configurable debounce window. If new speech arrives while an AI call is in flight, it's cancelled and replaced with a fresh call.

**Zero-copy mobile serving**

`include_str!` embeds the entire mobile frontend at compile time. The Axum server serves HTML, CSS, JS, and the web manifest directly from process memory. No static files are needed at runtime — the binary is fully self-contained.

**Structured response with retry**

The system prompt instructs the LLM to return only a JSON object. If the first response fails to parse, Hyprly sends a second request with the malformed text and asks the model to fix it. If the retry also fails, the raw text is displayed as a plain fallback.

**Streaming token display**

Tokens flow through an unbounded `mpsc` channel from the Groq stream to the WebSocket handler. Each token is broadcast as an `answer_token` event. The phone's JavaScript reads the `answer` field out of the partial JSON string incrementally — it appears word-by-word as it's typed, with Markdown rendered live.

<br />

<br />

## Requirements

| Requirement | Notes |
|---|---|
| Linux | Any modern distro |
| GTK4 | Desktop control app |
| PipeWire + WirePlumber | For audio capture |
| `pw-record` | Ships with PipeWire |
| `wpctl` | Ships with WirePlumber |
| `pdftotext` | Poppler utility; needed to extract PDF uploads |
| `unzip` | Needed to extract DOCX uploads |
| Rust + Cargo | Install from [rustup.rs](https://rustup.rs/) |
| [Groq API key](https://console.groq.com/keys) | Free tier is enough |
| Phone on the same Wi-Fi | Any browser, no app install |
| StatusNotifier tray host | Optional; for example, Waybar with its tray module enabled |

<br />

## Contributing

Hyprly is early but functional. PRs are welcome for anything in the roadmap, bug fixes, or improvements you think are valuable. Please open an issue first for large changes so we can align on approach before you build.

<br />

## License

[MIT](LICENSE) — © 2026 Aryan Kumar Sinha
