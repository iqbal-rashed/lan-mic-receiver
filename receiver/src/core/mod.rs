pub mod signaling;

use crate::audio;
use anyhow::Result;
use crossbeam_queue::ArrayQueue;
use parking_lot::Mutex;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum CoreCommand {
    Start {
        bind_addr: String,
        output_device: Option<String>,
        use_stun: bool,
    },
    Stop,
}

pub struct CoreController {
    tx: std::sync::Arc<tokio::sync::mpsc::UnboundedSender<CoreCommand>>,
}

impl Clone for CoreController {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

impl PartialEq for CoreController {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.tx, &other.tx
        )
    }
}

impl CoreController {
    pub fn send(&self, cmd: CoreCommand) -> Result<(), tokio::sync::mpsc::error::SendError<CoreCommand>> {
        self.tx.send(cmd)
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct StatusSnapshot {
    pub server_running: bool,
    pub ws_url: Option<String>,
    pub client_connected: bool,
    pub pc_state: Option<String>,
    pub last_error: Option<String>,
    pub audio_packets: u64,
    pub log_lines: Vec<String>,
}

#[derive(Debug, Default)]
struct Status {
    server_running: bool,
    ws_url: Option<String>,
    client_connected: bool,
    pc_state: Option<String>,
    last_error: Option<String>,
    audio_packets: u64,
    log_lines: Vec<String>,
}

#[derive(Clone, Default)]
pub struct SharedStatus {
    inner: Arc<Mutex<Status>>,
}

impl SharedStatus {
    pub fn snapshot(&self) -> StatusSnapshot {
        let s = self.inner.lock();
        StatusSnapshot {
            server_running: s.server_running,
            ws_url: s.ws_url.clone(),
            client_connected: s.client_connected,
            pc_state: s.pc_state.clone(),
            last_error: s.last_error.clone(),
            audio_packets: s.audio_packets,
            log_lines: s.log_lines.clone(),
        }
    }

    fn set_server_running(&self, running: bool) {
        self.inner.lock().server_running = running;
    }

    fn set_ws_url(&self, url: Option<String>) {
        self.inner.lock().ws_url = url;
    }

    pub fn set_client_connected(&self, connected: bool) {
        self.inner.lock().client_connected = connected;
    }

    pub fn set_pc_state(&self, state: Option<String>) {
        self.inner.lock().pc_state = state;
    }

    pub fn set_last_error(&self, err: Option<String>) {
        self.inner.lock().last_error = err;
    }

    pub fn bump_audio_packets(&self, n: u64) {
        let mut s = self.inner.lock();
        s.audio_packets = s.audio_packets.saturating_add(n);
    }

    pub fn log_line(&self, line: impl Into<String>) {
        let mut s = self.inner.lock();
        let line = line.into();
        s.log_lines.push(line);
        if s.log_lines.len() > 1500 {
            let drain = s.log_lines.len() - 1500;
            s.log_lines.drain(0..drain);
        }
    }
}

struct Running {
    server: signaling::ServerHandle,
    _audio: audio::AudioOutput,
    _queue: Arc<ArrayQueue<i16>>,
}

pub fn spawn_runtime(shared: SharedStatus) -> CoreController {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<CoreCommand>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.block_on(async move {
            let mut running: Option<Running> = None;

            while let Some(cmd) = rx.recv().await {
                match cmd {
                    CoreCommand::Start { bind_addr, output_device, use_stun } => {
                        // Stop any existing run
                        if let Some(r) = running.take() {
                            shared.log_line("Stopping previous server…");
                            let _ = r.server.shutdown().await;
                            shared.set_server_running(false);
                        }

                        shared.set_last_error(None);

                        // Audio queue (mono i16 @48k)
                        let queue = Arc::new(ArrayQueue::<i16>::new(48_000)); // ~1 second buffer

                        // Start audio output
                        match audio::AudioOutput::start(output_device.as_deref(), Arc::clone(&queue)) {
                            Ok(audio_out) => {
                                shared.log_line(format!(
                                    "Audio output started: {}",
                                    audio_out.device_name()
                                ));

                                // Start signaling + webrtc
                                match signaling::start_server(
                                    bind_addr.clone(),
                                    Arc::clone(&queue),
                                    use_stun,
                                    shared.clone(),
                                )
                                .await
                                {
                                    Ok(server) => {
                                        shared.set_server_running(true);
                                        shared.set_ws_url(Some(server.ws_url.clone()));
                                        shared.log_line(format!("Signaling server listening on {}", server.bind_addr));
                                        shared.log_line(format!("WebSocket URL: {}", server.ws_url));
                                        running = Some(Running { server, _audio: audio_out, _queue: queue });
                                    }
                                    Err(e) => {
                                        shared.set_last_error(Some(e.to_string()));
                                        shared.log_line(format!("Failed to start server: {e}"));
                                        shared.set_server_running(false);
                                        shared.set_ws_url(None);
                                    }
                                }
                            }
                            Err(e) => {
                                shared.set_last_error(Some(e.to_string()));
                                shared.log_line(format!("Failed to start audio output: {e}"));
                            }
                        }
                    }
                    CoreCommand::Stop => {
                        if let Some(r) = running.take() {
                            shared.log_line("Stopping…");
                            let _ = r.server.shutdown().await;
                        }
                        shared.set_server_running(false);
                        shared.set_ws_url(None);
                        shared.set_client_connected(false);
                        shared.set_pc_state(None);
                        shared.log_line("Stopped.");
                    }
                }
            }
        });
    });

    CoreController { tx: std::sync::Arc::new(tx) }
}
