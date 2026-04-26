use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::agent::trace::TraceEvent;
use crate::server::error::{app_err, ErrorResponse};
use crate::server::handlers::AppState;
use crate::store::{Store, StreamEvent};

#[derive(Debug, Deserialize)]
pub struct RealtimeParams {
    pub viewer: String,
}

pub async fn handle_events_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Query(params): Query<RealtimeParams>,
) -> Result<impl IntoResponse, (axum::http::StatusCode, Json<ErrorResponse>)> {
    if state
        .store
        .lookup_sender_type(&params.viewer)
        .map_err(|e| app_err!(StatusCode::BAD_REQUEST, e.to_string()))?
        .is_none()
    {
        return Err(app_err!(
            StatusCode::BAD_REQUEST,
            "viewer not found: {}",
            params.viewer
        ));
    }

    Ok(ws.on_upgrade(move |socket| realtime_session(socket, state.store.clone(), params.viewer)))
}

async fn realtime_session(mut socket: WebSocket, store: Arc<Store>, viewer: String) {
    let mut stream_rx = store.subscribe();
    let mut trace_rx = store.subscribe_traces();

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    // No client subscription protocol: ignore application text frames.
                    Some(Ok(Message::Text(_))) => {}
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        debug!(viewer = %viewer, error = %err, "realtime websocket receive failed");
                        break;
                    }
                }
            }
            stream_event = stream_rx.recv() => {
                match stream_event {
                    Ok(event) => {
                        if let Err(err) =
                            forward_stream_event(&mut socket, store.as_ref(), &viewer, &event).await
                        {
                            warn!(viewer = %viewer, error = %err, "realtime websocket send failed");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            trace_event = trace_rx.recv() => {
                match trace_event {
                    Ok(event) => {
                        if let Err(err) =
                            forward_trace_event(&mut socket, store.as_ref(), &viewer, &event).await
                        {
                            warn!(viewer = %viewer, error = %err, "realtime websocket trace send failed");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!(viewer = %viewer, lagged = n, "trace broadcast lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

async fn forward_stream_event(
    socket: &mut WebSocket,
    store: &Store,
    viewer: &str,
    event: &StreamEvent,
) -> anyhow::Result<()> {
    let is_member = store.channel_member_exists(&event.channel_id, viewer)?;
    if !is_member {
        debug!(
            viewer = %viewer,
            channel_id = %event.channel_id,
            event_type = %event.event_type,
            latest_seq = event.latest_seq,
            "realtime skip stream event: viewer is not a member of the channel"
        );
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

/// Forward a trace event to the viewer if they are a member of the run's channel.
async fn forward_trace_event(
    socket: &mut WebSocket,
    store: &Store,
    viewer: &str,
    event: &TraceEvent,
) -> anyhow::Result<()> {
    let is_visible = match event.channel_id {
        Some(ref ch_id) => store.channel_member_exists(ch_id, viewer).unwrap_or(false),
        // No channel context yet — fall back to checking all agent channels.
        None => {
            let agent_channels = store.agent_channel_ids(&event.agent_name)?;
            agent_channels
                .iter()
                .any(|ch_id| store.channel_member_exists(ch_id, viewer).unwrap_or(false))
        }
    };
    if !is_visible {
        return Ok(());
    }

    let kind_data = match &event.kind {
        crate::agent::trace::TraceEventKind::Reading => {
            json!({})
        }
        crate::agent::trace::TraceEventKind::Thinking { text } => {
            json!({ "text": text })
        }
        crate::agent::trace::TraceEventKind::ToolCall {
            tool_name,
            tool_input,
        } => {
            json!({ "toolName": tool_name, "toolInput": tool_input })
        }
        crate::agent::trace::TraceEventKind::ToolResult { tool_name, content } => {
            json!({ "toolName": tool_name, "content": content })
        }
        crate::agent::trace::TraceEventKind::Text { text } => {
            json!({ "text": text })
        }
        crate::agent::trace::TraceEventKind::TurnEnd => {
            json!({})
        }
        crate::agent::trace::TraceEventKind::Error { message } => {
            json!({ "message": message })
        }
    };

    let kind_str = match &event.kind {
        crate::agent::trace::TraceEventKind::Reading => "reading",
        crate::agent::trace::TraceEventKind::Thinking { .. } => "thinking",
        crate::agent::trace::TraceEventKind::ToolCall { .. } => "tool_call",
        crate::agent::trace::TraceEventKind::ToolResult { .. } => "tool_result",
        crate::agent::trace::TraceEventKind::Text { .. } => "text",
        crate::agent::trace::TraceEventKind::TurnEnd => "turn_end",
        crate::agent::trace::TraceEventKind::Error { .. } => "error",
    };

    send_json(
        socket,
        json!({
            "type": "trace",
            "event": {
                "eventType": "agent.trace",
                "runId": event.run_id,
                "agentName": event.agent_name,
                "channelId": event.channel_id,
                "seq": event.seq,
                "timestampMs": event.timestamp_ms,
                "kind": kind_str,
                "data": kind_data,
            },
        }),
    )
    .await
}

async fn send_json(socket: &mut WebSocket, value: Value) -> anyhow::Result<()> {
    debug!(value = value.to_string(), "realtime websocket send");
    socket.send(Message::Text(value.to_string().into())).await?;
    Ok(())
}
