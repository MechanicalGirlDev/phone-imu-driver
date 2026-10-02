//! Single-origin HTTPS server: static page (the `web/` directory, served as-is) + `/ws` WebSocket.
//!
//! Received device-frame samples are kept latest-wins and read synchronously through
//! [`PhoneImuHandle::latest`] (meant to be called from a control loop or a driver `read()`).
//! Only one WS client is active at a time: when a new connection arrives, the old one is
//! sent a `status` message and disconnected (newest wins - so a stale socket left behind
//! by a phone page reload does not get in the way).

use alloc::sync::Arc;
use core::net::SocketAddr;
use core::sync::atomic::{AtomicU64, Ordering};
use std::path::PathBuf;
use std::time::Instant;

use axum::Router;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum_server::tls_rustls::RustlsConfig;
use tower_http::services::ServeDir;
use tracing::{debug, info, warn};

use crate::cert::{self, CertError};
use crate::protocol::{ClientMessage, PhoneImuSample, ServerMessage, parse_sample_frame};

/// Server configuration. Paths must already be resolved by the caller (absolute or relative
/// to the working directory).
#[derive(Debug, Clone)]
pub struct PhoneImuServerConfig {
    /// HTTPS/WSS listen address.
    pub listen: SocketAddr,
    /// Web root to serve statically (this repository's `web/`: plain HTML/ESM, no build
    /// step). When None or nonexistent, a built-in guidance page is served instead (the WS
    /// still works, so connectivity can be checked).
    pub web_root: Option<PathBuf>,
    /// Where the self-signed TLS certificate is generated and stored.
    pub cert_dir: PathBuf,
}

/// Failure to start the server.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error(transparent)]
    /// The self-signed certificate could not be loaded or generated.
    Cert(#[from] CertError),
    #[error("TLS config rejected the generated PEM: {0}")]
    /// rustls refused the PEM pair.
    Tls(std::io::Error),
    /// The listening socket could not be bound.
    #[error("failed to bind {addr}: {source}")]
    Bind {
        /// The address the server tried to listen on.
        addr: SocketAddr,
        /// The underlying OS error.
        source: std::io::Error,
    },
}

/// One sample with its receive time and connection generation.
///
/// `connection_id` is a generation number that increases with every WS connection. A
/// consumer detects "a new connection" from a change of generation and re-takes the yaw
/// zero point.
#[derive(Debug, Clone, Copy)]
pub struct StampedSample {
    /// When the server received the frame (freshness checks are based on this).
    pub received: Instant,
    /// Generation number (starting at 1) of the WS connection that sent this sample.
    pub connection_id: u64,
    /// The decoded sample, still in the device frame.
    pub sample: PhoneImuSample,
}

/// State shared between the server and consumers (driver / CLI).
#[derive(Debug)]
struct Shared {
    latest: std::sync::Mutex<Option<StampedSample>>,
    /// Generation number of the currently active connection (0 = no connection).
    active_conn: AtomicU64,
    /// Counter that hands out connection generations.
    next_conn: AtomicU64,
    /// Total number of samples received (for the CLI rate display).
    samples: AtomicU64,
}

/// Read handle onto the received samples. Cloneable and thread-safe.
#[derive(Debug, Clone)]
pub struct PhoneImuHandle {
    shared: Arc<Shared>,
}

