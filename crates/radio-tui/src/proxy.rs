use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::StreamExt;
use reqwest::Client;
use tokio::sync::{broadcast, Mutex};
use tracing::{debug, info, warn};

use radio_proto::state::StateManager;

pub const PROXY_PORT: u16 = 8990;
pub const PROXY_HOST: &str = "127.0.0.1";
const PROXY_BROADCAST_CAPACITY: usize = 4096;

#[derive(Clone)]
pub struct ProxyState {
    state_manager: Arc<StateManager>,
    client: Client,
    streams: Arc<Mutex<HashMap<usize, Arc<SharedStream>>>>,
}

struct SharedStream {
    headers: reqwest::header::HeaderMap,
    tx: broadcast::Sender<Bytes>,
}

impl ProxyState {
    pub fn new(state_manager: Arc<StateManager>) -> Self {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .default_headers({
                let mut h = reqwest::header::HeaderMap::new();
                h.insert(
                    "Icy-MetaData",
                    reqwest::header::HeaderValue::from_static("1"),
                );
                h
            })
            .build()
            .expect("failed to build reqwest client for stream proxy");

        Self {
            state_manager,
            client,
            streams: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn station_url(&self, idx: usize) -> Option<String> {
        let state = self.state_manager.get_state().await;
        state.stations.get(idx).map(|s| s.url.clone())
    }

    async fn get_or_start_stream(&self, idx: usize) -> Result<Arc<SharedStream>, StatusCode> {
        if let Some(existing) = self.streams.lock().await.get(&idx).cloned() {
            return Ok(existing);
        }

        let url = self.station_url(idx).await.ok_or(StatusCode::NOT_FOUND)?;
        info!(
            "proxy: opening shared upstream for station {} → {}",
            idx, url
        );

        let upstream = self.client.get(&url).send().await.map_err(|e| {
            warn!("proxy: upstream connect failed for idx={}: {}", idx, e);
            StatusCode::BAD_GATEWAY
        })?;

        if !upstream.status().is_success() {
            warn!(
                "proxy: upstream returned {} for idx={}",
                upstream.status(),
                idx
            );
            return Err(StatusCode::BAD_GATEWAY);
        }

        let headers = upstream.headers().clone();
        let (tx, _rx) = broadcast::channel::<Bytes>(PROXY_BROADCAST_CAPACITY);
        let shared = Arc::new(SharedStream { headers, tx });

        self.streams.lock().await.insert(idx, shared.clone());

        let streams = self.streams.clone();
        let shared_for_task = shared.clone();
        tokio::spawn(async move {
            let mut bytes_stream = upstream.bytes_stream();
            let mut no_receivers_since: Option<Instant> = None;

            while let Some(next) = bytes_stream.next().await {
                let chunk = match next {
                    Ok(c) => c,
                    Err(e) => {
                        warn!("proxy: upstream read error idx={}: {}", idx, e);
                        break;
                    }
                };

                if shared_for_task.tx.receiver_count() == 0 {
                    if no_receivers_since
                        .get_or_insert_with(Instant::now)
                        .elapsed()
                        >= Duration::from_secs(2)
                    {
                        debug!("proxy: no subscribers for idx={}, closing upstream", idx);
                        break;
                    }
                    continue;
                }
                no_receivers_since = None;

                if shared_for_task.tx.send(chunk).is_err() {
                    continue;
                }
            }

            let mut map = streams.lock().await;
            if map
                .get(&idx)
                .map(|current| Arc::ptr_eq(current, &shared_for_task))
                .unwrap_or(false)
            {
                map.remove(&idx);
            }
            debug!("proxy: upstream pump exited for idx={}", idx);
        });

        Ok(shared)
    }
}

async fn stream_station(
    Path(idx): Path<usize>,
    State(state): State<ProxyState>,
) -> impl IntoResponse {
    let shared = match state.get_or_start_stream(idx).await {
        Ok(s) => s,
        Err(code) => {
            return Response::builder()
                .status(code)
                .body(Body::empty())
                .unwrap();
        }
    };

    let mut builder = Response::builder().status(200);
    for (name, value) in &shared.headers {
        let name_str = name.as_str();
        if name_str.starts_with("icy-")
            || name_str == "content-type"
            || name_str == "transfer-encoding"
        {
            if let Ok(hv) = axum::http::HeaderValue::from_bytes(value.as_bytes()) {
                builder = builder.header(name_str, hv);
            }
        }
    }

    let stream = futures_util::stream::unfold(shared.tx.subscribe(), |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(chunk) => return Some((Ok::<Bytes, std::io::Error>(chunk), rx)),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("proxy: subscriber lagged by {} chunks", n);
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    builder.body(Body::from_stream(stream)).unwrap()
}

pub fn start_server(
    state_manager: Arc<StateManager>,
) -> tokio::task::JoinHandle<()> {
    let proxy_state = ProxyState::new(state_manager);
    let app = Router::new()
        .route("/stream/:idx", get(stream_station))
        .with_state(proxy_state);

    tokio::spawn(async move {
        let addr = format!("{}:{}", PROXY_HOST, PROXY_PORT);
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

pub fn proxy_url(idx: usize) -> String {
    format!("http://{}:{}/stream/{}", PROXY_HOST, PROXY_PORT, idx)
}
