//! Bridge ↔ Platform WebSocket handler (Phase 3, slices 1-4).
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
//! Slice 4 adds bearer-token auth on the WS upgrade. If the platform
//! has tokens configured (via `CHORUS_BRIDGE_TOKENS` env var or
//! explicit `BridgeAuth`), the request must include
//! `Authorization: Bearer <token>` and the `bridge.hello.machine_id`
//! must match the token's bound `machine_id`. With no tokens
//! configured, auth is disabled and any client may connect (loopback
//! default).
//!
//! `chat.message.received` push and `chat.ack` come in later slices.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, warn};

use crate::server::bridge_auth::AuthOutcome;
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

/// `chat.ack` payload, sent by the bridge after it has buffered a
/// `chat.message.received` batch into an agent's local mailbox. Slice 5
/// uses it to advance an in-memory cursor in `BridgeRegistry`; future
/// slices will persist `last_acked_seq` to the DB so reconnect-replay
/// only re-emits messages the bridge hasn't seen.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ChatAckPayload {
    agent_id: String,
    last_seq: i64,
    #[serde(default)]
    #[allow(dead_code)]
    ts: Option<String>,
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
    headers: HeaderMap,
) -> axum::response::Response {
    let expected_machine_id = match state.bridge_auth.check(&headers) {
        AuthOutcome::Disabled => None,
        AuthOutcome::Allowed {
            expected_machine_id,
        } => Some(expected_machine_id),
        AuthOutcome::Rejected => {
            warn!("bridge_ws: rejecting upgrade — invalid or missing bearer token");
            return (StatusCode::UNAUTHORIZED, "missing or invalid bearer token").into_response();
        }
    };
    ws.on_upgrade(move |socket| {
        bridge_session(
            socket,
            state.store.clone(),
            state.bridge_registry.clone(),
            expected_machine_id,
        )
    })
    .into_response()
}