impl PhoneImuHandle {
    /// The latest sample (with receive time and connection generation). None if nothing has
    /// been received yet.
    pub fn latest(&self) -> Option<StampedSample> {
        *self.shared.latest.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether a WS client is connected.
    pub fn connected(&self) -> bool {
        self.shared.active_conn.load(Ordering::Relaxed) != 0
    }

    /// Total number of samples received.
    pub fn sample_count(&self) -> u64 {
        self.shared.samples.load(Ordering::Relaxed)
    }
}

/// Starts the HTTPS server and returns a read handle and the server task's JoinHandle.
///
/// Binding and certificate preparation complete inside this function (failures return Err
/// immediately). Aborting the returned JoinHandle stops the server (no graceful shutdown is
/// needed - the state lives only in the process).
pub async fn spawn(
    cfg: PhoneImuServerConfig,
) -> Result<(PhoneImuHandle, tokio::task::JoinHandle<()>), ServeError> {
    let pair = cert::load_or_generate(&cfg.cert_dir)?;

    // rustls 0.23 needs a process-wide CryptoProvider. Install ring explicitly (if one is
    // already installed elsewhere this just returns Err, which can be ignored).
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tls = RustlsConfig::from_pem(pair.cert_pem.into_bytes(), pair.key_pem.into_bytes())
        .await
        .map_err(ServeError::Tls)?;

    let shared = Arc::new(Shared {
        latest: std::sync::Mutex::new(None),
        active_conn: AtomicU64::new(0),
        next_conn: AtomicU64::new(0),
        samples: AtomicU64::new(0),
    });

    let mut app = Router::new()
        .route("/ws", get(ws_upgrade))
        .with_state(shared.clone());
    match &cfg.web_root {
        Some(root) if root.is_dir() => {
            info!("serving web root {}", root.display());
            app = app.fallback_service(ServeDir::new(root));
        }
        Some(root) => {
            warn!(
                "web_root {} does not exist; serving built-in placeholder page (point it at this repository's web/ directory)",
                root.display()
            );
            app = app.fallback(placeholder_page);
        }
        None => {
            info!("no web_root configured; serving built-in placeholder page");
            app = app.fallback(placeholder_page);
        }
    }

    // Bind a std listener first so errors (such as a port clash) surface here.
    let listener = std::net::TcpListener::bind(cfg.listen).map_err(|source| ServeError::Bind {
        addr: cfg.listen,
        source,
    })?;
    listener
        .set_nonblocking(true)
        .map_err(|source| ServeError::Bind {
            addr: cfg.listen,
            source,
        })?;

    let shown_ip = cert::primary_local_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    info!(
        "phone IMU server listening on https://{}:{}/ (open this on the phone, same WiFi)",
        shown_ip,
        cfg.listen.port()
    );

    // Since axum-server 0.8, from_tcp_rustls returns an io::Result (the std listener is
    // made non-blocking here). Hiding it inside the spawned task would reduce a failure to
    // a warning in the log, so it is returned to the caller as a Bind error instead.
    let server =
        axum_server::from_tcp_rustls(listener, tls).map_err(|source| ServeError::Bind {
            addr: cfg.listen,
            source,
        })?;

    let handle = PhoneImuHandle { shared };
    let task = tokio::spawn(async move {
        if let Err(e) = server.serve(app.into_make_service()).await {
            warn!("phone IMU server exited: {e}");
        }
    });
    Ok((handle, task))
}

/// Guidance page shown when web_root is unset or missing. It still lets the certificate
/// exception be granted ahead of time.
async fn placeholder_page() -> impl IntoResponse {
    Html(
        "<!doctype html><meta charset=\"utf-8\">\
         <title>phone-imu</title>\
         <body style=\"font-family:sans-serif;background:#111;color:#eee;padding:2rem\">\
         <h1>phone-imu server</h1>\
         <p>The WebSocket endpoint <code>/ws</code> is up, but no web UI is deployed.</p>\
         <p>Start the server with <code>--web-root web</code> (the <code>web/</code> directory of \
         this repository; plain HTML/ESM - no build step).</p></body>",
    )
}

async fn ws_upgrade(State(shared): State<Arc<Shared>>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| ws_session(shared, socket))
}

/// Session loop of one WS connection. The newest connection always wins: when a newer
/// generation than our own appears, send a status message and leave.
async fn ws_session(shared: Arc<Shared>, mut socket: WebSocket) {
    let conn_id = shared.next_conn.fetch_add(1, Ordering::Relaxed) + 1;
    let prev = shared.active_conn.swap(conn_id, Ordering::Relaxed);
    if prev != 0 {
        info!("phone connected (conn #{conn_id}, superseding #{prev})");
    } else {
        info!("phone connected (conn #{conn_id})");
    }

    loop {
        // If superseded by a newer connection, notify and leave.
        if shared.active_conn.load(Ordering::Relaxed) != conn_id {
            let bye = ServerMessage::Status {
                connected: false,
                message: "superseded by a newer connection".to_string(),
            };
            if let Ok(json) = serde_json::to_string(&bye) {
                let _ = socket.send(Message::Text(json.into())).await;
            }
            let _ = socket.send(Message::Close(None)).await;
            break;
        }

        match socket.recv().await {
            Some(Ok(Message::Binary(buf))) => match parse_sample_frame(&buf) {
                Ok(sample) => store_sample(&shared, conn_id, sample),
                Err(e) => debug!("dropping bad sample frame: {e}"),
            },
            Some(Ok(Message::Text(text))) => handle_text(text.as_str()),
            Some(Ok(Message::Close(_))) | None => break,
            Some(Ok(_)) => {} // ws-level Ping/Pong is answered by axum
            Some(Err(e)) => {
                debug!("ws receive error (conn #{conn_id}): {e}");
                break;
            }
        }
    }

    // If we are still the active connection, return to "no connection" (do not touch it if
    // we were superseded).
    let _ = shared
        .active_conn
        .compare_exchange(conn_id, 0, Ordering::Relaxed, Ordering::Relaxed);
    info!("phone disconnected (conn #{conn_id})");
}

fn store_sample(shared: &Shared, conn_id: u64, sample: PhoneImuSample) {
    let stamped = StampedSample {
        received: Instant::now(),
        connection_id: conn_id,
        sample,
    };
    *shared.latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(stamped);
    let _ = shared.samples.fetch_add(1, Ordering::Relaxed);
}

