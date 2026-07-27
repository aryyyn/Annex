//! Serves the client page, runs signalling, and owns one peer connection per
//! client.
//!
//! Platform neutral by design. The native client in `apps/client-native` reuses
//! this crate unchanged, which is why nothing Apple specific may leak in here.
//!
//! # Being LAN-only buys a lot
//!
//! ICE is configured with no STUN and no TURN servers, so only host candidates
//! are gathered and the two machines connect directly. That removes the entire
//! NAT traversal problem, every external dependency, and the usual multi-second
//! connection setup. It is the single largest simplification in the design.
//!
//! # One port, two protocols
//!
//! `GET /` serves the client page, `GET /signal` upgrades to a WebSocket. Both
//! on the same port, so the user has one URL to type and one number to put in a
//! QR code.
//!
//! # Back pressure
//!
//! [`Server::broadcast`] is fed from a bounded channel. When that channel
//! fills, the correct response is to **drop the oldest frame** and ask the
//! encoder for a keyframe. Queueing instead trades a brief glitch for
//! permanently growing latency, which is far worse to use. See
//! [`Server::stats`] for whether that is happening.

pub mod session;
pub mod signaling;

pub use session::{RtcConfig, Session};

use annex_core::EncodedSample;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};

#[derive(Debug)]
pub enum RtcError {
    Bind(std::io::Error),
    /// A client failed or omitted the shared-secret handshake.
    Unauthorised,
    Negotiation(String),
    PeerClosed,
}

impl std::fmt::Display for RtcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bind(e) => write!(f, "could not bind: {e}"),
            Self::Unauthorised => write!(f, "client failed the token handshake"),
            Self::Negotiation(m) => write!(f, "negotiation failed: {m}"),
            Self::PeerClosed => write!(f, "peer connection closed"),
        }
    }
}

impl std::error::Error for RtcError {}

#[derive(Debug, Default)]
pub struct Stats {
    pub clients_connected: AtomicU64,
    pub clients_now: AtomicU64,
    pub samples_sent: AtomicU64,
    pub samples_dropped: AtomicU64,
    /// Set when a client's decoder loses sync and asks for a fresh IDR. Wire
    /// this to `Encoder::request_keyframe`.
    pub keyframe_requests: AtomicU64,
}

/// Shared state every WebSocket connection needs.
pub struct AppState {
    pub cfg: RtcConfig,
    /// Fan-out to every connected client. A broadcast channel is right here:
    /// each session gets its own receiver, and a slow one lags rather than
    /// blocking the encoder.
    pub frames: broadcast::Sender<Arc<EncodedSample>>,
    pub stats: Arc<Stats>,
    /// Set when any client asks for a keyframe.
    pub want_keyframe: Arc<std::sync::atomic::AtomicBool>,
    pub sessions: Arc<Mutex<Vec<Arc<Session>>>>,
}

/// The HTTP and WebSocket server.
pub struct Server {
    state: Arc<AppState>,
    handle: tokio::task::JoinHandle<()>,
    bound: SocketAddr,
}

impl Server {
    /// Binds and starts serving. Returns once the listener is live, so the
    /// caller can print a URL that already works.
    pub async fn bind(cfg: RtcConfig) -> Result<Self, RtcError> {
        // Capacity is deliberately small. If a client cannot keep up it should
        // lag and recover with a keyframe, not accumulate a backlog of stale
        // frames that arrive late and useless.
        let (frames, _) = broadcast::channel(8);
        let state = Arc::new(AppState {
            cfg: cfg.clone(),
            frames,
            stats: Arc::new(Stats::default()),
            want_keyframe: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            sessions: Arc::new(Mutex::new(Vec::new())),
        });

        let listener = tokio::net::TcpListener::bind(cfg.bind_addr)
            .await
            .map_err(RtcError::Bind)?;
        let bound = listener.local_addr().map_err(RtcError::Bind)?;

        let app = signaling::router(Arc::clone(&state));
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Ok(Self {
            state,
            handle,
            bound,
        })
    }

    /// Hands one encoded sample to every connected client.
    ///
    /// Never blocks and never fails: with no receivers the send is simply
    /// dropped, which is the correct behaviour when nobody is watching.
    pub fn broadcast(&self, sample: EncodedSample) {
        match self.state.frames.send(Arc::new(sample)) {
            Ok(n) => {
                self.state
                    .stats
                    .samples_sent
                    .fetch_add(n as u64, Ordering::Relaxed);
            }
            Err(_) => {
                // No subscribers. Not an error: nobody is connected yet.
            }
        }
    }

    /// Whether a client has asked for a keyframe since the last check.
    ///
    /// Clearing on read makes this a one-shot: the caller forces exactly one
    /// IDR per request rather than every frame until it happens to notice.
    pub fn take_keyframe_request(&self) -> bool {
        self.state.want_keyframe.swap(false, Ordering::SeqCst)
    }

    pub fn stats(&self) -> &Stats {
        &self.state.stats
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.bound
    }

    /// The URL to show in the tray menu and encode into the QR code.
    pub fn connect_url(&self) -> String {
        let ip = lan_ip().unwrap_or(IpAddr::from([127, 0, 0, 1]));
        match &self.state.cfg.auth_token {
            Some(t) => format!("http://{}:{}/?token={}", ip, self.bound.port(), t),
            None => format!("http://{}:{}/", ip, self.bound.port()),
        }
    }

    pub async fn shutdown(self) {
        for s in self.state.sessions.lock().await.iter() {
            s.close().await;
        }
        self.handle.abort();
    }
}

/// This machine's address on the LAN.
///
/// There is no portable "which interface reaches the network" call, so this
/// uses the standard trick: open a UDP socket toward an address on the local
/// network and ask what source address the kernel picked. UDP is
/// connectionless, so nothing is actually sent and the target need not exist.
pub fn lan_ip() -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.168.1.1:80")
        .or_else(|_| sock.connect("8.8.8.8:80"))
        .ok()?;
    sock.local_addr().ok().map(|a| a.ip())
}
