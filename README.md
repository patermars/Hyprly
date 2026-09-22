<div align="center">
  <h1>Hyprly</h1>
  <p><strong>A lightning-fast, context-aware AI overlay designed specifically for Hyprland & Wayland.</strong></p>
</div>

---

Hyprly is a native Rust Wayland application that acts as an intelligent overlay. Powered by the **Groq API** and **GTK4 Layer Shell**, it instantly slides over your screen to answer questions, read your clipboard, and see your active windows with near-zero latency.

No X11 dependencies. No bloated Electron. Just raw Rust and native Wayland.

## Features
- **Wayland Native**: Built from the ground up for Hyprland using `gtk4-layer-shell`.
- **Context-Aware**: Automatically reads your active window title and your clipboard data.
- **Lightning Fast**: Uses Groq's high-speed inference for instant token streaming.
- **Smart UI Layering**: Sits cleanly on top of your windows without stealing focus until you click.
- **Markdown Support**: Renders code blocks, bold text, and formatting on-the-fly.

## Requirements
Ensure you have the following Wayland/Arch utilities installed:
- `grim` (Screen capture)
- `slurp` (Region selection)
- `tesseract` & `tesseract-data-eng` (OCR for reading the screen)
- `wl-clipboard` (Clipboard access)

```bash
sudo pacman -S grim slurp tesseract tesseract-data-eng wl-clipboard
```

## Installation & Setup

1. **Clone the repository:**
```bash
git clone https://github.com/yourusername/Hyprly.git
cd Hyprly
```

2. **Set your Groq API Key:**
```bash
export GROQ_API_KEY="gsk_your_api_key_here"
```

3. **Start the daemon (runs in the background):**
```bash
cargo run --release -- daemon
```

## Usage

Hyprly operates using a lightweight background socket daemon. To trigger the UI instantly, simply run:
```bash
cargo run --release -- trigger
```

**Pro Tip:** Bind this to a shortcut in your `hyprland.conf` for the best experience!
```ini
# Start the daemon on login
exec-once = GROQ_API_KEY="your_key" /path/to/hyprly daemon

# Trigger the overlay with Super + Shift + Space
bind = SUPER SHIFT, Space, exec, /path/to/hyprly trigger
```

## Configuration
Hyprly looks for a config file at `~/.config/hyprly/config.toml`. It supports customizing UI width, position, opacity, API settings, and more. 

---
<div align="center">
  <sub>Built with Rust & GTK4 </sub>
</div>
