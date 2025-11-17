mod types;

use crate::types::{
    AppState, CreateLobbyResponse, JoinLobbyResponse, Lobby, LobbyRequest, SignalingMessage,
};
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use serde_json;
use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;
use warp::{
    http::StatusCode,
    ws::{Message as WsMessage, WebSocket},
    Filter, Reply,
};

#[tokio::main]
async fn main() {
    let app_state = AppState {
        lobbies: Arc::new(DashMap::new()),
    };

    let state_filter = warp::any().map(move || app_state.clone());

    // POST /create_lobby
    let create = warp::path("create_lobby")
        .and(warp::post())
        .and(state_filter.clone())
        .and_then(create_lobby);

    // POST /join_lobby
    let join = warp::path("join_lobby")
        .and(warp::post())
        .and(state_filter.clone())
        .and(warp::body::json())
        .and_then(join_lobby);

    // POST /register_host
    let register = warp::path("register_host")
        .and(warp::post())
        .and(state_filter.clone())
        .and(warp::body::json())
        .and_then(register_host);

    // POST /request_offer
    let request_offer_route = warp::path("request_offer")
        .and(warp::post())
        .and(state_filter.clone())
        .and(warp::body::json())
        .and_then(request_offer);

    // POST /send_offer
    let send_offer_route = warp::path("send_offer")
        .and(warp::post())
        .and(state_filter.clone())
        .and(warp::body::json())
        .and_then(send_offer);

    // POST /send_answer
    let send_answer_route = warp::path("send_answer")
        .and(warp::post())
        .and(state_filter.clone())
        .and(warp::body::json())
        .and_then(send_answer);

    // POST /send_ice
    let send_ice_route = warp::path("send_ice")
        .and(warp::post())
        .and(state_filter.clone())
        .and(warp::body::json())
        .and_then(send_ice);

    // WebSocket /ws/{lobby_code}/{peer_id}
    let ws_route = warp::path("ws")
        .and(warp::path::param::<String>())
        .and(warp::path::param::<String>())
        .and(warp::ws())
        .and(state_filter.clone())
        .and_then(
            |lobby_code: String, peer_id: String, ws: warp::ws::Ws, state: AppState| async move {
                Ok::<_, warp::Rejection>(
                    ws.on_upgrade(move |socket| handle_socket(socket, lobby_code, peer_id, state)),
                )
            },
        );

    // POST /request_player_list
    let request_player_list_route = warp::path("request_player_list")
        .and(warp::post())
        .and(state_filter.clone())
        .and(warp::body::json())
        .and_then(request_player_list);

    // POST /send_player_list
    let send_player_list_route = warp::path("send_player_list")
        .and(warp::post())
        .and(state_filter.clone())
        .and(warp::body::json())
        .and_then(send_player_list);

    let routes = create
        .or(join)
        .or(register)
        .or(request_offer_route)
        .or(send_offer_route)
        .or(send_answer_route)
        .or(send_ice_route)
        .or(request_player_list_route)
        .or(send_player_list_route)
        .or(ws_route)
        .with(warp::cors().allow_any_origin());

    println!("Coordinator server running on 127.0.0.1:3000");
    warp::serve(routes).run(([127, 0, 0, 1], 3000)).await;
}

async fn create_lobby(state: AppState) -> Result<impl Reply, warp::Rejection> {
    let lobby_code = generate_code();
    let local_id = Uuid::new_v4().to_string();
    println!("Creating lobby with code: {}", lobby_code);

    let lobby = Lobby {
        host_id: local_id.clone(),
        peers: DashMap::new(),
    };
    state.lobbies.insert(lobby_code.clone(), lobby);

    Ok(warp::reply::with_status(
        warp::reply::json(&CreateLobbyResponse {
            local_id,
            lobby_code,
        }),
        StatusCode::OK,
    ))
}

async fn join_lobby(state: AppState, payload: LobbyRequest) -> Result<impl Reply, warp::Rejection> {
    if let Some(lobby) = state.lobbies.get(&payload.lobby_code) {
        let local_id = Uuid::new_v4().to_string();
        Ok(warp::reply::with_status(
            warp::reply::json(&JoinLobbyResponse {
                local_id,
                host_id: lobby.host_id.clone(),
            }),
            StatusCode::OK,
        ))
    } else {
        Ok(warp::reply::with_status(
            warp::reply::json(&"Lobby not found"),
            StatusCode::NOT_FOUND,
        ))
    }
}

async fn register_host(
    state: AppState,
    payload: LobbyRequest,
) -> Result<impl Reply, warp::Rejection> {
    if let Some(mut lobby) = state.lobbies.get_mut(&payload.lobby_code) {
        if let Some(host_id) = payload.host_id {
            lobby.host_id = host_id;
        }
        Ok(warp::reply::with_status(warp::reply(), StatusCode::OK))
    } else {
        Ok(warp::reply::with_status(
            warp::reply(),
            StatusCode::NOT_FOUND,
        ))
    }
}

async fn request_offer(
    state: AppState,
    payload: LobbyRequest,
) -> Result<impl Reply, warp::Rejection> {
    if let Some(lobby) = state.lobbies.get(&payload.lobby_code) {
        if let Some(ref host_id) = payload.host_id {
            if let Some(peer) = lobby.peers.get(host_id) {
                let msg = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
                let _ = peer.value().send(msg);
            }
            Ok(warp::reply::with_status(warp::reply(), StatusCode::OK))
        } else {
            Ok(warp::reply::with_status(
                warp::reply(),
                StatusCode::BAD_REQUEST,
            ))
        }
    } else {
        Ok(warp::reply::with_status(
            warp::reply(),
            StatusCode::NOT_FOUND,
        ))
    }
}

