<div align="center">
<img src="https://raw.githubusercontent.com/iqbal-rashed/lan-mic-receiver/main/.github/icon.png" alt="LAN Mic Receiver" width="100" height="100">
</div>
<br/>

# LAN Mic Receiver

A real-time audio streaming application that turns your phone into a microphone for your PC/Mac over the local network.

## Features
- **Low Latency**: Uses WebRTC for sub-second delay.
- **High Quality**: 48kHz Opus audio.
- **Secure**: Self-signed HTTPS support for LAN access.
- **Multi-Platform Sender**: 
    - **Web**: Works in any modern mobile browser.
    - **iOS**: Native app available for better performance.

## Components

The project consists of three main parts:

1.  **Receiver (Desktop)**: The host application that runs on your computer.
2.  **Web Sender**: A browser-based microphone (served automatically by the receiver).
3.  **iOS Sender**: A native iOS application for iPhone/iPad.

---

## 1. Receiver (Desktop)
This is the core application that receives audio and plays it through your computer's speakers or a virtual cable.

### Prerequisites
- **Rust**: [Install Rust](https://rustup.rs/).

### Installation & Running
```bash
git clone <repo-url>
cd lan-mic-receiver/receiver
cargo run --release
```

### Usage
- The app will launch and display a **QR Code**.
- It starts a secure HTTPS server (needed for microphone access).
- **Security Warning**: When connecting, you will see a self-signed certificate warning. This is expected for local LAN connections. You must accept it.

---

## 2. Web Sender (Universal)
The easiest way to use LAN Mic. No app installation required.

### Usage
1.  Run the **Receiver** on your computer.
2.  Scan the **QR Code** displayed in the receiver app with your phone's camera.
3.  **Accept the "Not Secure" warning** (Advanced -> Proceed).
4.  Click **Start** and allow microphone permissions.

*Note for Developers: The web source code is in `sender(web)/index.html`. It is embedded into the receiver at compile time.*

---

## 3. iOS Sender (Native App)
A native iOS application for lower latency and better background performance.

### Prerequisites
- **Xcode**: Required to build and install the app.
- **iOS Device**: iPhone or iPad.

### Installation
1.  Open `sender(ios)/Lan Mic.xcodeproj` in Xcode.
2.  Connect your iPhone/iPad via USB.
3.  Select your device as the run target.
4.  Press **Cmd + R** to build and install.
5.  **Trust Developer**: On your iPhone, go to Settings -> General -> VPN & Device Management -> Trust your developer profile.

---

## Troubleshooting

### Microphone Access Denied (Web)
- **Cause**: Browsers block microphone access on insecure (HTTP) connections.
- **Fix**: Ensure you are using the `https://` link (e.g., `https://192.168.1.5:9001`). You MUST accept the browser security warning.

### Cannot Connect
- **Wi-Fi**: Ensure both devices are on the **same Wi-Fi network**.
- **Firewall**: Check if your computer's firewall is blocking port **9001**.
- **Manual IP**: If QR code scanning fails, type the URL manually.

### "Channel Closed" Error (Receiver)
- If the receiver crashes on startup, ensure you are running the latest version from this repo. Fixed by using the `ring` crypto provider.

## Tech Stack
- **Receiver**: Rust, Iced, Axum, WebRTC, Cpal, Rustls.
- **Web Sender**: HTML5, Vanilla JS, TailwindCSS.
- **iOS Sender**: Swift, WebRTC.

## License
MIT
