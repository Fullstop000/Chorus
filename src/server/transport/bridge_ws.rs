//! Bridge ↔ Platform WebSocket handler.
//!
//! `GET /api/bridge/ws`. The bridge dials in, sends `bridge.hello` as
//! its first frame; the platform replies with `bridge.target` listing
//! the desired runtime config for every agent that should run on this
//! bridge. The platform pushes a fresh `bridge.target` on every change
//! to desired state (see `broadcast_target_update`). The bridge sends
//! `agent.state` frames upstream when its runtimes transition, and
//! `chat.ack` after delivering a `chat.message.received` to the runtime.
//!
//! Frames carry a `runtime_pid` instance discriminator: `started` events
//! record the current pid in `BridgeRegistry`, and non-started transitions
//! are dropped if their pid doesn't match. This blocks the stop→start
//! race where a delayed `crashed` from the previous instance would
//! otherwise silently mark the live new instance dead.
//!
//! Auth: when the platform has tokens configured (via
//! `CHORUS_BRIDGE_TOKENS` env var or explicit `BridgeAuth`), the request
//! must include `Authorization: Bearer <token>` and the
//! `bridge.hello.machine_id` must match the token's bound `machine_id`.
//! With no tokens configured, auth is disabled (loopback default).

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use tracing::{debug, info, warn};

use crate::bridge::protocol::{
    AgentState as AgentStatePayload, AgentTarget, BridgeTarget, ChatAck as ChatAckPayload, EnvVar,
    Hello as BridgeHello, WireFrame, FRAME_AGENT_STATE, FRAME_BRIDGE_HELLO, FRAME_BRIDGE_TARGET,
    FRAME_CHAT_ACK, FRAME_CHAT_MESSAGE_RECEIVED,
};
use crate::server::bridge_auth::AuthOutcome;
use crate::server::bridge_registry::BridgeRegistry;
use crate::server::handlers::AppState;
use crate::store::agents::Agent;
use crate::store::Store;

/// Convert a platform `Agent` row into an outbound `AgentTarget` payload.
/// Lives here (not in `protocol.rs`) so the protocol module stays free of
/// store types — the wire shape is shared, but the source-of-truth
/// hydration is platform-side.
fn agent_to_target(a: Agent) -> AgentTarget {
    AgentTarget {
        agent_id: a.id,
        name: a.name,
        display_name: a.display_name,
        runtime: a.runtime,
        model: a.model,
        description: a.description,
        system_prompt: a.system_prompt,
        reasoning_effort: a.reasoning_effort,
        env_vars: a
            .env_vars
            .into_iter()
            .map(|e| EnvVar {
                key: e.key,
                value: e.value,
            })
            .collect(),
        init_directive: None,
        pending_prompt: None,
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
    if envelope.frame_type != FRAME_BRIDGE_HELLO {
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
        FRAME_AGENT_STATE => {
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
        FRAME_CHAT_ACK => {
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
        // Bridge re-sending hello is allowed for self-correction. Treat
        // as a no-op today; a future change can trigger a fresh target
        // push in response.
        FRAME_BRIDGE_HELLO => {
            debug!(machine_id = %machine_id, "bridge_ws: re-hello received, ignoring");
            Ok(())
        }
        other => {
            anyhow::bail!("unknown inbound frame type: {other}");
        }
    }
}

/// Build a serialized `bridge.target` frame from the current set of
/// agents in the store, scoped to a specific bridge `machine_id`. Only
/// agents explicitly bound to this bridge are included; agents with
/// NULL `machine_id` are platform-local and never sent to any bridge.
///
/// Every agent has exactly one owner (the platform itself, or one named
/// bridge). Fanning NULL agents to all bridges would cause dual-runtime
/// contention as soon as a second bridge connects.
pub fn build_target_frame_text_for_machine(
    store: &Store,
    machine_id: &str,
) -> anyhow::Result<String> {
    let agents: Vec<Agent> = store
        .get_agents()?
        .into_iter()
        .filter(|a| a.machine_id.as_deref() == Some(machine_id))
        .collect();
    let target = BridgeTarget {
        target_agents: agents.into_iter().map(agent_to_target).collect(),
    };
    let envelope = WireFrame {
        v: 1,
        frame_type: FRAME_BRIDGE_TARGET.to_string(),
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

/// Forward a chat-message stream event to the bridge that owns each
/// recipient agent. Called wherever a `message.created` `StreamEvent`
/// is published. Routes by the agent's `machine_id`; platform-local
/// agents (NULL `machine_id`) are skipped — they're delivered by the
/// platform's own `AgentManager` in `deliver_message_to_agents`.
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
        // Route only to the bridge that owns this agent. Platform-local
        // agents (machine_id NULL) are delivered by the platform's own
        // AgentManager via deliver_message_to_agents — they have no
        // bridge to push to.
        let agent_record = store.get_agent_by_id(agent_id, false).ok().flatten();
        let Some(owner_machine_id) = agent_record.and_then(|a| a.machine_id) else {
            continue;
        };
        let frame = match build_chat_message_frame_text(agent_id, event) {
            Ok(t) => t,
            Err(err) => {
                warn!(error = %err, "bridge_ws: failed to build chat frame");
                continue;
            }
        };
        let delivered = registry.send_to(&owner_machine_id, &frame);
        debug!(
            delivered,
            agent_id = %agent_id,
            seq = event.latest_seq,
            owner = %owner_machine_id,
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
        frame_type: FRAME_CHAT_MESSAGE_RECEIVED.to_string(),
        data: payload,
    };
    Ok(serde_json::to_string(&envelope)?)
}
