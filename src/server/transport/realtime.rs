use std::collections::BTreeSet;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::server::handlers::{api_err, AppState, ErrorResponse};
use crate::store::{Store, StoredEvent};

#[derive(Debug, Deserialize)]
pub struct RealtimeParams {
    pub viewer: String,
}

#[derive(Debug, Deserialize)]
struct SubscribeScope {
    kind: String,
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientFrame {
    Subscribe {
        #[serde(default, rename = "resumeFrom")]
        resume_from: Option<i64>,
        #[serde(default)]
        scopes: Vec<SubscribeScope>,
    },
}

pub async fn handle_events_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(params): Query<RealtimeParams>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, Json<ErrorResponse>)> {
    if state
        .store
        .lookup_sender_type(&params.viewer)
        .map_err(|e| api_err(e.to_string()))?
        .is_none()
    {
        return Err(api_err(format!("viewer not found: {}", params.viewer)));
    }

    Ok(ws.on_upgrade(move |socket| realtime_session(socket, state.store.clone(), params.viewer)))
}

async fn realtime_session(mut socket: WebSocket, store: Arc<Store>, viewer: String) {
    let mut subscribed_scopes = BTreeSet::new();
    let mut last_seen_event_id = 0_i64;
    let mut event_rx = store.subscribe_events();

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if handle_client_frame(
                            &mut socket,
                            store.as_ref(),
                            &viewer,
                            &mut subscribed_scopes,
                            &mut last_seen_event_id,
                            text.as_str(),
                        ).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        warn!(viewer = %viewer, error = %err, "realtime websocket receive failed");
                        break;
                    }
                }
            }
            event_notice = event_rx.recv(), if !subscribed_scopes.is_empty() => {
                match event_notice {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                        match replay_matching_events(
                            &mut socket,
                            store.as_ref(),
                            &subscribed_scopes,
                            last_seen_event_id,
                        ).await {
                            Ok(updated_cursor) => {
                                last_seen_event_id = updated_cursor;
                            }
                            Err(err) => {
                                warn!(viewer = %viewer, error = %err, "realtime websocket replay failed");
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

async fn handle_client_frame(
    socket: &mut WebSocket,
    store: &Store,
    viewer: &str,
    subscribed_scopes: &mut BTreeSet<(String, String)>,
    last_seen_event_id: &mut i64,
    text: &str,
) -> anyhow::Result<()> {
    let frame: ClientFrame = match serde_json::from_str(text) {
        Ok(frame) => frame,
        Err(err) => {
            send_json(
                socket,
                json!({
                    "type": "error",
                    "code": "invalid_request",
                    "message": err.to_string(),
                }),
            )
            .await?;
            return Ok(());
        }
    };
    match frame {
        ClientFrame::Subscribe {
            resume_from,
            scopes,
        } => {
            let requested_scopes = match validate_scopes(store, viewer, scopes).await {
                Ok(scopes) => scopes,
                Err(err) => {
                    let message = err.to_string();
                    let code = if message.starts_with("forbidden_scope:") {
                        "forbidden_scope"
                    } else {
                        "invalid_scope"
                    };
                    send_json(
                        socket,
                        json!({
                            "type": "error",
                            "code": code,
                            "message": message,
                        }),
                    )
                    .await?;
                    return Ok(());
                }
            };
            for scope in &requested_scopes {
                subscribed_scopes.insert(scope.clone());
            }
            if let Some(resume_from) = resume_from {
                *last_seen_event_id = resume_from;
            }
            send_json(
                socket,
                json!({
                    "type": "subscribed",
                    "resumeFrom": *last_seen_event_id,
                    "scopes": requested_scopes.iter().map(|(kind, id)| json!({ "kind": kind, "id": id })).collect::<Vec<_>>(),
                }),
            )
            .await?;
            *last_seen_event_id =
                replay_matching_events(socket, store, subscribed_scopes, *last_seen_event_id)
                    .await?;
        }
    }
    Ok(())
}

async fn validate_scopes(
    store: &Store,
    viewer: &str,
    scopes: Vec<SubscribeScope>,
) -> anyhow::Result<Vec<(String, String)>> {
    let mut validated = Vec::with_capacity(scopes.len());
    for scope in scopes {
        if !store.can_access_event_scope(viewer, &scope.kind, &scope.id)? {
            return Err(anyhow::anyhow!(
                "forbidden_scope:{}:{}",
                scope.kind,
                scope.id
            ));
        }
        validated.push((scope.kind, scope.id));
    }
    Ok(validated)
}

async fn replay_matching_events(
    socket: &mut WebSocket,
    store: &Store,
    subscribed_scopes: &BTreeSet<(String, String)>,
    after_event_id: i64,
) -> anyhow::Result<i64> {
    let mut cursor = after_event_id;
    loop {
        let events = store.list_events(if cursor > 0 { Some(cursor) } else { None }, 200)?;
        if events.is_empty() {
            break;
        }

        for event in events {
            cursor = event.event_id;
            if scope_matches(subscribed_scopes, &event) {
                send_json(
                    socket,
                    json!({
                        "type": "event",
                        "event": event_to_json_value(&event),
                    }),
                )
                .await?;
            }
        }

        if store.list_events(Some(cursor), 1)?.is_empty() {
            break;
        }
    }
    Ok(cursor)
}

fn scope_matches(subscribed_scopes: &BTreeSet<(String, String)>, event: &StoredEvent) -> bool {
    subscribed_scopes.contains(&(event.scope_kind.clone(), event.scope_id.clone()))
}

pub fn event_to_json_value(event: &StoredEvent) -> Value {
    let actor = event
        .actor_name
        .as_ref()
        .map(|name| json!({ "name": name, "type": event.actor_type }));
    let caused_by = event
        .caused_by_kind
        .as_ref()
        .map(|kind| json!({ "kind": kind }));
    json!({
        "eventId": event.event_id,
        "eventType": event.event_type,
        "scopeKind": event.scope_kind,
        "scopeId": event.scope_id,
        "channelId": event.channel_id,
        "channelName": event.channel_name,
        "threadParentId": event.thread_parent_id,
        "actor": actor,
        "causedBy": caused_by,
        "payload": event.payload,
        "createdAt": event.created_at.to_rfc3339(),
    })
}

async fn send_json(socket: &mut WebSocket, value: Value) -> anyhow::Result<()> {
    debug!(frame_type = value["type"].as_str().unwrap_or("unknown"), "realtime websocket send");
    socket.send(Message::Text(value.to_string().into())).await?;
    Ok(())
}
