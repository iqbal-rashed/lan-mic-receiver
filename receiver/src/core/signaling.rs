use crate::core::SharedStatus;
use anyhow::{anyhow, Result};
use axum::{
    extract::{ws::WebSocketUpgrade, ConnectInfo, State},
    response::Response,
    routing::get,
    Router,
};
use crossbeam_queue::ArrayQueue;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

mod webrtc_session;

/// mDNS service type for LAN Mic discovery.
const MDNS_SERVICE_TYPE: &str = "_lanmic._tcp.local.";

#[derive(Clone)]
struct AppState {
    queue: Arc<ArrayQueue<i16>>,
    use_stun: bool,
    shared: SharedStatus,
    active: Arc<tokio::sync::Mutex<bool>>,
}

pub struct ServerHandle {
    pub bind_addr: String,
    pub ws_url: String,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    join: tokio::task::JoinHandle<Result<()>>,
    mdns_daemon: Option<ServiceDaemon>,
    mdns_fullname: Option<String>,
}

impl ServerHandle {
    pub async fn shutdown(mut self) -> Result<()> {
        // Unregister mDNS service first
        if let (Some(daemon), Some(fullname)) = (self.mdns_daemon.take(), self.mdns_fullname.take())
        {
            log::info!("Unregistering mDNS service: {fullname}");
            if let Err(e) = daemon.unregister(&fullname) {
                log::warn!("mDNS unregister error: {e}");
            }
            if let Err(e) = daemon.shutdown() {
                log::warn!("mDNS daemon shutdown error: {e}");
            }
        }

        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        match self.join.await {
            Ok(r) => r,
            Err(e) => Err(anyhow!("server task join error: {e}")),
        }
    }
}

pub async fn start_server(
    bind_addr: String,
    queue: Arc<ArrayQueue<i16>>,
    use_stun: bool,
    shared: SharedStatus,
) -> Result<ServerHandle> {
    let listener = TcpListener::bind(&bind_addr).await?;
    let local_addr = listener.local_addr()?;

    let ip = pick_local_ip().unwrap_or_else(|| local_addr.ip().to_string());
    let ws_url = format!("ws://{}:{}/ws", ip, local_addr.port());

    // Register mDNS service for auto-discovery
    let (mdns_daemon, mdns_fullname) = match register_mdns_service(&ip, local_addr.port()) {
        Ok((daemon, fullname)) => {
            shared.log_line(format!("mDNS service registered: {fullname}"));
            (Some(daemon), Some(fullname))
        }
        Err(e) => {
            shared.log_line(format!("mDNS registration failed (non-fatal): {e}"));
            log::warn!("mDNS registration error: {e}");
            (None, None)
        }
    };

    let state = AppState {
        queue,
        use_stun,
        shared: shared.clone(),
        active: Arc::new(tokio::sync::Mutex::new(false)),
    };

    let app = Router::new()
        .route(
            "/",
            get(|| async { "LAN Mic Receiver (WebRTC) running. Connect WebSocket at /ws" }),
        )
        .route("/ws", get(ws_handler))
        .with_state(state);

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let join = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        })
        .await
        .map_err(|e| anyhow!("axum serve error: {e}"))?;
        Ok(())
    });

    Ok(ServerHandle {
        bind_addr,
        ws_url,
        shutdown_tx: Some(shutdown_tx),
        join,
        mdns_daemon,
        mdns_fullname,
    })
}

/// Register this receiver as a Bonjour/mDNS service on the local network.
fn register_mdns_service(ip: &str, port: u16) -> Result<(ServiceDaemon, String)> {
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
        None, // no TXT properties needed
    )?;

    let fullname = service.get_fullname().to_string();
    daemon.register(service)?;

    log::info!("mDNS: advertising {fullname} at {ip}:{port}");
    Ok((daemon, fullname))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<AppState>,
) -> Response {
    let client_ip = addr.to_string();
    ws.on_upgrade(move |socket| async move {
        // One active connection at a time
        {
            let mut active = state.active.lock().await;
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

        let res =
            webrtc_session::run(socket, state.queue, state.use_stun, state.shared.clone()).await;

        if let Err(e) = &res {
            state.shared.set_last_error(Some(e.to_string()));
            state.shared.log_line(format!("Session error: {e}"));
        }

        state.shared.set_client_connected(false);
        state.shared.set_client_addr(None);
        state.shared.set_pc_state(None);
        state.shared.log_line("WebSocket client disconnected.");

        let mut active = state.active.lock().await;
        *active = false;
    })
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
