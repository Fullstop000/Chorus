//! Bridge ↔ Platform WebSocket handler (Phase 3, slices 1-3).
//!
//! `GET /api/bridge/ws`. The bridge dials in, sends `bridge.hello` as
//! its first frame; the platform replies with `bridge.target` listing
//! the desired runtime config for every agent that should run on this
//! bridge. The platform pushes a fresh `bridge.target` on every change
//! to desired state (see `broadcast_target_update`). The bridge sends
//! `agent.state` frames upstream when its runtimes transition.
//!
//! Slice 3 adds the `runtime_pid` instance discriminator on
//! `agent.state`: `started` events record the current pid in
//! `BridgeRegistry`, and non-started transitions are dropped if their
//! pid doesn't match. This blocks the stop→start race where a delayed
//! `crashed` from the previous instance would otherwise silently mark
//! the live new instance dead.
//!
//! Auth, `chat.message.received` push, and `chat.ack` come in later
//! slices.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::server::bridge_registry::BridgeRegistry;
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
/// Slice 1-2 reads `machine_id` (registry key) + `bridge_version` for
/// logging. `supported_frames` and `agents_alive` are part of the wire
/// contract for later slices (frame compat negotiation, target-vs-alive
/// reconciliation). Kept on the deserializer with `#[allow(dead_code)]`
/// so malformed payloads still fail loudly.
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

/// `agent.state` payload, sent by the bridge when one of its runtimes
/// transitions. `runtime_pid` is the instance discriminator: the
/// platform tracks the pid set by the most recent `started` transition
/// and drops state updates whose pid doesn't match (slice 3).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct AgentStatePayload {
    agent_id: String,
    state: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    ts: Option<String>,
    runtime_pid: u32,
}

/// `bridge.target` payload, sent by the platform in reply to
/// `bridge.hello` and on every change to desired state.
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
    ws.on_upgrade(move |socket| {
        bridge_session(socket, state.store.clone(), state.bridge_registry.clone())
    })
}

