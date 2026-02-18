use crate::core::SharedStatus;
use anyhow::{anyhow, Result};
use axum::extract::ws::{Message, WebSocket};
use crossbeam_queue::ArrayQueue;
use opus::{Channels, Decoder as OpusDecoder};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::{RTCIceCandidate, RTCIceCandidateInit};
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp_transceiver::rtp_codec::RTPCodecType;
use webrtc::rtp_transceiver::rtp_transceiver_direction::RTCRtpTransceiverDirection;
use webrtc::rtp_transceiver::RTCRtpTransceiverInit;

// ---------------------------------------------------------------------------
// Signaling message format — matches the iOS sender's flat JSON schema:
//   SDP:  {"type":"offer"|"answer", "sdp":"v=0..."}
//   ICE:  {"type":"ice", "candidate":"...", "sdpMid":"0", "sdpMLineIndex":0}
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct SignalMessage {
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sdp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "sdpMid")]
    sdp_mid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "sdpMLineIndex")]
    sdp_mline_index: Option<i32>,
}

/// Maximum outbound signaling messages before backpressure.
const SIGNAL_CHANNEL_SIZE: usize = 64;

pub async fn run(
    mut socket: WebSocket,
    queue: Arc<ArrayQueue<i16>>,
    use_stun: bool,
    shared: SharedStatus,
) -> Result<()> {
    let (out_tx, mut out_rx) = mpsc::channel::<SignalMessage>(SIGNAL_CHANNEL_SIZE);

    let pc =
        create_peer_connection(use_stun, shared.clone(), queue.clone(), out_tx.clone()).await?;
    shared.set_pc_state(Some("created".into()));

    // --- Create SDP offer and send to sender ---
    let offer = pc.create_offer(None).await?;
    pc.set_local_description(offer).await?;

    if let Some(local_desc) = pc.local_description().await {
        shared.log_line(format!(
            "Created SDP offer ({:?}), sending to sender",
            local_desc.sdp_type
        ));
        let msg = SignalMessage {
            msg_type: "offer".to_string(),
            sdp: Some(local_desc.sdp),
            candidate: None,
            sdp_mid: None,
            sdp_mline_index: None,
        };
        let txt = serde_json::to_string(&msg)?;
        socket
            .send(Message::Text(txt))
            .await
            .map_err(|e| anyhow!("Failed to send offer over WebSocket: {e}"))?;
    }

    // Pending ICE candidates that arrive before remote description is set
    let pending_ice: Arc<tokio::sync::Mutex<Vec<RTCIceCandidateInit>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));

    loop {
        tokio::select! {
            // Inbound WebSocket messages
            msg = socket.recv() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(_)) | None => break,
                };

                match msg {
                    Message::Text(txt) => {
                        match serde_json::from_str::<SignalMessage>(&txt) {
                            Ok(signal) => {
                                handle_signal_message(
                                    &signal, &pc, &out_tx, &pending_ice, &shared,
                                ).await?;
                            }
                            Err(e) => {
                                shared.log_line(format!("Bad signaling message: {e}"));
                            }
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }

            // Outbound WebSocket messages (ICE candidates, SDP answers)
            out = out_rx.recv() => {
                let out = match out {
                    Some(m) => m,
                    None => break,
                };
                let txt = serde_json::to_string(&out)?;
                if let Err(e) = socket.send(Message::Text(txt)).await {
                    log::warn!("WebSocket send failed: {e}");
                    break;
                }
            }
        }
    }

    shared.log_line("Closing PeerConnection…");
    pc.close().await?;
    Ok(())
}

/// Process a single inbound signaling message.
async fn handle_signal_message(
    signal: &SignalMessage,
    pc: &Arc<webrtc::peer_connection::RTCPeerConnection>,
    out_tx: &mpsc::Sender<SignalMessage>,
    pending_ice: &Arc<tokio::sync::Mutex<Vec<RTCIceCandidateInit>>>,
    shared: &SharedStatus,
) -> Result<()> {
    match signal.msg_type.as_str() {
        "offer" | "answer" => {
            if let Some(sdp_str) = &signal.sdp {
                let is_offer = signal.msg_type == "offer";
                shared.log_line(format!("Got SDP: {}", signal.msg_type));

                let desc = if is_offer {
                    RTCSessionDescription::offer(sdp_str.clone())
                        .map_err(|e| anyhow!("parse offer: {e}"))?
                } else {
                    RTCSessionDescription::answer(sdp_str.clone())
                        .map_err(|e| anyhow!("parse answer: {e}"))?
                };
                pc.set_remote_description(desc).await?;

                // Apply any ICE candidates that arrived before the remote description
                let mut pend = pending_ice.lock().await;
                for c in pend.drain(..) {
                    if let Err(e) = pc.add_ice_candidate(c).await {
                        log::warn!("Failed to add queued ICE candidate: {e}");
                    }
                }

                // If remote sent an offer, respond with an answer
                if is_offer {
                    let answer = pc.create_answer(None).await?;
                    pc.set_local_description(answer).await?;
                    if let Some(local) = pc.local_description().await {
                        out_tx
                            .send(SignalMessage {
                                msg_type: "answer".to_string(),
                                sdp: Some(local.sdp),
                                candidate: None,
                                sdp_mid: None,
                                sdp_mline_index: None,
                            })
                            .await
                            .map_err(|e| anyhow!("Failed to send answer: {e}"))?;
                    }
                }
            }
        }
        "ice" => {
            if let Some(candidate_str) = &signal.candidate {
                let init = RTCIceCandidateInit {
                    candidate: candidate_str.clone(),
                    sdp_mid: Some(signal.sdp_mid.clone().unwrap_or_default()),
                    sdp_mline_index: Some(signal.sdp_mline_index.unwrap_or(0) as u16),
                    username_fragment: Some(String::new()),
                };
                if pc.remote_description().await.is_none() {
                    pending_ice.lock().await.push(init);
                } else if let Err(e) = pc.add_ice_candidate(init).await {
                    log::warn!("Failed to add ICE candidate: {e}");
                }
            }
        }
        "ping" => { /* keep-alive, ignore */ }
        other => {
            shared.log_line(format!("Unknown message type: {other}"));
        }
    }
    Ok(())
}

