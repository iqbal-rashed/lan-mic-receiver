use crate::core::SharedStatus;
use anyhow::{anyhow, Result};
use axum::{
    extract::{ws::WebSocketUpgrade, ConnectInfo, State},
    response::{Html, Response},
    routing::get,
    Router,
};
use axum_server::tls_rustls::RustlsConfig;
use crossbeam_queue::ArrayQueue;
use rcgen::generate_simple_self_signed;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[cfg(not(target_os = "macos"))]
use mdns_sd::{ServiceDaemon, ServiceInfo};

mod webrtc_session;

/// mDNS service type for LAN Mic discovery.
const MDNS_SERVICE_TYPE: &str = "_lanmic._tcp.local.";

/// Embed the web sender app at compile time.
const SENDER_HTML: &str = include_str!("../../../sender(web)/index.html");

/// Shared state for the axum server.
///
/// `session_state` is `None` until the user clicks START, at which point
/// it is populated with the queue, STUN flag, and cancellation token.
/// WebSocket connections are rejected while it is `None`.
#[derive(Clone)]
struct AppState {
    shared: SharedStatus,
    /// Populated when the user clicks START; cleared on STOP.
    session_state: Arc<tokio::sync::RwLock<Option<SessionState>>>,
}

#[derive(Clone)]
struct SessionState {
    queue: Arc<ArrayQueue<i16>>,
    use_stun: bool,
    active: Arc<tokio::sync::Mutex<bool>>,
    session_cancel: CancellationToken,
}

/// Platform-specific mDNS handle.
enum MdnsHandle {
    /// macOS: native `dns-sd -R` child process
    #[cfg(target_os = "macos")]
    NativeProcess(std::process::Child),
    /// Windows/Linux: mdns-sd crate daemon
    #[cfg(not(target_os = "macos"))]
    CrateDaemon {
        daemon: ServiceDaemon,
        fullname: String,
    },
}

