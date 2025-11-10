use anyhow::Result;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{net::UdpSocket, sync::RwLock};
use tracing::{error, info};

const PLAYER_TIMEOUT_SECS: u64 = 60;
const CLEANUP_INTERVAL_SECS: u64 = 30;
const LISTEN_PORT: u16 = 9000;

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type", rename_all = "PascalCase")]
enum ClientMsg {
    Register {
        session_id: String,
        player_id: String,
    },
    Heartbeat {
        session_id: String,
        player_id: String,
    },
    Leave {
        session_id: String,
        player_id: String,
    },
    ListPeers {
        session_id: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "PascalCase")]
enum ServerMsg {
    Registered,
    PeerList {
        peers: Vec<PeerInfo>,
    },
    PeerJoined {
        player_id: String,
        ip: String,
        port: u16,
    },
    PeerLeft {
        player_id: String,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct PeerInfo {
    player_id: String,
    ip: String,
    port: u16,
}

#[derive(Debug, Clone)]
struct Player {
    player_id: String,
    addr: SocketAddr,
    last_seen: Instant,
}

#[derive(Debug, Default)]
struct Session {
    players: DashMap<String, Player>,
}

#[derive(Debug, Default)]
struct CoordinatorState {
    sessions: DashMap<String, Arc<Session>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    tokio::spawn(async {
        use tokio::net::TcpListener;
        if let Ok(listener) = TcpListener::bind("0.0.0.0:8080").await {
            info!("Dummy TCP listener active on 0.0.0.0:8080");
            loop {
                let _ = listener.accept().await;
            }
        }
    });

    let bind_addr = format!("0.0.0.0:{}", LISTEN_PORT);
    let socket = Arc::new(UdpSocket::bind(&bind_addr).await?);
    info!("Coordinator listening on {}", bind_addr);

    let state = Arc::new(CoordinatorState::default());

    {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(CLEANUP_INTERVAL_SECS)).await;
                run_cleanup(&state).await;
            }
        });
    }

    let mut buf = vec![0u8; 2048];

    loop {
        match socket.recv_from(&mut buf).await {
            Ok((len, addr)) => {
                let data = &buf[..len];
                let s = match std::str::from_utf8(data) {
                    Ok(s) => s.to_string(),
                    Err(_) => {
                        error!("Non-utf8 packet from {}", addr);
                        continue;
                    }
                };

                let socket = socket.clone();
                let state = state.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_packet(&socket, &state, &s, addr).await {
                        error!("Error handling packet from {}: {:?}", addr, e);
                    }
                });
            }
            Err(e) => {
                error!("recv_from error: {:?}", e);
            }
        }
    }
}

async fn handle_packet(
    socket: &UdpSocket,
    state: &Arc<CoordinatorState>,
    raw: &str,
    sender: SocketAddr,
) -> Result<()> {
    let parsed: serde_json::Result<ClientMsg> = serde_json::from_str(raw);
    match parsed {
        Ok(ClientMsg::Register {
            session_id,
            player_id,
        }) => {
            handle_register(socket, state, &session_id, &player_id, sender).await?;
        }
        Ok(ClientMsg::Heartbeat {
            session_id,
            player_id,
        }) => {
            touch_player(state, &session_id, &player_id, sender).await;
        }
        Ok(ClientMsg::Leave {
            session_id,
            player_id,
        }) => {
            handle_leave(socket, state, &session_id, &player_id).await?;
        }
        Ok(ClientMsg::ListPeers { session_id }) => {
            handle_list_peers(socket, state, &session_id, sender).await?;
        }
        Err(e) => {
            error!(
                "Invalid client message from {}: {} -- raw: {}",
                sender, e, raw
            );
            let reply = ServerMsg::Error {
                message: format!("Invalid message: {}", e),
            };
            send_json(socket, &reply, sender).await?;
        }
    }
    Ok(())
}

async fn handle_register(
    socket: &UdpSocket,
    state: &Arc<CoordinatorState>,
    session_id: &str,
    player_id: &str,
    sender: SocketAddr,
) -> Result<()> {
    let session = state
        .sessions
        .entry(session_id.to_string())
        .or_insert_with(|| Arc::new(Session::default()))
        .value()
        .clone();

    let player = Player {
        player_id: player_id.to_string(),
        addr: sender,
        last_seen: Instant::now(),
    };

    session
        .players
        .insert(player_id.to_string(), player.clone());
    info!("Register: {} in {} from {}", player_id, session_id, sender);

    send_json(socket, &ServerMsg::Registered, sender).await?;

    let peers: Vec<PeerInfo> = session
        .players
        .iter()
        .filter_map(|kv| {
            let p = kv.value();
            if p.player_id == player_id {
                None
            } else {
                Some(PeerInfo {
                    player_id: p.player_id.clone(),
                    ip: p.addr.ip().to_string(),
                    port: p.addr.port(),
                })
            }
        })
        .collect();

    send_json(
        socket,
        &ServerMsg::PeerList {
            peers: peers.clone(),
        },
        sender,
    )
    .await?;

    for kv in session.players.iter() {
        let p = kv.value();
        if p.player_id != player_id {
            let notify = ServerMsg::PeerJoined {
                player_id: player_id.to_string(),
                ip: sender.ip().to_string(),
                port: sender.port(),
            };
            let _ = send_json(socket, &notify, p.addr).await;
        }
    }
    Ok(())
}

