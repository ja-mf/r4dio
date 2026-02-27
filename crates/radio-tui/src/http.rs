use crate::core::DaemonEvent;
use radio_proto::protocol::Command;
use radio_proto::state::StateManager;
use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::get,
    Router,
};
use serde::Serialize;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::{error, info};

#[derive(Clone)]
struct HttpState {
    state_manager: Arc<StateManager>,
    event_tx: mpsc::Sender<DaemonEvent>,
}

#[derive(Serialize)]
struct ApiState {
    stations: Vec<StationInfo>,
    current_station: Option<usize>,
    volume: f32,
    is_playing: bool,
    icy_title: Option<String>,
}

#[derive(Serialize)]
struct StationInfo {
    idx: usize,
    name: String,
    description: String,
}

#[derive(Serialize)]
struct VolumeStatus {
    volume: u8,
}

pub fn start_server(
    bind_address: String,
    port: u16,
    state_manager: Arc<StateManager>,
    event_tx: mpsc::Sender<DaemonEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let app_state = HttpState { state_manager, event_tx };
        
        let app = Router::new()
            .route("/api/state", get(get_state))
            .route("/api/play/:idx", get(play_station).post(play_station))
            .route("/api/stop", get(stop).post(stop))
            .route("/api/next", get(next_station).post(next_station))
            .route("/api/prev", get(prev_station).post(prev_station))
            .route("/api/random", get(random_station).post(random_station))
            .route("/api/volume/:volume", get(set_volume).post(set_volume))
            .route("/api/volume", get(get_volume))
            .with_state(app_state);
        
        let addr = format!("{}:{}", bind_address, port);
        let listener = match TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                error!("Failed to bind HTTP server to {}: {}", addr, e);
                return;
            }
        };
        
        info!("HTTP API server listening on http://{}", addr);
        
        if let Err(e) = axum::serve(listener, app).await {
            error!("HTTP server error: {}", e);
        }
    })
}

async fn get_state(State(state): State<HttpState>) -> Result<Json<ApiState>, StatusCode> {
    let daemon_state = state.state_manager.get_state().await;
    
    let stations: Vec<StationInfo> = daemon_state
        .stations
        .iter()
        .enumerate()
        .map(|(idx, s)| StationInfo {
            idx,
            name: s.name.clone(),
            description: s.description.clone(),
        })
        .collect();
    
    let api_state = ApiState {
        stations,
        current_station: daemon_state.current_station,
        volume: daemon_state.volume,
        is_playing: daemon_state.is_playing,
        icy_title: daemon_state.icy_title,
    };
    
    Ok(Json(api_state))
}

async fn play_station(
    State(state): State<HttpState>,
    axum::extract::Path(idx): axum::extract::Path<usize>,
) -> StatusCode {
    info!("HTTP API: Play station {}", idx);
    let cmd = Command::Play { station_idx: idx };
    if state.event_tx.send(DaemonEvent::ClientCommand(cmd)).await.is_err() {
        error!("Failed to send play command");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    StatusCode::OK
}

async fn stop(State(state): State<HttpState>) -> StatusCode {
    info!("HTTP API: Stop");
    if state.event_tx.send(DaemonEvent::ClientCommand(Command::Stop)).await.is_err() {
        error!("Failed to send stop command");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    StatusCode::OK
}

async fn next_station(State(state): State<HttpState>) -> StatusCode {
    info!("HTTP API: Next station");
    if state.event_tx.send(DaemonEvent::ClientCommand(Command::Next)).await.is_err() {
        error!("Failed to send next command");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    StatusCode::OK
}

async fn prev_station(State(state): State<HttpState>) -> StatusCode {
    info!("HTTP API: Previous station");
    if state.event_tx.send(DaemonEvent::ClientCommand(Command::Prev)).await.is_err() {
        error!("Failed to send prev command");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    StatusCode::OK
}

async fn random_station(State(state): State<HttpState>) -> StatusCode {
    info!("HTTP API: Random station");
    if state.event_tx.send(DaemonEvent::ClientCommand(Command::Random)).await.is_err() {
        error!("Failed to send random command");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    StatusCode::OK
}

async fn set_volume(
    State(state): State<HttpState>,
    axum::extract::Path(volume): axum::extract::Path<i32>,
) -> StatusCode {
    let vol = (volume as f32 / 100.0).clamp(0.0, 1.0);
    info!("HTTP API: Set volume to {}%", volume);
    let cmd = Command::Volume { value: vol };
    if state.event_tx.send(DaemonEvent::ClientCommand(cmd)).await.is_err() {
        error!("Failed to send volume command");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    StatusCode::OK
}

async fn get_volume(State(state): State<HttpState>) -> Json<VolumeStatus> {
    let daemon_state = state.state_manager.get_state().await;
    let volume = (daemon_state.volume * 100.0).round() as u8;
    Json(VolumeStatus { volume })
}
