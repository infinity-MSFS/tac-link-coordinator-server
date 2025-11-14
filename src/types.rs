use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) lobbies: Arc<DashMap<String, Lobby>>,
}

pub(crate) struct Lobby {
    pub host_id: String,
    pub peers: DashMap<String, broadcast::Sender<String>>,
}

#[derive(Serialize)]
pub(crate) struct CreateLobbyResponse {
    pub local_id: String,
    pub lobby_code: String,
}

#[derive(Serialize)]
pub(crate) struct JoinLobbyResponse {
    pub local_id: String,
    pub host_id: String,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct LobbyRequest {
    pub lobby_code: String,
    pub host_id: Option<String>,
    pub joiner_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct SignalingMessage {
    pub r#type: String,
    pub lobby_code: String,
    pub from: String,
    pub to: Option<String>,
    pub sdp: Option<String>,
    pub candidate: Option<serde_json::Value>,
}