async fn bridge_session(
    mut socket: WebSocket,
    store: Arc<Store>,
    registry: Arc<BridgeRegistry>,
    expected_machine_id: Option<String>,
) {
    let hello = match recv_hello(&mut socket).await {
        Ok(h) => h,
        Err(err) => {
            warn!(error = %err, "bridge_ws: hello handshake failed");
            return;
        }
    };

    // Auth pin: when the upgrade carried a known token, the bridge must
    // declare the `machine_id` that token is bound to. Anything else is
    // a spoof attempt — close without sending a target.
    if let Some(ref expected) = expected_machine_id {
        if hello.machine_id != *expected {
            warn!(
                token_bound_machine_id = %expected,
                claimed_machine_id = %hello.machine_id,
                "bridge_ws: dropping connection — bridge.hello.machine_id does not match token binding"
            );
            return;
        }
    }

    info!(
        machine_id = %hello.machine_id,
        bridge_version = %hello.bridge_version,
        agents_alive = hello.agents_alive.len(),
        auth = expected_machine_id.is_some(),
        "bridge_ws: bridge connected"
    );

    let machine_id = hello.machine_id.clone();
    let (mut outbound_rx, _registration) = registry.register(&machine_id);

    if let Err(err) = send_initial_target(&mut socket, store.as_ref(), &machine_id).await {
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

async fn send_initial_target(
    socket: &mut WebSocket,
    store: &Store,
    machine_id: &str,
) -> anyhow::Result<()> {
    let text = build_target_frame_text_for_machine(store, machine_id)?;
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
        "chat.ack" => {
            let payload: ChatAckPayload = serde_json::from_value(envelope.data)
                .map_err(|e| anyhow::anyhow!("failed to parse chat.ack payload: {e}"))?;
            registry.record_chat_ack(machine_id, &payload.agent_id, payload.last_seq);
            debug!(
                machine_id = %machine_id,
                agent_id = %payload.agent_id,
                last_seq = payload.last_seq,
                "bridge_ws: chat.ack cursor advanced"
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
/// agents in the store, scoped to a specific bridge `machine_id`. An
/// agent is included if its `machine_id` matches the connecting
/// bridge's, or if the agent has no `machine_id` set (NULL ownership =
/// any bridge can run it; back-compat with pre-slice-6 agents).
pub fn build_target_frame_text_for_machine(
    store: &Store,
    machine_id: &str,
) -> anyhow::Result<String> {
    let agents: Vec<Agent> = store
        .get_agents()?
        .into_iter()
        .filter(|a| match a.machine_id.as_deref() {
            None => true,
            Some(m) => m == machine_id,
        })
        .collect();
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

/// Push a fresh `bridge.target` to every connected bridge, scoped to
/// each bridge's `machine_id`. Called from agent CRUD handlers after
/// they mutate state. Failures to build a frame for one bridge are
/// logged but don't block the others.
pub fn broadcast_target_update(store: &Store, registry: &BridgeRegistry) {
    for machine_id in registry.connected_machine_ids() {
        match build_target_frame_text_for_machine(store, &machine_id) {
            Ok(text) => {
                let delivered = registry.send_to(&machine_id, &text);
                debug!(
                    delivered,
                    machine_id = %machine_id,
                    "bridge_ws: pushed scoped bridge.target on change"
                );
            }
            Err(err) => {
                warn!(
                    machine_id = %machine_id,
                    error = %err,
                    "bridge_ws: failed to build scoped target frame"
                );
            }
        }
    }
}

/// Forward a chat-message stream event to every connected bridge as a
/// `chat.message.received` frame. Called wherever a `message.created`
/// `StreamEvent` is published. Slice 5 broadcasts to every bridge for
/// every agent member of the channel; slice 6 narrows by `machine_id`
/// so each bridge only sees chat for the agents it owns.
///
/// The bridge is responsible for matching the inner `agent_id` to its
/// own running runtime and updating that mailbox.
pub fn forward_chat_event_to_bridges(
    store: &Store,
    registry: &BridgeRegistry,
    event: &crate::store::stream::StreamEvent,
) {
    if event.event_type != "message.created" {
        return;
    }
    let members = match store.get_channel_members(&event.channel_id) {
        Ok(m) => m,
        Err(err) => {
            warn!(
                channel_id = %event.channel_id,
                error = %err,
                "bridge_ws: failed to read channel members for chat forward"
            );
            return;
        }
    };
    let agent_recipients: Vec<&str> = members
        .iter()
        .filter(|m| matches!(m.member_type, crate::store::messages::SenderType::Agent))
        .map(|m| m.member_id.as_str())
        .collect();
    if agent_recipients.is_empty() {
        return;
    }
    for agent_id in agent_recipients {
        let frame = match build_chat_message_frame_text(agent_id, event) {
            Ok(t) => t,
            Err(err) => {
                warn!(error = %err, "bridge_ws: failed to build chat frame");
                continue;
            }
        };
        // Scope by the agent's owning machine_id when set. Owner-less
        // agents (machine_id NULL) fan to every bridge — back-compat
        // with pre-slice-6 behavior + the explicit "any bridge can run
        // this agent" affordance.
        let agent_record = store.get_agent_by_id(agent_id, false).ok().flatten();
        let owner_machine_id = agent_record.and_then(|a| a.machine_id);
        let delivered = match owner_machine_id.as_deref() {
            Some(m) => registry.send_to(m, &frame),
            None => registry.broadcast(&frame),
        };
        debug!(
            delivered,
            agent_id = %agent_id,
            seq = event.latest_seq,
            owner = owner_machine_id.as_deref().unwrap_or("<any>"),
            "bridge_ws: pushed chat.message.received"
        );
    }
}

fn build_chat_message_frame_text(
    agent_id: &str,
    event: &crate::store::stream::StreamEvent,
) -> anyhow::Result<String> {
    let payload = serde_json::json!({
        "agent_id": agent_id,
        "channel_id": event.channel_id,
        "seq": event.latest_seq,
        "schema_version": event.schema_version,
        "messages": [event.event_payload.clone()],
    });
    let envelope = WireFrame {
        v: 1,
        frame_type: "chat.message.received".to_string(),
        data: payload,
    };
    Ok(serde_json::to_string(&envelope)?)
}
