pub mod signaling;

use crate::audio;
use anyhow::Result;
use crossbeam_queue::ArrayQueue;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Maximum log lines retained in memory.
const MAX_LOG_LINES: usize = 1500;

// ---------------------------------------------------------------------------
// Commands sent from the UI to the core runtime
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum CoreCommand {
    Start {
        bind_addr: String,
        output_device: Option<String>,
        use_stun: bool,
    },
    Stop,
    ChangeOutputDevice {
        device_name: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Controller — the UI-thread handle for sending commands
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct CoreController {
    tx: Arc<tokio::sync::mpsc::UnboundedSender<CoreCommand>>,
}

impl PartialEq for CoreController {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.tx, &other.tx)
    }
}

impl CoreController {
    pub fn send(
        &self,
        cmd: CoreCommand,
    ) -> Result<(), tokio::sync::mpsc::error::SendError<CoreCommand>> {
        self.tx.send(cmd)
    }
}

// ---------------------------------------------------------------------------
// Shared status — thread-safe state visible to both UI and core
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, PartialEq)]
pub struct StatusSnapshot {
    pub server_running: bool,
    pub ws_url: Option<String>,
    pub client_connected: bool,
    pub client_addr: Option<String>,
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
    client_addr: Option<String>,
    pc_state: Option<String>,
    last_error: Option<String>,
    audio_packets: u64,
    log_lines: VecDeque<String>,
}

#[derive(Clone, Default)]
pub struct SharedStatus {
    inner: Arc<Mutex<Status>>,
}

impl SharedStatus {
    /// Take a consistent snapshot of the entire status in one lock acquisition.
    pub fn snapshot(&self) -> StatusSnapshot {
        let s = self.inner.lock();
        StatusSnapshot {
            server_running: s.server_running,
            ws_url: s.ws_url.clone(),
            client_connected: s.client_connected,
            client_addr: s.client_addr.clone(),
            pc_state: s.pc_state.clone(),
            last_error: s.last_error.clone(),
            audio_packets: s.audio_packets,
            log_lines: s.log_lines.iter().cloned().collect(),
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

    pub fn set_client_addr(&self, addr: Option<String>) {
        self.inner.lock().client_addr = addr;
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
        s.log_lines.push_back(line.into());
        while s.log_lines.len() > MAX_LOG_LINES {
            s.log_lines.pop_front();
        }
    }

    /// Reset all connection-related fields in a single lock acquisition.
    fn reset_connection(&self) {
        let mut s = self.inner.lock();
        s.server_running = false;
        s.client_connected = false;
        s.client_addr = None;
        s.pc_state = None;
    }
}

// ---------------------------------------------------------------------------
// Core runtime — runs on a dedicated thread with its own tokio runtime
// ---------------------------------------------------------------------------

struct Running {
    audio: audio::AudioOutput,
    queue: Arc<ArrayQueue<i16>>,
    _session_cancel: CancellationToken,
    mdns: Option<signaling::MdnsRegistration>,
}

pub fn spawn_runtime(shared: SharedStatus) -> CoreController {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<CoreCommand>();
    let tx = Arc::new(tx);

    let shared_for_thread = shared.clone();
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                shared_for_thread
                    .set_last_error(Some(format!("Failed to create tokio runtime: {e}")));
                shared_for_thread.log_line(format!("Runtime creation failed: {e}"));
                return;
            }
        };

