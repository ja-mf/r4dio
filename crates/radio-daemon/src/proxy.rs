/// HTTP stream proxy for radio stations.
///
/// Serves `GET /stream/:idx` on a local port (default 8990).  When mpv wants
/// to play station N it is directed to `http://127.0.0.1:8990/stream/N`.
/// This handler opens **one** upstream HTTP connection and streams the bytes
/// straight through to mpv, forwarding all response headers (Content-Type,
/// ICY-*, Transfer-Encoding, etc.) so mpv sees the stream exactly as if it
/// had connected directly.
///
/// Design notes
/// ─────────────
/// • Each GET /stream/:idx opens a fresh upstream connection — no shared
///   broadcast channel yet.  That keeps failure handling simple: if mpv drops
///   its connection, the upstream fetch is cancelled.  Shared-broadcast for
///   scope-tui can be layered on top later.
/// • ICY metadata is preserved because we forward the raw response headers
///   (including Icy-MetaInt) and the body byte-for-byte.
/// • The proxy re-uses a global `reqwest::Client` so TLS sessions are shared.
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::StreamExt;
use reqwest::Client;
use tokio::sync::RwLock;
use tokio_util::io::ReaderStream;
use tracing::{info, warn};

use radio_proto::protocol::DaemonState;

// ── Shared state ──────────────────────────────────────────────────────────────

/// Proxy server state — holds a reference to the shared daemon state (to read
/// station URLs) plus a persistent HTTP client.
#[derive(Clone)]
pub struct ProxyState {
    pub daemon_state: Arc<RwLock<DaemonState>>,
    pub client: Client,
}

impl ProxyState {
    pub fn new(daemon_state: Arc<RwLock<DaemonState>>) -> Self {
        let client = Client::builder()
            // Follow redirects (common for HLS playlists and Icecast streams)
            .redirect(reqwest::redirect::Policy::limited(10))
            // Send ICY metadata request header — many Icecast servers require this
            .default_headers({
                let mut h = reqwest::header::HeaderMap::new();
                h.insert(
                    "Icy-MetaData",
                    reqwest::header::HeaderValue::from_static("1"),
                );
                h
            })
            .build()
            .expect("failed to build reqwest client for proxy");

        Self {
            daemon_state,
            client,
        }
    }
}

// ── Route handler ─────────────────────────────────────────────────────────────

async fn stream_station(
    Path(idx): Path<usize>,
    State(state): State<ProxyState>,
) -> impl IntoResponse {
    // Resolve station URL from shared daemon state
    let url = {
        let ds = state.daemon_state.read().await;
        match ds.stations.get(idx) {
            Some(s) => s.url.clone(),
            None => {
                warn!(
                    "proxy: station index {} not found (have {} stations)",
                    idx,
                    ds.stations.len()
                );
                return Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::empty())
                    .unwrap();
            }
        }
    };

    info!("proxy: opening upstream for station {} → {}", idx, url);

    // Open upstream connection
    let upstream = match state.client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => {
            warn!("proxy: upstream connect failed for idx={}: {}", idx, e);
            return Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .body(Body::empty())
                .unwrap();
        }
    };

    let upstream_status = upstream.status();
    if !upstream_status.is_success() {
        warn!(
            "proxy: upstream returned {} for idx={}",
            upstream_status, idx
        );
        return Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .body(Body::empty())
            .unwrap();
    }

    // Forward relevant headers to mpv
    let mut builder = Response::builder().status(200);
    for (name, value) in upstream.headers() {
        let name_str = name.as_str();
        // Forward content-type and all ICY headers; skip hop-by-hop headers
        if name_str.starts_with("icy-")
            || name_str == "content-type"
            || name_str == "transfer-encoding"
        {
            if let Ok(hv) = axum::http::HeaderValue::from_bytes(value.as_bytes()) {
                builder = builder.header(name_str, hv);
            }
        }
    }

    // Stream bytes from upstream directly to mpv
    let byte_stream = upstream.bytes_stream();
    let reader = tokio_util::io::StreamReader::new(
        byte_stream
            .map(|result| result.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))),
    );
    let axum_stream = ReaderStream::new(reader);
    let body = Body::from_stream(axum_stream);

    builder.body(body).unwrap()
}

// ── Server startup ────────────────────────────────────────────────────────────

pub const PROXY_PORT: u16 = 8990;

pub fn start_server(
    bind_address: String,
    port: u16,
    daemon_state: Arc<RwLock<DaemonState>>,
) -> tokio::task::JoinHandle<()> {
    let proxy_state = ProxyState::new(daemon_state);

    let app = Router::new()
        .route("/stream/{idx}", get(stream_station))
        .with_state(proxy_state);

    tokio::spawn(async move {
        let addr = format!("{}:{}", bind_address, port);
        info!("Stream proxy listening on http://{}", addr);
        let listener = match tokio::net::TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                warn!("Failed to bind stream proxy on {}: {}", addr, e);
                return;
            }
        };
        if let Err(e) = axum::serve(listener, app).await {
            warn!("Stream proxy error: {}", e);
        }
    })
}

/// Returns the local proxy URL for a given station index.
pub fn proxy_url(bind_address: &str, port: u16, idx: usize) -> String {
    format!("http://{}:{}/stream/{}", bind_address, port, idx)
}