async fn bridge_session(mut socket: WebSocket, store: Arc<Store>, registry: Arc<BridgeRegistry>) {
    let hello = match recv_hello(&mut socket).await {
        Ok(h) => h,
        Err(err) => {
            warn!(error = %err, "bridge_ws: hello handshake failed");
            return;
        }
    };
    info!(
        machine_id = %hello.machine_id,
        bridge_version = %hello.bridge_version,
        agents_alive = hello.agents_alive.len(),
        "bridge_ws: bridge connected"
    );

    let machine_id = hello.machine_id.clone();
    let (mut outbound_rx, _registration) = registry.register(&machine_id);

    if let Err(err) = send_initial_target(&mut socket, store.as_ref()).await {
        warn!(machine_id = %machine_id, error = %err, "bridge_ws: failed to send initial bridge.target");
        return;
    }

    // Session loop: forward outbound frames pushed by the platform
    // (push-on-change `bridge.target` etc.) and process inbound frames
    // from the bridge (`agent.state`, future `chat.ack`).
    loop {
        tokio::select! {
            outbound = outbound_rx.recv() => {
                match outbound {
                    Some(text) => {
                        if let Err(err) = socket.send(Message::Text(text.into())).await {
                            debug!(machine_id = %machine_id, error = %err, "bridge_ws: outbound send failed; closing");
                            break;
                        }
                    }
                    // Sender dropped — shouldn't happen while registration
                    // is alive; treat as terminal.
                    None => break,
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Err(err) =
                            handle_inbound_frame(&machine_id, text.as_str(), registry.as_ref()).await
                        {
                            warn!(machine_id = %machine_id, error = %err, "bridge_ws: dropping malformed inbound frame");
                            // Don't disconnect on a single bad frame —
                            // log and keep the session alive. A series
                            // of bad frames will eventually trigger
                            // overrun close in a later slice.
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        info!(machine_id = %machine_id, "bridge_ws: bridge disconnected");
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        debug!(machine_id = %machine_id, error = %err, "bridge_ws: socket recv error; closing");
                        break;
                    }
                }
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

async fn send_initial_target(socket: &mut WebSocket, store: &Store) -> anyhow::Result<()> {
    let text = build_target_frame_text(store)?;
    socket.send(Message::Text(text.into())).await?;
    Ok(())
}

async fn handle_inbound_frame(
    machine_id: &str,
    text: &str,
    registry: &BridgeRegistry,
) -> anyhow::Result<()> {
    let envelope: WireFrame = serde_json::from_str(text)
        .map_err(|e| anyhow::anyhow!("failed to parse inbound envelope: {e}"))?;
    match envelope.frame_type.as_str() {
        "agent.state" => {
            let payload: AgentStatePayload = serde_json::from_value(envelope.data)
                .map_err(|e| anyhow::anyhow!("failed to parse agent.state payload: {e}"))?;
            // `started` transitions establish the current pid for this
            // agent on this machine. Other transitions are checked
            // against the tracker to filter stale frames from a
            // previous instance (the stop→start race).
            if payload.state == "started" {
                registry.record_started(machine_id, &payload.agent_id, payload.runtime_pid);
                info!(
                    machine_id = %machine_id,
                    agent_id = %payload.agent_id,
                    runtime_pid = payload.runtime_pid,
                    "bridge_ws: agent.state started — pid recorded"
                );
            } else if !registry.is_current_pid(machine_id, &payload.agent_id, payload.runtime_pid) {
                warn!(
                    machine_id = %machine_id,
                    agent_id = %payload.agent_id,
                    state = %payload.state,
                    runtime_pid = payload.runtime_pid,
                    "bridge_ws: dropping agent.state with stale runtime_pid"
                );
                return Ok(());
            } else {
                info!(
                    machine_id = %machine_id,
                    agent_id = %payload.agent_id,
                    state = %payload.state,
                    runtime_pid = payload.runtime_pid,
                    reason = payload.reason.as_deref().unwrap_or(""),
                    ts = payload.ts.as_deref().unwrap_or(""),
                    "bridge_ws: agent.state received"
                );
            }
            Ok(())
        }
        // Frames defined in §3.2 but not yet handled in this slice.
        "chat.ack" => {
            debug!(
                machine_id = %machine_id,
                "bridge_ws: chat.ack received (deferred to slice 4)"
            );
            Ok(())
        }
        // Bridge re-sending hello (allowed per §6 belt-and-suspenders
        // self-correction). Treat as a no-op for now; later slices will
        // trigger a fresh target push in response.
        "bridge.hello" => {
            debug!(machine_id = %machine_id, "bridge_ws: re-hello received (slice 2 ignores)");
            Ok(())
        }
        other => {
            anyhow::bail!("unknown inbound frame type: {other}");
        }
    }
}

/// Build a serialized `bridge.target` frame from the current set of
/// agents in the store. Public so agent CRUD handlers can re-use the
/// exact same encoding when triggering a push.
pub fn build_target_frame_text(store: &Store) -> anyhow::Result<String> {
    let agents = store.get_agents()?;
    let target = BridgeTarget {
        target_agents: agents.into_iter().map(AgentTarget::from).collect(),
    };
    let envelope = WireFrame {
        v: 1,
        frame_type: "bridge.target".to_string(),
        data: serde_json::to_value(&target)?,
    };
    Ok(serde_json::to_string(&envelope)?)
}

/// Push a fresh `bridge.target` to every connected bridge. Called from
/// agent CRUD handlers after they mutate state. Failure to build the
/// frame is logged but doesn't surface to the HTTP caller — the agent
/// CRUD itself succeeded; bridges will reconcile on next re-hello at
/// worst.
pub fn broadcast_target_update(store: &Store, registry: &BridgeRegistry) {
    let text = match build_target_frame_text(store) {
        Ok(t) => t,
        Err(err) => {
            warn!(error = %err, "bridge_ws: failed to build target frame for broadcast");
            return;
        }
    };
    let delivered = registry.broadcast(&text);
    debug!(delivered, "bridge_ws: broadcast bridge.target on change");
}
