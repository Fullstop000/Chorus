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
        .map_err(|e| api_err(e.to_string()))?
        .is_none()
    {
        return Err(api_err(format!("viewer not found: {}", params.viewer)));
    }

    Ok(ws.on_upgrade(move |socket| realtime_session(socket, state.store.clone(), params.viewer)))
}

async fn realtime_session(mut socket: WebSocket, store: Arc<Store>, viewer: String) {
    let mut stream_rx = store.subscribe();

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
                        warn!(viewer = %viewer, error = %err, "realtime websocket receive failed");
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

async fn send_json(socket: &mut WebSocket, value: Value) -> anyhow::Result<()> {
    debug!(
        frame_type = value["type"].as_str().unwrap_or("unknown"),
        "realtime websocket send"
    );
    socket.send(Message::Text(value.to_string().into())).await?;
    Ok(())
}