async fn send_offer(
    state: AppState,
    payload: SignalingMessage,
) -> Result<impl Reply, warp::Rejection> {
    if let Some(lobby) = state.lobbies.get(&payload.lobby_code) {
        let msg = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
        for peer in lobby.peers.iter() {
            if peer.key() != &payload.from {
                let _ = peer.value().send(msg.clone());
            }
        }
        Ok(warp::reply::with_status(warp::reply(), StatusCode::OK))
    } else {
        Ok(warp::reply::with_status(
            warp::reply(),
            StatusCode::NOT_FOUND,
        ))
    }
}

async fn send_answer(
    state: AppState,
    payload: SignalingMessage,
) -> Result<impl Reply, warp::Rejection> {
    if let Some(lobby) = state.lobbies.get(&payload.lobby_code) {
        if let Some(to) = &payload.to {
            if let Some(peer) = lobby.peers.get(to) {
                let msg = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
                let _ = peer.value().send(msg);
                return Ok(warp::reply::with_status(warp::reply(), StatusCode::OK));
            }
            Ok(warp::reply::with_status(
                warp::reply(),
                StatusCode::NOT_FOUND,
            ))
        } else {
            Ok(warp::reply::with_status(
                warp::reply(),
                StatusCode::BAD_REQUEST,
            ))
        }
    } else {
        Ok(warp::reply::with_status(
            warp::reply(),
            StatusCode::NOT_FOUND,
        ))
    }
}

async fn send_ice(
    state: AppState,
    payload: SignalingMessage,
) -> Result<impl Reply, warp::Rejection> {
    if let Some(lobby) = state.lobbies.get(&payload.lobby_code) {
        let msg = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
        for peer in lobby.peers.iter() {
            if peer.key() != &payload.from {
                let _ = peer.value().send(msg.clone());
            }
        }
        Ok(warp::reply::with_status(warp::reply(), StatusCode::OK))
    } else {
        Ok(warp::reply::with_status(
            warp::reply(),
            StatusCode::NOT_FOUND,
        ))
    }
}

async fn handle_socket(ws: WebSocket, lobby_code: String, peer_id: String, state: AppState) {
    let (mut sender, mut receiver) = ws.split();

    let (tx, mut rx) = broadcast::channel::<String>(32);

    // Insert into lobby
    if let Some(lobby) = state.lobbies.get(&lobby_code) {
        lobby.peers.insert(peer_id.clone(), tx.clone());
    } else {
        println!("Peer joined non-existing lobby {}", lobby_code);
        return;
    }

    let mut rx_send = rx.resubscribe();
    // task to forward broadcast messages to the websocket
    tokio::spawn(async move {
        while let Ok(msg) = rx_send.recv().await {
            if sender.send(WsMessage::text(msg)).await.is_err() {
                break;
            }
        }
    });

    // receive messages from the websocket and broadcast to peers
    while let Some(result) = receiver.next().await {
        match result {
            Ok(msg) => {
                if msg.is_text() {
                    let txt = msg.to_str().unwrap_or("").to_string();
                    if let Some(lobby) = state.lobbies.get(&lobby_code) {
                        for peer in lobby.peers.iter() {
                            if peer.key() != &peer_id {
                                let _ = peer.value().send(txt.clone());
                            }
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("websocket error: {}", e);
                break;
            }
        }
    }

    if let Some(lobby) = state.lobbies.get(&lobby_code) {
        lobby.peers.remove(&peer_id);
    }
}

fn generate_code() -> String {
    let code: String = Uuid::new_v4()
        .to_string()
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .take(6)
        .collect();
    code
}

#[derive(serde::Deserialize)]
struct PlayerListRequest {
    lobby_code: String,
    requester_id: String,
}

async fn request_player_list(
    state: AppState,
    payload: PlayerListRequest,
) -> Result<impl Reply, warp::Rejection> {
    if let Some(lobby) = state.lobbies.get(&payload.lobby_code) {
        if let Some(peer) = lobby.peers.get(&lobby.host_id) {
            let msg = serde_json::json!({
                "type": "request_player_list",
                "requester_id": payload.requester_id
            })
            .to_string();

            let _ = peer.value().send(msg);
        }
        return Ok(warp::reply::with_status(warp::reply(), StatusCode::OK));
    }

    Ok(warp::reply::with_status(
        warp::reply(),
        StatusCode::NOT_FOUND,
    ))
}

#[derive(serde::Deserialize, serde::Serialize)]
struct PlayerInfo {
    id: String,
    name: String,
    ip: String,
    is_host: bool,
}

#[derive(serde::Deserialize)]
struct PlayerListPayload {
    lobby_code: String,
    players: Vec<PlayerInfo>,
}

async fn send_player_list(
    state: AppState,
    payload: PlayerListPayload,
) -> Result<impl Reply, warp::Rejection> {
    if let Some(lobby) = state.lobbies.get(&payload.lobby_code) {
        let msg = serde_json::json!({
            "type": "players_updated",
            "players": payload.players
        })
            .to_string();

        for peer in lobby.peers.iter() {
            let _ = peer.value().send(msg.clone());
        }

        return Ok(warp::reply::with_status(warp::reply(), StatusCode::OK));
    }

    Ok(warp::reply::with_status(
        warp::reply(),
        StatusCode::NOT_FOUND,
    ))
}