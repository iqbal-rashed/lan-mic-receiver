# LAN Mic Receiver (WebRTC) — Windows (Rust)

This project is a **LAN-only WebRTC audio receiver** with a desktop UI (egui/eframe).  
It accepts a WebRTC peer connection from a sender (e.g., an iPhone app) over a **built-in WebSocket signaling server**,
decodes the incoming **Opus** audio, and plays it to a selected **Windows output device**.

> Want to use it as a "microphone" in Discord/Zoom/etc?
> WebRTC gives you low-latency transport, but it **does not create a Windows microphone device**.
> For that, you still need a *virtual audio cable* (e.g., VB-Cable) or an audio driver.

---

## How it works (high-level)

- UI starts a local signaling server: `ws://<PC-IP>:9001/ws`
- Sender connects and sends:
  - SDP offer
  - ICE candidates (trickle)
- Receiver replies with:
  - SDP answer
  - ICE candidates
- When the remote audio track arrives, we:
  - read RTP packets (`TrackRemote::read_rtp`)
  - Opus-decode to PCM
  - push PCM into a lock-free queue
  - CPAL outputs audio to your selected device

---

## Build (Windows)

### Requirements
- Rust (stable): https://www.rust-lang.org/tools/install
- Visual Studio Build Tools (C++), or full Visual Studio (for native deps)
- CMake (required by the `opus` bindings if it can’t find a system opus)

### Build & Run
```powershell
git clone <your repo>
cd lan-mic-webrtc-receiver
cargo run --release
```

---

## Using VB-Cable (optional, but needed to appear as "mic")

1. Install VB-Cable (VB-Audio Virtual Cable)
2. In this app, choose output device: **"CABLE Input (VB-Audio Virtual Cable)"**
3. In Discord/Zoom/etc, choose microphone input: **"CABLE Output (VB-Audio Virtual Cable)"**

---

## Signaling message format (JSON)

Over the WebSocket (`/ws`) the app accepts and sends JSON messages:

### SDP (offer/answer)
```json
{ "type": "sdp", "data": { "type": "offer", "sdp": "v=0..." } }
```

### ICE candidate
```json
{
  "type": "ice",
  "data": {
    "candidate": "candidate:...",
    "sdp_mid": "0",
    "sdp_mline_index": 0,
    "username_fragment": null
  }
}
```

---

## Notes / Limitations

- WebRTC always uses a small **jitter buffer** internally. You can keep latency low, but not truly “zero buffering”.
- This receiver does **not** do echo cancellation (AEC). For a one-way mic stream, AEC is usually unnecessary.
- LAN-only is easiest with **host candidates** (no STUN). There's a UI toggle to enable a public STUN server if you want.

---

## Project layout

- `src/main.rs` — app entry
- `src/app.rs` — UI
- `src/core/*` — runtime thread, signaling server, webrtc session
- `src/audio/*` — CPAL output + PCM queue

---

## License
MIT OR Apache-2.0