impl MdnsHandle {
    fn shutdown(self) {
        match self {
            #[cfg(target_os = "macos")]
            MdnsHandle::NativeProcess(mut child) => {
                log::info!("Stopping mDNS registration process");
                let _ = child.kill();
                let _ = child.wait();
            }
            #[cfg(not(target_os = "macos"))]
            MdnsHandle::CrateDaemon { daemon, fullname } => {
                log::info!("Unregistering mDNS service: {fullname}");
                if let Err(e) = daemon.unregister(&fullname) {
                    log::warn!("mDNS unregister error: {e}");
                }
                if let Err(e) = daemon.shutdown() {
                    log::warn!("mDNS daemon shutdown error: {e}");
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// HttpServer — started immediately on app launch, serves the web sender page
// ---------------------------------------------------------------------------

pub struct HttpServer {
    pub bind_addr: String,
    pub ws_url: String,
    session_state: Arc<tokio::sync::RwLock<Option<SessionState>>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    join: tokio::task::JoinHandle<Result<()>>,
}

impl HttpServer {
    /// Activate WebSocket connections. Called when user clicks START.
    /// Returns the `SessionCancel` token for tracking active sessions.
    pub async fn activate(
        &self,
        queue: Arc<ArrayQueue<i16>>,
        use_stun: bool,
    ) -> CancellationToken {
        let cancel = CancellationToken::new();
        let state = SessionState {
            queue,
            use_stun,
            active: Arc::new(tokio::sync::Mutex::new(false)),
            session_cancel: cancel.clone(),
        };
        *self.session_state.write().await = Some(state);
        cancel
    }

    /// Deactivate WebSocket connections and cancel active sessions. Called on STOP.
    pub async fn deactivate(&self) {
        if let Some(state) = self.session_state.write().await.take() {
            state.session_cancel.cancel();
        }
    }

    /// Shut down the HTTP server entirely.
    pub async fn shutdown(mut self) -> Result<()> {
        self.deactivate().await;
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        match self.join.await {
            Ok(r) => r,
            Err(e) => Err(anyhow!("server task join error: {e}")),
        }
    }
}

/// Handles for mDNS that live alongside the server but are separate.
pub struct MdnsRegistration {
    handle: MdnsHandle,
}

impl MdnsRegistration {
    pub fn register(port: u16, shared: &SharedStatus) -> Option<Self> {
        let ip = pick_local_ip().unwrap_or_else(|| "0.0.0.0".to_string());
        match register_mdns(&ip, port) {
            Ok(handle) => {
                shared.log_line("mDNS service registered");
                Some(Self { handle })
            }
            Err(e) => {
                shared.log_line(format!("mDNS registration failed (non-fatal): {e}"));
                log::warn!("mDNS registration error: {e}");
                None
            }
        }
    }

    pub fn shutdown(self) {
        self.handle.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Start HTTP server — called once at app launch
// ---------------------------------------------------------------------------

pub async fn start_http_server(
    bind_addr: String,
    shared: SharedStatus,
) -> Result<HttpServer> {
    // Generate self-signed certificate
    let subject_alt_names = vec!["localhost".to_string(), "lan-mic-receiver".to_string()];
    let cert = generate_simple_self_signed(subject_alt_names)?;
    let tls_config = RustlsConfig::from_pem(
        cert.cert.pem().into_bytes(),
        cert.key_pair.serialize_pem().into_bytes(),
    )
    .await?;

    let addr: SocketAddr = bind_addr.parse()?;
    let ip = pick_local_ip().unwrap_or_else(|| addr.ip().to_string());
    let ws_url = format!("wss://{}:{}/ws", ip, addr.port());

    let session_state: Arc<tokio::sync::RwLock<Option<SessionState>>> =
        Arc::new(tokio::sync::RwLock::new(None));

    let state = AppState {
        shared: shared.clone(),
        session_state: session_state.clone(),
    };

    let app = Router::new()
        .route("/", get(|| async { Html(SENDER_HTML) }))
        .route("/ws", get(ws_handler))
        .with_state(state);

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let join = tokio::spawn(async move {
        let handle = axum_server::Handle::new();
        let handle_clone = handle.clone();
        
        // Spawn a task to listen for shutdown signal
        tokio::spawn(async move {
            let _ = shutdown_rx.await;
            handle_clone.graceful_shutdown(None);
        });

        axum_server::bind_rustls(addr, tls_config)
            .handle(handle)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .map_err(|e| anyhow!("axum serve error: {e}"))?;
        Ok(())
    });

    let bind_addr_str = format!("{}:{}", ip, addr.port());

    Ok(HttpServer {
        bind_addr: bind_addr_str,
        ws_url,
        session_state,
        shutdown_tx: Some(shutdown_tx),
        join,
    })
}

// ---------------------------------------------------------------------------
// WebSocket handler
// ---------------------------------------------------------------------------

async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Response {
    let client_ip = addr.to_string();
    ws.on_upgrade(move |socket| async move {
        // Check if server is activated (user clicked START)
        let session = {
            let guard = state.session_state.read().await;
            guard.clone()
        };

        let session = match session {
            Some(s) => s,
            None => {
                state
                    .shared
                    .log_line("Rejected WebSocket: server not started.");
                return;
            }
        };

        // One active connection at a time
        {
            let mut active = session.active.lock().await;
            if *active {
                state
                    .shared
                    .log_line("Rejected WebSocket: already connected.");
                return;
            }
            *active = true;
        }

        state.shared.set_client_connected(true);
        state.shared.set_client_addr(Some(client_ip));
        state.shared.set_pc_state(Some("new".into()));
        state.shared.log_line("WebSocket client connected.");

        let res = webrtc_session::run(
            socket,
            session.queue,
            session.use_stun,
            state.shared.clone(),
            session.session_cancel,
        )
        .await;

        if let Err(e) = &res {
            state.shared.set_last_error(Some(e.to_string()));
            state.shared.log_line(format!("Session error: {e}"));
        }

        state.shared.set_client_connected(false);
        state.shared.set_client_addr(None);
        state.shared.set_pc_state(None);
        state.shared.log_line("WebSocket client disconnected.");

        let mut active = session.active.lock().await;
        *active = false;
    })
}

// ---------------------------------------------------------------------------
// mDNS registration — platform-specific
// ---------------------------------------------------------------------------

/// macOS: use native `dns-sd -R` command (integrates with mDNSResponder).
#[cfg(target_os = "macos")]
fn register_mdns(_ip: &str, port: u16) -> Result<MdnsHandle> {
    let hostname = gethostname::gethostname()
        .into_string()
        .unwrap_or_else(|_| "lan-mic-receiver".to_string());
    let service_name = format!("LAN Mic Receiver ({})", hostname);

    let child = std::process::Command::new("dns-sd")
        .args(["-R", &service_name, MDNS_SERVICE_TYPE, "local.", &port.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn dns-sd: {e}"))?;

    log::info!(
        "mDNS: advertising '{}' on port {} via native dns-sd (pid {})",
        service_name, port, child.id()
    );
    Ok(MdnsHandle::NativeProcess(child))
}

/// Windows/Linux: use the `mdns-sd` crate.
#[cfg(not(target_os = "macos"))]
fn register_mdns(ip: &str, port: u16) -> Result<MdnsHandle> {
    let daemon = ServiceDaemon::new()?;
    let hostname = gethostname::gethostname()
        .into_string()
        .unwrap_or_else(|_| "lan-mic-receiver".to_string());

    let service_name = format!("LAN Mic Receiver ({})", hostname);
    let host = format!("{hostname}.local.");

    let service = ServiceInfo::new(
        MDNS_SERVICE_TYPE,
        &service_name,
        &host,
        ip,
        port,
        None,
    )?;

    let fullname = service.get_fullname().to_string();
    daemon.register(service)?;

    log::info!("mDNS: advertising {fullname} at {ip}:{port}");
    Ok(MdnsHandle::CrateDaemon { daemon, fullname })
}

/// Best-effort: pick an IPv4 LAN address to show in UI.
fn pick_local_ip() -> Option<String> {
    match local_ip_address::list_afinet_netifas() {
        Ok(list) => {
            // Prefer private IPv4 addresses
            for (_name, ip) in &list {
                if let std::net::IpAddr::V4(v4) = ip {
                    if v4.is_private() {
                        return Some(v4.to_string());
                    }
                }
            }
            // Else first IPv4
            for (_name, ip) in &list {
                if let std::net::IpAddr::V4(v4) = ip {
                    return Some(v4.to_string());
                }
            }
            None
        }
        Err(_) => local_ip_address::local_ip().ok().map(|ip| ip.to_string()),
    }
}