        rt.block_on(async move {
            // Start the HTTP server immediately so the web sender page is always available
            let http_server = match signaling::start_http_server(
                "0.0.0.0:9001".to_string(),
                shared.clone(),
            )
            .await
            {
                Ok(server) => {
                    shared.set_ws_url(Some(server.ws_url.clone()));
                    shared.log_line(format!(
                        "Web sender available at http://{}",
                        server.bind_addr
                    ));
                    server
                }
                Err(e) => {
                    shared.set_last_error(Some(e.to_string()));
                    shared.log_line(format!("Failed to start HTTP server: {e}"));
                    return;
                }
            };

            let mut running: Option<Running> = None;

            while let Some(cmd) = rx.recv().await {
                match cmd {
                    CoreCommand::Start {
                        bind_addr: _,
                        output_device,
                        use_stun,
                    } => {
                        // Stop any existing run first
                        if let Some(r) = running.take() {
                            shared.log_line("Stopping previous session…");
                            http_server.deactivate().await;
                            if let Some(mdns) = r.mdns {
                                mdns.shutdown();
                            }
                            shared.set_server_running(false);
                        }

                        shared.set_last_error(None);

                        // Audio queue (mono i16 @ 48 kHz, ~1 second buffer)
                        let queue = Arc::new(ArrayQueue::<i16>::new(48_000));

                        // Start audio output
                        match audio::AudioOutput::start(
                            output_device.as_deref(),
                            Arc::clone(&queue),
                        ) {
                            Ok(audio_out) => {
                                shared.log_line(format!(
                                    "Audio output started: {}",
                                    audio_out.device_name()
                                ));

                                // Activate WebSocket connections on the already-running server
                                let session_cancel = http_server
                                    .activate(Arc::clone(&queue), use_stun)
                                    .await;

                                // Register mDNS for auto-discovery
                                let mdns =
                                    signaling::MdnsRegistration::register(9001, &shared);

                                shared.set_server_running(true);
                                shared.log_line(format!(
                                    "Listening on {}",
                                    http_server.bind_addr
                                ));
                                shared.log_line(format!(
                                    "WebSocket URL: {}",
                                    http_server.ws_url
                                ));

                                running = Some(Running {
                                    audio: audio_out,
                                    queue,
                                    _session_cancel: session_cancel,
                                    mdns,
                                });
                            }
                            Err(e) => {
                                shared.set_last_error(Some(e.to_string()));
                                shared.log_line(format!(
                                    "Failed to start audio output: {e}"
                                ));
                            }
                        }
                    }
                    CoreCommand::Stop => {
                        if let Some(r) = running.take() {
                            shared.log_line("Stopping…");
                            http_server.deactivate().await;
                            if let Some(mdns) = r.mdns {
                                mdns.shutdown();
                            }
                        }
                        shared.reset_connection();
                        shared.log_line("Stopped.");
                    }
                    CoreCommand::ChangeOutputDevice { device_name } => {
                        if let Some(ref mut r) = running {
                            let old_device = r.audio.device_name().to_string();
                            shared.log_line(format!(
                                "Switching audio from '{old_device}'…"
                            ));

                            // Drop old stream to stop its cpal callback
                            let queue_ref = Arc::clone(&r.queue);
                            let old_audio = std::mem::replace(
                                &mut r.audio,
                                audio::AudioOutput::stopped(),
                            );
                            drop(old_audio);

                            // Brief pause for cpal callback thread to stop
                            tokio::time::sleep(std::time::Duration::from_millis(50))
                                .await;

                            // Drain stale samples
                            while queue_ref.pop().is_some() {}

                            // Start new stream on the selected device
                            match audio::AudioOutput::start(
                                device_name.as_deref(),
                                Arc::clone(&r.queue),
                            ) {
                                Ok(new_audio) => {
                                    shared.log_line(format!(
                                        "Audio output switched to: {}",
                                        new_audio.device_name()
                                    ));
                                    r.audio = new_audio;
                                }
                                Err(e) => {
                                    shared.set_last_error(Some(e.to_string()));
                                    shared.log_line(format!(
                                        "Failed to switch audio: {e}"
                                    ));
                                    if let Ok(fallback) = audio::AudioOutput::start(
                                        Some(&old_device),
                                        Arc::clone(&r.queue),
                                    ) {
                                        shared.log_line(
                                            "Reverted to previous audio device",
                                        );
                                        r.audio = fallback;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
    });

    CoreController { tx }
}
