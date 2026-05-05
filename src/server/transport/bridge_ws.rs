//! Bridge ↔ Platform WebSocket handler (Phase 3, slice 1).
//!
//! Handles `GET /api/bridge/ws`. The bridge dials in, sends `bridge.hello`
//! as its first frame; the platform responds with `bridge.target` listing
//! the desired runtime config for every agent that should run on this
//! `machine_id`. Slice 1 stub: ignores `machine_id`, returns every agent
//! in the active workspace. Subsequent slices add auth, scoping, and the
//! rest of the frame catalog (`agent.state`, `chat.ack`,
//! `chat.message.received`).

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

use crate::server::handlers::AppState;
use crate::store::agents::Agent;
use crate::store::Store;

/// Wire envelope for every WS frame in the bridge protocol.
#[derive(Debug, Serialize, Deserialize)]
struct WireFrame {
    v: u32,
    #[serde(rename = "type")]
    frame_type: String,
    data: Value,
}

/// `bridge.hello` payload, sent by the bridge as its first frame.
///
/// Slice 1 only reads `machine_id` / `bridge_version` for logging.
/// `supported_frames` and `agents_alive` are part of the wire contract for
/// later slices (target-vs-alive reconciliation, frame compat) — kept here
/// so the deserializer rejects malformed payloads loudly instead of
/// silently dropping fields.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct BridgeHello {
    machine_id: String,
    bridge_version: String,
    #[serde(default)]
    supported_frames: Vec<String>,
    #[serde(default)]
    agents_alive: Vec<AgentAliveEntry>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct AgentAliveEntry {
    agent_id: String,
    state: String,
    #[serde(default)]
    runtime_pid: Option<u32>,
    #[serde(default)]
    last_acked_seq: Option<u64>,
}

/// `bridge.target` payload, sent by the platform in reply to `bridge.hello`
/// (and on every subsequent change to desired state — slice 1 only sends it
/// once per connect).
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct BridgeTarget {
    target_agents: Vec<AgentTarget>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
struct AgentTarget {
    agent_id: String,
    runtime: String,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_prompt: Option<String>,
    env_vars: Vec<EnvVarOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    init_directive: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_prompt: Option<String>,
}

#[derive(Debug, Serialize)]
struct EnvVarOut {
    key: String,
    value: String,
}

impl From<Agent> for AgentTarget {
    fn from(a: Agent) -> Self {
        AgentTarget {
            agent_id: a.id,
            runtime: a.runtime,
            model: a.model,
            system_prompt: a.system_prompt,
            env_vars: a
                .env_vars
                .into_iter()
                .map(|e| EnvVarOut {
                    key: e.key,
                    value: e.value,
                })
                .collect(),
            init_directive: None,
            pending_prompt: None,
        }
    }
}

pub async fn handle_bridge_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| bridge_session(socket, state.store.clone()))
}

async fn bridge_session(mut socket: WebSocket, store: Arc<Store>) {
    let hello = match recv_hello(&mut socket).await {
        Ok(h) => h,
        Err(err) => {
            warn!(error = %err, "bridge_ws: hello handshake failed");
            return;
        }
    };
    debug!(
        machine_id = %hello.machine_id,
        bridge_version = %hello.bridge_version,
        agents_alive = hello.agents_alive.len(),
        "bridge_ws: hello received"
    );

    if let Err(err) = send_target(&mut socket, store.as_ref()).await {
        warn!(error = %err, "bridge_ws: failed to send bridge.target");
        return;
    }

    // Slice 1 keeps the connection open after the initial reconcile so the
    // test (and a real bridge) can verify the frame and either close cleanly
    // or — in later slices — receive subsequent push frames. We respond to
    // pings, ignore other client frames, and exit on close/error.
    while let Some(msg) = socket.recv().await {
        match msg {
            Ok(Message::Ping(payload)) => {
                if socket.send(Message::Pong(payload)).await.is_err() {
                    break;
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(err) => {
                debug!(error = %err, "bridge_ws: socket recv error");
                break;
            }
        }
    }
}

async fn recv_hello(socket: &mut WebSocket) -> anyhow::Result<BridgeHello> {
    let frame = socket
        .recv()
        .await
        .ok_or_else(|| anyhow::anyhow!("connection closed before hello"))?
        .map_err(|e| anyhow::anyhow!("ws recv error: {e}"))?;
    let text = match frame {
        Message::Text(t) => t,
        other => anyhow::bail!("expected text frame for hello, got {other:?}"),
    };
    let envelope: WireFrame = serde_json::from_str(text.as_str())
        .map_err(|e| anyhow::anyhow!("failed to parse hello envelope: {e}"))?;
    if envelope.frame_type != "bridge.hello" {
        anyhow::bail!(
            "first frame must be bridge.hello, got {}",
            envelope.frame_type
        );
    }
    let hello: BridgeHello = serde_json::from_value(envelope.data)
        .map_err(|e| anyhow::anyhow!("failed to parse hello payload: {e}"))?;
    Ok(hello)
}

async fn send_target(socket: &mut WebSocket, store: &Store) -> anyhow::Result<()> {
    let agents = store.get_agents()?;
    let target = BridgeTarget {
        target_agents: agents.into_iter().map(AgentTarget::from).collect(),
    };
    let envelope = WireFrame {
        v: 1,
        frame_type: "bridge.target".to_string(),
        data: serde_json::to_value(&target)?,
    };
    let text = serde_json::to_string(&envelope)?;
    socket.send(Message::Text(text.into())).await?;
    Ok(())
}