fn handle_text(text: &str) {
    match serde_json::from_str::<ClientMessage>(text) {
        Ok(ClientMessage::Hello { platform, ua }) => {
            info!(
                "phone hello: platform={} ua={}",
                platform.as_deref().unwrap_or("?"),
                ua.as_deref().unwrap_or("?")
            );
        }
        Err(e) => debug!("ignoring unknown text message: {e}"),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, unused_results)]
mod tests {
    use super::*;

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            latest: std::sync::Mutex::new(None),
            active_conn: AtomicU64::new(0),
            next_conn: AtomicU64::new(0),
            samples: AtomicU64::new(0),
        })
    }

    fn sample(t: f64) -> PhoneImuSample {
        PhoneImuSample {
            t_client_ms: t,
            gyro_rad_s: [0.0, 0.0, 0.0],
            accel_m_s2: [0.0, 0.0, 9.81],
            quat_wxyz: [1.0, 0.0, 0.0, 0.0],
        }
    }

    /// A scratch directory of our own, so a parallel test run cannot collide.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "phone-imu-test-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_fresh_handle_reports_no_connection_and_no_samples() {
        let h = PhoneImuHandle { shared: shared() };
        assert!(h.latest().is_none());
        assert!(!h.connected());
        assert_eq!(h.sample_count(), 0);
    }

    #[test]
    fn samples_are_latest_wins_and_carry_their_connection_generation() {
        // The control loop reads at 100Hz while the phone sends far faster; queueing would
        // hand the robot stale attitude, so the newest sample must simply overwrite.
        let s = shared();
        store_sample(&s, 3, sample(1.0));
        store_sample(&s, 3, sample(2.0));
        let h = PhoneImuHandle {
            shared: Arc::clone(&s),
        };
        assert_eq!(h.sample_count(), 2);
        let latest = h.latest().unwrap();
        assert!((latest.sample.t_client_ms - 2.0).abs() < f64::EPSILON);
        assert_eq!(latest.connection_id, 3);
    }

    #[test]
    fn connected_follows_the_active_connection_slot() {
        let s = shared();
        let h = PhoneImuHandle {
            shared: Arc::clone(&s),
        };
        assert!(!h.connected());
        s.active_conn.store(1, Ordering::Relaxed);
        assert!(h.connected());
        s.active_conn.store(0, Ordering::Relaxed);
        assert!(!h.connected());
    }

    #[test]
    fn hello_and_unparsable_text_leave_the_state_untouched() {
        let s = shared();
        handle_text(r#"{"type":"hello","platform":"iPhone","ua":"Safari"}"#);
        // `platform` / `ua` are optional.
        handle_text(r#"{"type":"hello"}"#);
        // Not JSON at all, and JSON of an unknown shape.
        handle_text("not json");
        handle_text(r#"{"type":"whatever"}"#);
        let h = PhoneImuHandle {
            shared: Arc::clone(&s),
        };
        assert_eq!(h.sample_count(), 0);
    }

    #[tokio::test]
    async fn the_placeholder_page_names_the_websocket_endpoint() {
        // It is what the phone sees before `web/` is wired up, so it has to say where to look.
        let body = placeholder_page().await.into_response();
        assert_eq!(body.status(), 200);
    }

    #[tokio::test]
    async fn spawn_binds_generates_a_certificate_and_serves() {
        let dir = scratch("spawn");
        let (handle, task) = spawn(PhoneImuServerConfig {
            listen: "127.0.0.1:0".parse().unwrap(),
            web_root: None,
            cert_dir: dir.clone(),
        })
        .await
        .unwrap();
        // The cert is written on first run and reused afterwards (delete the dir to rotate).
        assert!(dir.join("phone-imu.crt").is_file());
        assert!(dir.join("phone-imu.key").is_file());
        assert!(!handle.connected());
        assert_eq!(handle.sample_count(), 0);
        task.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_web_root_that_exists_is_served_and_a_missing_one_falls_back() {
        // Both branches must start: a mistyped `web_root` should still leave the WS usable,
        // because that is how the phone grants the certificate exception in the first place.
        let dir = scratch("webroot");
        std::fs::create_dir_all(dir.join("web")).unwrap();
        for root in [Some(dir.join("web")), Some(dir.join("nope"))] {
            let (_h, task) = spawn(PhoneImuServerConfig {
                listen: "127.0.0.1:0".parse().unwrap(),
                web_root: root,
                cert_dir: dir.clone(),
            })
            .await
            .unwrap();
            task.abort();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_port_that_is_already_taken_is_reported_as_a_bind_error() {
        // Surfacing this is the whole reason the listener is bound before the task spawns —
        // otherwise a port clash would only ever appear as a warning in the log.
        let dir = scratch("bind");
        let squatter = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = squatter.local_addr().unwrap();
        let err = spawn(PhoneImuServerConfig {
            listen: addr,
            web_root: None,
            cert_dir: dir.clone(),
        })
        .await
        .unwrap_err();
        assert!(matches!(err, ServeError::Bind { .. }), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
