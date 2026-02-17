use crate::core::SharedStatus;
use anyhow::{anyhow, Result};
use axum::extract::ws::{Message, WebSocket};
use crossbeam_queue::ArrayQueue;
use opus::{Channels, Decoder as OpusDecoder};
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "lowercase")]
enum SignalMessage {
    Sdp(RTCSessionDescription),
    Ice(RTCIceCandidateInit),
    Ping,
}

pub async fn run(
    mut socket: WebSocket,
    queue: Arc<ArrayQueue<i16>>,
    use_stun: bool,
    shared: SharedStatus,
) -> Result<()> {
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<SignalMessage>();

    let pc =
        create_peer_connection(use_stun, shared.clone(), queue.clone(), out_tx.clone()).await?;
    shared.set_pc_state(Some("created".into()));

    // Pending ICE candidates that arrive before remote description
    let pending_ice: Arc<tokio::sync::Mutex<Vec<RTCIceCandidateInit>>> =
        Arc::new(tokio::sync::Mutex::new(vec![]));

    loop {
        tokio::select! {
            // inbound WS
            msg = socket.recv() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    Some(Err(_)) => break,
                    None => break,
                };

                match msg {
                    Message::Text(txt) => {
                        let parsed: Result<SignalMessage, _> = serde_json::from_str(&txt);
                        match parsed {
                            Ok(SignalMessage::Sdp(desc)) => {
                                shared.log_line(format!("Got SDP: {:?}", desc.sdp_type));
                                pc.set_remote_description(desc).await?;
                                // Apply pending ICE
                                let mut pend = pending_ice.lock().await;
                                for c in pend.drain(..) {
                                    let _ = pc.add_ice_candidate(c).await;
                                }

                                // If remote was an offer, answer it
                                let answer = pc.create_answer(None).await?;
                                pc.set_local_description(answer).await?;
                                if let Some(local) = pc.local_description().await {
                                    let _ = out_tx.send(SignalMessage::Sdp(local));
                                }
                            }
                            Ok(SignalMessage::Ice(cand)) => {
                                if pc.remote_description().await.is_none() {
                                    pending_ice.lock().await.push(cand);
                                } else {
                                    let _ = pc.add_ice_candidate(cand).await;
                                }
                            }
                            Ok(SignalMessage::Ping) => {
                                // ignore
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

            // outbound WS
            out = out_rx.recv() => {
                let out = match out {
                    Some(m) => m,
                    None => break,
                };
                let txt = serde_json::to_string(&out)?;
                if socket.send(Message::Text(txt)).await.is_err() {
                    break;
                }
            }
        }
    }

    shared.log_line("Closing PeerConnection…");
    pc.close().await?;
    Ok(())
}

async fn create_peer_connection(
    use_stun: bool,
    shared: SharedStatus,
    queue: Arc<ArrayQueue<i16>>,
    out_tx: mpsc::UnboundedSender<SignalMessage>,
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

    // Ensure we can receive audio
    pc.add_transceiver_from_kind(
        RTPCodecType::Audio,
        Some(RTCRtpTransceiverInit {
            direction: RTCRtpTransceiverDirection::Recvonly,
            send_encodings: vec![],
        }),
    )
    .await?;

    // PeerConnection state
    let shared_pc = shared.clone();
    pc.on_peer_connection_state_change(Box::new(move |s: RTCPeerConnectionState| {
        let shared_pc = shared_pc.clone();
        Box::pin(async move {
            shared_pc.set_pc_state(Some(format!("{s:?}")));
        })
    }));

    // Trickle ICE out to the sender
    let ice_tx = out_tx.clone();
    pc.on_ice_candidate(Box::new(move |c: Option<RTCIceCandidate>| {
        let ice_tx = ice_tx.clone();
        Box::pin(async move {
            if let Some(c) = c {
                if let Ok(init) = c.to_json() {
                    let _ = ice_tx.send(SignalMessage::Ice(init));
                }
            }
        })
    }));

    // When remote track arrives, decode audio (Opus) and output via CPAL queue
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

    // Enough for max opus frame (60ms @ 48k) * stereo
    let mut pcm = vec![0i16; 5760];

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

        if channels >= 2 {
            for i in 0..n {
                let l = pcm[i * 2] as i32;
                let r = pcm[i * 2 + 1] as i32;
                let m = ((l + r) / 2) as i16;
                let _ = queue.push(m);
            }
        } else {
            for i in 0..n {
                let _ = queue.push(pcm[i]);
            }
        }
    }
}