async fn create_peer_connection(
    use_stun: bool,
    shared: SharedStatus,
    queue: Arc<ArrayQueue<i16>>,
    out_tx: mpsc::Sender<SignalMessage>,
) -> Result<Arc<webrtc::peer_connection::RTCPeerConnection>> {
    // Media engine + codecs
    let mut m = MediaEngine::default();
    m.register_default_codecs()?;

    // Interceptors (NACK, RTCP reports, etc.)
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m)?;

    let api = APIBuilder::new()
        .with_media_engine(m)
        .with_interceptor_registry(registry)
        .build();

    let ice_servers = if use_stun {
        vec![RTCIceServer {
            urls: vec!["stun:stun.l.google.com:19302".to_owned()],
            ..Default::default()
        }]
    } else {
        vec![]
    };

    let config = RTCConfiguration {
        ice_servers,
        ..Default::default()
    };

    let pc = Arc::new(api.new_peer_connection(config).await?);

    // Add receive-only audio transceiver
    pc.add_transceiver_from_kind(
        RTPCodecType::Audio,
        Some(RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Recvonly,
            send_encodings: vec![],
        }),
    )
    .await?;

    // PeerConnection state change callback
    let shared_pc = shared.clone();
    pc.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
        let shared_pc = shared_pc.clone();
        Box::pin(async move {
            shared_pc.set_pc_state(Some(format!("{s:?}")));
        })
    }));

    // Trickle ICE — forward local candidates to the sender
    let ice_tx = out_tx.clone();
    pc.on_ice_candidate(Box::new(move |c: Option<RTCIceCandidate>| {
        let ice_tx = ice_tx.clone();
        Box::pin(async move {
            if let Some(c) = c {
                if let Ok(init) = c.to_json() {
                    let msg = SignalMessage {
                        msg_type: "ice".to_string(),
                        sdp: None,
                        candidate: Some(init.candidate),
                        sdp_mid: init.sdp_mid,
                        sdp_mline_index: init.sdp_mline_index.map(|v| v as i32),
                    };
                    if let Err(e) = ice_tx.send(msg).await {
                        log::warn!("Failed to send ICE candidate: {e}");
                    }
                }
            }
        })
    }));

    // When remote audio track arrives, decode Opus and push to CPAL queue
    let shared_track = shared.clone();
    pc.on_track(Box::new(move |track, _receiver, _transceiver| {
        let queue = queue.clone();
        let shared_track = shared_track.clone();

        Box::pin(async move {
            if track.kind() != RTPCodecType::Audio {
                return;
            }

            let codec = track.codec();
            shared_track.log_line(format!("Audio track: {}", codec.capability.mime_type));
            let ch = codec.capability.channels as usize;
            let channels = if ch >= 2 { 2 } else { 1 };

            tokio::spawn(async move {
                if let Err(e) =
                    decode_track_to_queue(track, queue, channels, shared_track.clone()).await
                {
                    shared_track.log_line(format!("Audio decode stopped: {e}"));
                }
            });
        })
    }));

    Ok(pc)
}

async fn decode_track_to_queue(
    track: Arc<webrtc::track::track_remote::TrackRemote>,
    queue: Arc<ArrayQueue<i16>>,
    channels: usize,
    shared: SharedStatus,
) -> Result<()> {
    let opus_channels = if channels >= 2 {
        Channels::Stereo
    } else {
        Channels::Mono
    };
    let mut dec =
        OpusDecoder::new(48_000, opus_channels).map_err(|e| anyhow!("opus decoder init: {e:?}"))?;

    // Buffer large enough for max Opus frame (60ms @ 48kHz) × stereo
    let max_samples_per_channel = 5760;
    let mut pcm = vec![0i16; max_samples_per_channel * channels];

    // Track dropped samples for periodic logging
    let dropped = AtomicU64::new(0);
    let mut last_log = std::time::Instant::now();

    loop {
        let (rtp, _attr) = track
            .read_rtp()
            .await
            .map_err(|e| anyhow!("read_rtp: {e}"))?;
        shared.bump_audio_packets(1);

        if rtp.payload.is_empty() {
            continue;
        }

        let n = dec
            .decode(&rtp.payload, &mut pcm, false)
            .map_err(|e| anyhow!("opus decode: {e:?}"))?;

        if n == 0 {
            continue;
        }

        let mut local_dropped = 0u64;

        if channels >= 2 {
            // Downmix stereo to mono for the output queue
            for i in 0..n {
                let l = pcm[i * 2] as i32;
                let r = pcm[i * 2 + 1] as i32;
                let m = ((l + r) / 2) as i16;
                if queue.push(m).is_err() {
                    local_dropped += 1;
                }
            }
        } else {
            for i in 0..n {
                if queue.push(pcm[i]).is_err() {
                    local_dropped += 1;
                }
            }
        }

        // Accumulate and periodically log drops
        if local_dropped > 0 {
            let total = dropped.fetch_add(local_dropped, Ordering::Relaxed) + local_dropped;
            if last_log.elapsed().as_secs() >= 5 {
                shared.log_line(format!("Audio queue overflow: {} samples dropped", total));
                last_log = std::time::Instant::now();
                dropped.store(0, Ordering::Relaxed);
            }
        }
    }
}