async fn touch_player(
    state: &Arc<CoordinatorState>,
    session_id: &str,
    player_id: &str,
    sender: SocketAddr,
) {
    if let Some(session_arc) = state.sessions.get(session_id) {
        let session = session_arc.value();
        if let Some(mut entry) = session.players.get_mut(player_id) {
            entry.addr = sender;
            entry.last_seen = Instant::now();
            info!("Heartbeat from {}@{}", player_id, sender);
            return;
        } else {
            session.players.insert(
                player_id.to_string(),
                Player {
                    player_id: player_id.to_string(),
                    addr: sender,
                    last_seen: Instant::now(),
                },
            );
            info!(
                "Heartbeat created registration for {} in {}",
                player_id, session_id
            );
        }
    } else {
        let new_session = Arc::new(Session::default());
        new_session.players.insert(
            player_id.to_string(),
            Player {
                player_id: player_id.to_string(),
                addr: sender,
                last_seen: Instant::now(),
            },
        );
        state.sessions.insert(session_id.to_string(), new_session);
        info!(
            "Heartbeat created session {} and player {}",
            session_id, player_id
        );
    }
}

async fn handle_leave(
    socket: &UdpSocket,
    state: &Arc<CoordinatorState>,
    session_id: &str,
    player_id: &str,
) -> Result<()> {
    if let Some(session_arc) = state.sessions.get(session_id) {
        let session = session_arc.value();
        if session.players.remove(player_id).is_some() {
            info!("Player {} left session {}", player_id, session_id);
            for kv in session.players.iter() {
                let p = kv.value();
                let notify = ServerMsg::PeerLeft {
                    player_id: player_id.to_string(),
                };
                let _ = send_json(socket, &notify, p.addr).await;
            }
        }

        if session.players.is_empty() {
            state.sessions.remove(session_id);
            info!("Session {} removed (empty)", session_id);
        }
    }

    send_json(
        socket,
        &ServerMsg::Registered,
        /* ack */
        "0.0.0.0:0"
            .parse()
            .unwrap_or_else(|_| "127.0.0.1:0".parse().unwrap()),
    )
    .await
    .ok();
    Ok(())
}

async fn handle_list_peers(
    socket: &UdpSocket,
    state: &Arc<CoordinatorState>,
    session_id: &str,
    sender: SocketAddr,
) -> Result<()> {
    if let Some(session_arc) = state.sessions.get(session_id) {
        let session = session_arc.value();
        let peers: Vec<PeerInfo> = session
            .players
            .iter()
            .map(|kv| {
                let p = kv.value();
                PeerInfo {
                    player_id: p.player_id.clone(),
                    ip: p.addr.ip().to_string(),
                    port: p.addr.port(),
                }
            })
            .collect();
        send_json(socket, &ServerMsg::PeerList { peers }, sender).await?;
    } else {
        send_json(
            socket,
            &ServerMsg::Error {
                message: "Unknown session".into(),
            },
            sender,
        )
        .await?;
    }
    Ok(())
}

async fn run_cleanup(state: &Arc<CoordinatorState>) {
    let now = Instant::now();
    let timeout = Duration::from_secs(PLAYER_TIMEOUT_SECS);

    let mut empty_sessions: Vec<String> = Vec::new();

    for entry in state.sessions.iter() {
        let session_id = entry.key().clone();
        let session = entry.value();

        // collect players to remove
        let mut to_remove: Vec<String> = Vec::new();
        for kv in session.players.iter() {
            let p = kv.value();
            if now.duration_since(p.last_seen) > timeout {
                to_remove.push(p.player_id.clone());
            }
        }

        for pid in to_remove.iter() {
            session.players.remove(pid);
            info!("Timed out {} in session {}", pid, session_id);
        }

        if session.players.is_empty() {
            empty_sessions.push(session_id.clone());
        }
    }

    for sid in empty_sessions {
        state.sessions.remove(&sid);
        info!("Removed empty session {}", sid);
    }
}

async fn send_json<T: Serialize>(socket: &UdpSocket, message: &T, addr: SocketAddr) -> Result<()> {
    let b = serde_json::to_vec(message)?;
    let _ = socket.send_to(&b, &addr).await?;
    Ok(())
}
