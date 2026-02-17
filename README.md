# LAN Mic

<p align="center">
  <img src=".github/icon.png" alt="LAN Mic Icon" width="128" height="128" />
</p>

<p align="center">
  <strong>Use your iPhone as a wireless microphone for your PC — over LAN via WebRTC.</strong>
</p>

<p align="center">
  <a href="https://github.com/iqbal-rashed/lan-mic-receiver/actions/workflows/ci.yml">
    <img src="https://github.com/iqbal-rashed/lan-mic-receiver/actions/workflows/ci.yml/badge.svg" alt="CI" />
  </a>
</p>

---

## How It Works

```
┌──────────────┐    WebRTC (audio)    ┌──────────────────┐
│  iOS Sender  │ ◄══════════════════► │  Desktop Receiver │
│  (iPhone)    │    WebSocket (SDP)   │  (Rust / iced)    │
└──────────────┘                      └──────────────────┘
```

1. **Receiver** runs on your PC and listens for connections
2. **Sender** (iOS) captures microphone audio and streams it via WebRTC
3. Audio plays through the selected output device on your PC (speakers, virtual cable, etc.)

## Components

| Component | Language | Location |
|-----------|----------|----------|
| **Receiver** — Desktop app with GUI | Rust | [`receiver/`](receiver/) |
| **Sender** — iOS app | Swift | [`sender/`](sender/) |

## Quick Start

### Receiver (macOS / Windows / Linux)

```bash
# Prerequisites: Rust toolchain (https://rustup.rs)
cd receiver
cargo run --release
```

The receiver window will show the WebSocket URL (e.g. `ws://192.168.1.100:9001/ws`).

### Sender (iOS)

1. Open `sender/Lan Mic.xcodeproj` in Xcode
2. Build and run on your iPhone
3. Enter the receiver's IP address and port
4. Tap **Connect**

### Virtual Audio Cable (Optional)

To route microphone audio into other apps (Discord, OBS, etc.), use a virtual audio device:

| Platform | Tool |
|----------|------|
| macOS | [VB-Cable](https://vb-audio.com/Cable/) or [BlackHole](https://github.com/ExistentialAudio/BlackHole) |
| Windows | [VB-Cable](https://vb-audio.com/Cable/) |
| Linux | PulseAudio null sink |

Select the virtual cable as the output device in the receiver, then set it as the input in your target app.

## Tech Stack

- **WebRTC** for real-time, low-latency audio transport
- **Opus** codec for high-quality audio compression
- **WebSocket** for SDP/ICE signaling
- **iced** for the receiver's cross-platform GUI
- **cpal** for audio output

## Building

### Receiver

```bash
cd receiver
cargo build --release
```

**Linux dependencies:**
```bash
sudo apt-get install cmake pkg-config libasound2-dev libfontconfig1-dev libwayland-dev libxkbcommon-dev
```

### Sender

Open in Xcode and build for a physical device (microphone requires real hardware).

## License

MIT
