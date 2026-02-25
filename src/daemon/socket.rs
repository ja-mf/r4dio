use crate::BroadcastMessage;
use radio_tui::shared::protocol::{Broadcast, Command, Message};
use radio_tui::shared::state::StateManager;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{error, info, warn};

pub struct ClientHandle {
    pub id: usize,
}

pub fn start_server(
    bind_address: String,
    port: u16,
    state_manager: Arc<StateManager>,
    clients: Arc<RwLock<Vec<ClientHandle>>>,
    command_tx: mpsc::Sender<Command>,
    broadcast_tx: broadcast::Sender<BroadcastMessage>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let addr = format!("{}:{}", bind_address, port);
        
        let listener = match TcpListener::bind(&addr).await {
            Ok(l) => l,
            Err(e) => {
                error!("Failed to bind TCP socket {}: {}", addr, e);
                return;
            }
        };

        info!("TCP server listening at {}", addr);

        let mut client_id = 0usize;

        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    client_id += 1;
                    let id = client_id;

                    {
                        let mut guard = clients.write().await;
                        guard.push(ClientHandle { id });
                    }

                    info!("Client {} connected from {}", id, peer);

                    let sm = state_manager.clone();
                    let cmd_tx = command_tx.clone();
                    let bcast_rx = broadcast_tx.subscribe();
                    let clients_ref = clients.clone();

                    tokio::spawn(async move {
                        handle_client(stream, sm, id, cmd_tx, bcast_rx).await;

                        let mut guard = clients_ref.write().await;
                        guard.retain(|c| c.id != id);
                        info!("Client {} disconnected", id);
                    });
                }
                Err(e) => {
                    error!("Failed to accept connection: {}", e);
                }
            }
        }
    })
}

async fn handle_client(
    stream: TcpStream,
    state_manager: Arc<StateManager>,
    client_id: usize,
    command_tx: mpsc::Sender<Command>,
    mut broadcast_rx: broadcast::Receiver<BroadcastMessage>,
) {
    let (mut read_half, mut write_half) = stream.into_split();
    let mut tmp = [0u8; 4096];
    let mut read_buf: Vec<u8> = Vec::new();

    if let Ok(encoded) = encode_state(&state_manager).await {
        if write_half.write_all(&encoded).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            result = read_half.read(&mut tmp) => {
                match result {
                    Ok(0) => {
                        info!("Client {} closed connection", client_id);
                        break;
                    }
                    Ok(n) => {
                        read_buf.extend_from_slice(&tmp[..n]);

                        loop {
                            if read_buf.len() < 4 { break; }
                            match Message::decode(&read_buf) {
                                Ok((Message::Command(cmd), consumed)) => {
                                    read_buf.drain(..consumed);
                                    info!("Client {} sent command: {:?}", client_id, cmd);

                                    if command_tx.send(cmd).await.is_err() {
                                        warn!("Command channel closed");
                                        return;
                                    }

                                    if let Ok(encoded) = encode_state(&state_manager).await {
                                        if write_half.write_all(&encoded).await.is_err() {
                                            return;
                                        }
                                    }
                                }
                                Ok((_, consumed)) => {
                                    read_buf.drain(..consumed);
                                }
                                Err(_) => break,
                            }
                        }
                    }
                    Err(e) => {
                        error!("Read error from client {}: {}", client_id, e);
                        break;
                    }
                }
            }

            msg = broadcast_rx.recv() => {
                match msg {
                    Ok(BroadcastMessage::StateUpdated) => {
                        if let Ok(encoded) = encode_state(&state_manager).await {
                            if write_half.write_all(&encoded).await.is_err() {
                                break;
                            }
                        }
                    }
                    Ok(BroadcastMessage::IcyUpdated(title)) => {
                        let broadcast = Broadcast::Icy { title };
                        if let Ok(encoded) = Message::Broadcast(broadcast).encode() {
                            if write_half.write_all(&encoded).await.is_err() {
                                break;
                            }
                        }
                        if let Ok(encoded) = encode_state(&state_manager).await {
                            let _ = write_half.write_all(&encoded).await;
                        }
                    }
                    Ok(BroadcastMessage::Log(message)) => {
                        let broadcast = Broadcast::Log { message };
                        if let Ok(encoded) = Message::Broadcast(broadcast).encode() {
                            let _ = write_half.write_all(&encoded).await;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Client {} missed {} broadcast messages", client_id, n);
                        if let Ok(encoded) = encode_state(&state_manager).await {
                            let _ = write_half.write_all(&encoded).await;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

async fn encode_state(state_manager: &StateManager) -> anyhow::Result<Vec<u8>> {
    let state = state_manager.get_state().await;
    Message::Broadcast(Broadcast::State { data: state }).encode()
}
