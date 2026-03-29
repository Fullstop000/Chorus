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
use crate::store::{ResolvedSubscriptionTarget, Store, StoredEvent};

#[derive(Debug, Deserialize)]
pub struct RealtimeParams {
    pub viewer: String,
}

#[derive(Debug, Deserialize)]
struct SubscribeScope {
    kind: String,
    id: String,
}

#[derive(Debug, Clone)]
enum ReplayCursor {
    Global {
        event_id: i64,
    },
    Stream {
        stream_id: String,
        stream_pos: i64,
        fallback_event_id: i64,
    },
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
    let mut replay_cursor = ReplayCursor::Global { event_id: 0 };
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
                            &mut subscribed_targets,
                            &mut replay_cursor,
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
            event_notice = event_rx.recv(), if !subscribed_targets.is_empty() => {
                match event_notice {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                        match replay_matching_events(
                            &mut socket,
                            store.as_ref(),
                            &viewer,
                            &subscribed_targets,
                            &replay_cursor,
                        ).await {
                            Ok(updated_cursor) => {
                                replay_cursor = updated_cursor;
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
    subscribed_targets: &mut BTreeMap<String, ResolvedSubscriptionTarget>,
    replay_cursor: &mut ReplayCursor,
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
            replace,
            stream_id,
            resume_from_stream_pos,
            targets,
            scopes,
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
            let all_targets = next_subscribed_targets
                .values()
                .cloned()
                .collect::<Vec<_>>();
            let shared_stream_id = store.shared_stream_id_for_targets(&all_targets)?;
            let active_stream_id = match (shared_stream_id, stream_id) {
                (Some(shared_stream_id), Some(requested_stream_id))
                    if requested_stream_id != shared_stream_id =>
                {
                    send_json(
                        socket,
                        json!({
                            "type": "error",
                            "code": "invalid_scope",
                            "message": format!(
                                "requested stream {} does not match subscribed targets",
                                requested_stream_id
                            ),
                        }),
                    )
                    .await?;
                    return Ok(());
                }
                (Some(shared_stream_id), _) => Some(shared_stream_id),
                (None, _) => None,
            };

            let fallback_event_id = resume_from.unwrap_or(match replay_cursor {
                ReplayCursor::Global { event_id } => *event_id,
                ReplayCursor::Stream {
                    fallback_event_id, ..
                } => *fallback_event_id,
            });

            if let Some(stream_id) = active_stream_id.clone() {
                let current_stream_pos = match replay_cursor {
                    ReplayCursor::Stream {
                        stream_id: current_stream_id,
                        stream_pos,
                        ..
                    } if current_stream_id == &stream_id => *stream_pos,
                    _ => 0,
                };
                *replay_cursor = ReplayCursor::Stream {
                    stream_id: stream_id.clone(),
                    stream_pos: resume_from_stream_pos.unwrap_or(current_stream_pos),
                    fallback_event_id,
                };
            } else {
                *replay_cursor = ReplayCursor::Global {
                    event_id: fallback_event_id,
                };
            }
            *subscribed_targets = next_subscribed_targets;
            send_json(
                socket,
                json!({
                    "type": "subscribed",
                    "resumeFrom": fallback_event_id,
                    "streamId": active_stream_id,
                    "resumeFromStreamPos": match replay_cursor {
                        ReplayCursor::Stream { stream_pos, .. } => Some(*stream_pos),
                        ReplayCursor::Global { .. } => None,
                    },
                    "targets": subscribed_targets.keys().cloned().collect::<Vec<_>>(),
                    "scopes": [],
                }),
            )
            .await?;
            *replay_cursor =
                replay_matching_events(socket, store, viewer, subscribed_targets, replay_cursor)
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

async fn replay_matching_events(
    socket: &mut WebSocket,
    store: &Store,
    viewer: &str,
    subscribed_targets: &BTreeMap<String, ResolvedSubscriptionTarget>,
    replay_cursor: &ReplayCursor,
) -> anyhow::Result<ReplayCursor> {
    match replay_cursor {
        ReplayCursor::Global { event_id } => {
            let mut cursor = *event_id;
            loop {
                let events =
                    store.list_events(if cursor > 0 { Some(cursor) } else { None }, 200)?;
                if events.is_empty() {
                    break;
                }

                for event in events {
                    cursor = event.event_id;
                    if target_matches(subscribed_targets, &event) {
                        send_json(
                            socket,
                            json!({
                                "type": "event",
                                "event": event_to_json_value_for_viewer(store, viewer, &event),
                            }),
                        )
                        .await?;
                    }
                }

                if store.list_events(Some(cursor), 1)?.is_empty() {
                    break;
                }
            }
            Ok(ReplayCursor::Global { event_id: cursor })
        }
        ReplayCursor::Stream {
            stream_id,
            stream_pos,
            fallback_event_id,
        } => {
            let mut cursor = *stream_pos;
            let mut latest_event_id = *fallback_event_id;
            loop {
                let events = store.list_events_for_stream(
                    stream_id,
                    if cursor > 0 { Some(cursor) } else { None },
                    200,
                )?;
                if events.is_empty() {
                    break;
                }

                for event in events {
                    cursor = event.stream_pos;
                    latest_event_id = latest_event_id.max(event.event_id);
                    if target_matches(subscribed_targets, &event) {
                        send_json(
                            socket,
                            json!({
                                "type": "event",
                                "event": event_to_json_value_for_viewer(store, viewer, &event),
                            }),
                        )
                        .await?;
                    }
                }

                if store
                    .list_events_for_stream(stream_id, Some(cursor), 1)?
                    .is_empty()
                {
                    break;
                }
            }
            Ok(ReplayCursor::Stream {
                stream_id: stream_id.clone(),
                stream_pos: cursor,
                fallback_event_id: latest_event_id,
            })
        }
    }
}

fn target_matches(
    subscribed_targets: &BTreeMap<String, ResolvedSubscriptionTarget>,
    event: &StoredEvent,
) -> bool {
    subscribed_targets
        .values()
        .any(|target| target.matches_event(event))
}

pub fn event_to_json_value(store: &Store, event: &StoredEvent) -> Value {
    event_to_json_value_with_store(Some(store), event.actor_name.as_deref(), event)
}

pub fn event_to_json_value_for_viewer(store: &Store, viewer: &str, event: &StoredEvent) -> Value {
    event_to_json_value_with_store(Some(store), Some(viewer), event)
}

fn event_to_json_value_with_store(
    store: Option<&Store>,
    viewer: Option<&str>,
    event: &StoredEvent,
) -> Value {
    let actor = event
        .actor_name
        .as_ref()
        .map(|name| json!({ "name": name, "type": event.actor_type }));
    let caused_by = event
        .caused_by_kind
        .as_ref()
        .map(|kind| json!({ "kind": kind }));
    let payload = transport_payload_for_event(store, viewer, event);
    json!({
        "eventId": event.event_id,
        "streamId": event.stream_id,
        "streamKind": event.stream_kind,
        "streamPos": event.stream_pos,
        "eventType": if event.is_message_created() {
            "conversation.state"
        } else {
            event.event_type.as_str()
        },
        "scopeKind": event.scope_kind,
        "scopeId": event.scope_id,
        "channelId": event.channel_id,
        "channelName": event.channel_name,
        "threadParentId": event.thread_parent_id,
        "actor": actor,
        "causedBy": caused_by,
        "payload": payload,
        "createdAt": event.created_at.to_rfc3339(),
    })
}

fn transport_payload_for_event(
    store: Option<&Store>,
    viewer: Option<&str>,
    event: &StoredEvent,
) -> Value {
    if event.is_message_created() || event.is_conversation_state() {
        return conversation_state_payload(store, viewer, event);
    }
    if event.is_thread_state() {
        return thread_state_payload(store, viewer, event);
    }
    event.payload.clone()
}

fn conversation_state_payload(
    store: Option<&Store>,
    viewer: Option<&str>,
    event: &StoredEvent,
) -> Value {
    let message_id = event
        .payload
        .get("messageId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if message_id.is_empty() {
        return event.payload.clone();
    }

    let Some(store) = store else {
        return event.payload.clone();
    };
    let Some(message_view) = store
        .get_conversation_message_view(message_id)
        .ok()
        .flatten()
    else {
        return event.payload.clone();
    };

    let (last_read_seq, unread_count) = event
        .channel_name
        .as_deref()
        .and_then(|channel_name| {
            viewer.and_then(|viewer_name| {
                store
                    .get_inbox_conversation_state(channel_name, viewer_name)
                    .ok()
                    .flatten()
            })
        })
        .map(|state| (state.last_read_seq, state.unread_count))
        .unwrap_or((0, 0));

    let mut payload = event.payload.as_object().cloned().unwrap_or_default();
    payload.insert("conversationId".into(), json!(message_view.conversation_id));
    payload.insert(
        "conversationType".into(),
        json!(message_view.conversation_type),
    );
    payload.insert("messageId".into(), json!(message_view.message_id));
    payload.insert("latestSeq".into(), json!(message_view.seq));
    payload.insert("lastReadSeq".into(), json!(last_read_seq));
    payload.insert("unreadCount".into(), json!(unread_count));
    payload.insert(
        "threadParentId".into(),
        json!(message_view.thread_parent_id),
    );
    payload.insert("createdAt".into(), json!(message_view.created_at));
    Value::Object(payload)
}

fn thread_state_payload(store: Option<&Store>, viewer: Option<&str>, event: &StoredEvent) -> Value {
    let Some(store) = store else {
        return event.payload.clone();
    };
    let Some(channel_name) = event.channel_name.as_deref() else {
        return event.payload.clone();
    };
    let Some(parent_message_id) = event
        .thread_parent_id
        .as_deref()
        .or_else(|| event.payload.get("threadParentId").and_then(Value::as_str))
    else {
        return event.payload.clone();
    };

    let mut payload = event.payload.as_object().cloned().unwrap_or_default();
    if let Some(viewer_name) = viewer {
        if let Ok(Some(thread_state)) =
            store.get_thread_notification_state(channel_name, parent_message_id, viewer_name)
        {
            payload.insert("conversationId".into(), json!(thread_state.conversation_id));
            payload.insert(
                "threadParentId".into(),
                json!(thread_state.thread_parent_id),
            );
            payload.insert("latestSeq".into(), json!(thread_state.latest_seq));
            payload.insert("lastReadSeq".into(), json!(thread_state.last_read_seq));
            payload.insert("unreadCount".into(), json!(thread_state.unread_count));
            payload.insert(
                "lastReplyMessageId".into(),
                json!(thread_state.last_reply_message_id),
            );
            payload.insert("lastReplyAt".into(), json!(thread_state.last_reply_at));
        }
    }
    Value::Object(payload)
}

async fn send_json(socket: &mut WebSocket, value: Value) -> anyhow::Result<()> {
    debug!(
        frame_type = value["type"].as_str().unwrap_or("unknown"),
        "realtime websocket send"
    );
    socket.send(Message::Text(value.to_string().into())).await?;
    Ok(())
}
