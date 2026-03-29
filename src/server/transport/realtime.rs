use std::collections::BTreeMap;
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
use crate::store::{ResolvedSubscriptionTarget, Store, StreamEvent};

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
        replace: bool,
        #[serde(default, rename = "streamId")]
        stream_id: Option<String>,
        #[serde(default, rename = "resumeFromStreamPos")]
        resume_from_stream_pos: Option<i64>,
        #[serde(default)]
        targets: Vec<String>,
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
    let mut subscribed_targets = BTreeMap::new();
    let mut stream_rx = store.subscribe();

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if handle_client_frame(
                            &mut socket,
                            store.as_ref(),
                            &viewer,
                            &mut subscribed_targets,
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
            stream_event = stream_rx.recv(), if !subscribed_targets.is_empty() => {
                match stream_event {
                    Ok(event) => {
                        if let Err(err) = forward_stream_event(&mut socket, &subscribed_targets, &event).await {
                            warn!(viewer = %viewer, error = %err, "realtime websocket send failed");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
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
    subscribed_targets: &mut BTreeMap<String, ResolvedSubscriptionTarget>,
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
            replace,
            targets,
            scopes,
            ..
        } => {
            let requested_targets = match validate_targets(store, viewer, targets, scopes).await {
                Ok(targets) => targets,
                Err(err) => {
                    let message = err.to_string();
                    let code = if message.starts_with("forbidden_scope:")
                        || message.starts_with("forbidden_target:")
                    {
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
            let mut next_subscribed_targets = if replace {
                BTreeMap::new()
            } else {
                subscribed_targets.clone()
            };
            for target in requested_targets {
                next_subscribed_targets.insert(target.target_id.clone(), target);
            }
            *subscribed_targets = next_subscribed_targets;
            send_json(
                socket,
                json!({
                    "type": "subscribed",
                    "targets": subscribed_targets.keys().cloned().collect::<Vec<_>>(),
                }),
            )
            .await?;
        }
    }
    Ok(())
}

async fn validate_targets(
    store: &Store,
    viewer: &str,
    targets: Vec<String>,
    scopes: Vec<SubscribeScope>,
) -> anyhow::Result<Vec<ResolvedSubscriptionTarget>> {
    if !targets.is_empty() {
        let mut validated = Vec::with_capacity(targets.len());
        for target in targets {
            let Some(resolved) = store.resolve_subscription_target(viewer, &target)? else {
                return Err(anyhow::anyhow!("forbidden_target:{}", target));
            };
            validated.push(resolved);
        }
        return Ok(validated);
    }

    let mut validated = Vec::with_capacity(scopes.len());
    for scope in scopes {
        let Some(resolved) =
            store.resolve_scope_subscription_target(viewer, &scope.kind, &scope.id)?
        else {
            return Err(anyhow::anyhow!(
                "forbidden_scope:{}:{}",
                scope.kind,
                scope.id
            ));
        };
        validated.push(resolved);
    }
    Ok(validated)
}

fn target_matches_stream_event(
    subscribed_targets: &BTreeMap<String, ResolvedSubscriptionTarget>,
    event: &StreamEvent,
) -> bool {
    let conversation_target = format!("conversation:{}", event.channel_id);
    subscribed_targets
        .values()
        .any(|target| target.target_id == conversation_target)
}

async fn forward_stream_event(
    socket: &mut WebSocket,
    subscribed_targets: &BTreeMap<String, ResolvedSubscriptionTarget>,
    event: &StreamEvent,
) -> anyhow::Result<()> {
    if !target_matches_stream_event(subscribed_targets, event) {
        return Ok(());
    }
    send_json(
        socket,
        json!({
            "type": "event",
            "event": {
                "eventType": event.event_type,
                "channelId": event.channel_id,
                "latestSeq": event.latest_seq,
                "payload": event.event_payload,
                "schemaVersion": event.schema_version,
            },
        }),
    )
    .await
}

async fn send_json(socket: &mut WebSocket, value: Value) -> anyhow::Result<()> {
    debug!(
        frame_type = value["type"].as_str().unwrap_or("unknown"),
        "realtime websocket send"
    );
    socket.send(Message::Text(value.to_string().into())).await?;
    Ok(())
}
