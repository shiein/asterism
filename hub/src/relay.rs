use std::sync::Arc;

use asterism_sync::Envelope;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::auth::auth_token;
use crate::device::bearer;
use crate::state::HubState;

#[derive(Deserialize)]
pub struct WsQuery {
    pub token: Option<String>,
}

pub async fn ws(
    State(state): State<Arc<HubState>>,
    headers: HeaderMap,
    Query(q): Query<WsQuery>,
    upgrade: WebSocketUpgrade,
) -> Result<impl IntoResponse, StatusCode> {
    let (account, device) =
        auth_token(&state, bearer(&headers), q.token.as_deref()).ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(upgrade.on_upgrade(move |socket| handle(state, account, device, socket)))
}

async fn handle(
    state: Arc<HubState>,
    account: asterism_core::AccountId,
    device: asterism_core::DeviceId,
    mut socket: WebSocket,
) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    state.register_socket(device, tx);
    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(env) = serde_json::from_str::<Envelope>(&text) {
                            state.relay(account, device, env);
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
            outgoing = rx.recv() => {
                let Some(env) = outgoing else { break };
                if let Ok(text) = serde_json::to_string(&env)
                    && socket.send(Message::Text(text.into())).await.is_err()
                {
                    break;
                }
            }
        }
    }
    state.unregister_socket(device);
}
