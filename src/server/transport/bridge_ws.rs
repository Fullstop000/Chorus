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
//! `api_tokens` rows minted via `mint_bridge_token`), the request
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
    AgentDecisionDelivered, AgentRestart, AgentStart, AgentState as AgentStatePayload, AgentStop,
    AgentTarget, BridgeTarget, ChatAck as ChatAckPayload, DecisionResume, EnvVar,
    Hello as BridgeHello, WireFrame, FRAME_AGENT_DECISION_DELIVERED, FRAME_AGENT_RESTART,
    FRAME_AGENT_START, FRAME_AGENT_STATE, FRAME_AGENT_STOP, FRAME_BRIDGE_HELLO,
    FRAME_BRIDGE_TARGET, FRAME_CHAT_ACK, FRAME_CHAT_MESSAGE_RECEIVED,
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
    }
}

/// What kind of authentication context the WS upgrade carried. Drives
/// the post-hello dispatch in `bridge_session`.
enum BridgeAuthContext {
    /// No bridge tokens exist in the DB yet — passthrough.
    Passthrough,
    /// Legacy per-machine bridge token. `hello.machine_id` must equal
    /// the token's bound machine_id.
    PerMachine { expected_machine_id: String },
    /// User-scoped bridge token. The hello's machine_id is authoritative
    /// and is recorded in `bridge_machines`.
    UserScoped { token_hash: String },
}

pub async fn handle_bridge_ws(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> axum::response::Response {
    let auth_ctx = match crate::server::bridge_auth::check(state.store.as_ref(), &headers) {
        AuthOutcome::Disabled => BridgeAuthContext::Passthrough,
        AuthOutcome::Allowed {
            expected_machine_id,
        } => BridgeAuthContext::PerMachine {
            expected_machine_id,
        },
        AuthOutcome::UserBridgeAllowed { token_hash, .. } => {
            BridgeAuthContext::UserScoped { token_hash }
        }
        AuthOutcome::CliAllowed { .. } => {
            warn!("bridge_ws: rejecting upgrade — CLI token cannot open bridge session");
            return (
                StatusCode::FORBIDDEN,
                "CLI token not allowed on bridge upgrade",
            )
                .into_response();
        }
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
            auth_ctx,
        )
    })
    .into_response()
}

/// WebSocket close codes specific to this PRD. Extends the catalog in
/// `docs/plan/bridge-platform-protocol.md` §4.
const CLOSE_CODE_KICKED: u16 = 4004;

async fn close_with(socket: &mut WebSocket, code: u16, reason: &'static str) {
    use axum::extract::ws::CloseFrame;
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
}

async fn bridge_session(
    mut socket: WebSocket,
    store: Arc<Store>,
    registry: Arc<BridgeRegistry>,
    auth_ctx: BridgeAuthContext,
) {
    let hello = match recv_hello(&mut socket).await {
        Ok(h) => h,
        Err(err) => {
            warn!(error = %err, "bridge_ws: hello handshake failed");
            return;
        }
    };

    // Auth dispatch by token shape.
    let user_scoped_token_hash: Option<String> = match &auth_ctx {
        BridgeAuthContext::Passthrough => None,
        BridgeAuthContext::PerMachine {
            expected_machine_id,
        } => {
            if hello.machine_id != *expected_machine_id {
                warn!(
                    token_bound_machine_id = %expected_machine_id,
                    claimed_machine_id = %hello.machine_id,
                    "bridge_ws: dropping connection — bridge.hello.machine_id does not match token binding"
                );
                return;
            }
            None
        }
        BridgeAuthContext::UserScoped { token_hash } => {
            // Apply the bridge_machines state machine. The frame's
            // machine_id is authoritative; we record it.
            match store.register_bridge_machine_hello(
                token_hash,
                &hello.machine_id,
                Some(&hello.machine_id),
            ) {
                Ok((_, outcome)) => match outcome {
                    crate::store::auth::HelloOutcome::Rejected => {
                        warn!(
                            machine_id = %hello.machine_id,
                            "bridge_ws: rejecting hello — bridge_machines row is kicked"
                        );
                        close_with(
                            &mut socket,
                            CLOSE_CODE_KICKED,
                            "device kicked from Settings → Devices",
                        )
                        .await;
                        return;
                    }
                    other => {
                        debug!(
                            machine_id = %hello.machine_id,
                            outcome = ?other,
                            "bridge_ws: user-scoped bridge hello accepted"
                        );
                    }
                },
                Err(err) => {
                    warn!(error = %err, "bridge_ws: bridge_machines register failed");
                    return;
                }
            }
            Some(token_hash.clone())
        }
    };

    info!(
        machine_id = %hello.machine_id,
        bridge_version = %hello.bridge_version,
        agents_alive = hello.agents_alive.len(),
        user_scoped = user_scoped_token_hash.is_some(),
        "bridge_ws: bridge connected"
    );

    let machine_id = hello.machine_id.clone();
    let (mut outbound_rx, _registration) = registry.register(&machine_id);

    if let Err(err) = send_initial_target(&mut socket, store.as_ref(), &machine_id).await {
        warn!(machine_id = %machine_id, error = %err, "bridge_ws: failed to send initial bridge.target");
        return;
    }

    // Reconnect replay: agents owned by this bridge that have a pending
    // wake reason (unread inbox messages OR an undelivered resolved
    // decision) need a `agent.start` so the bridge launches them. This
    // is what makes the platform a real source of truth for "should be
    // running": the agent's row carries no `paused` / `restart_seq` /
    // `pending_init_directive` flags — instead we derive intent from
    // unread + decision state every time a bridge connects.
    if let Err(err) = replay_pending_starts(&mut socket, store.as_ref(), &machine_id).await {
        warn!(machine_id = %machine_id, error = %err, "bridge_ws: reconnect replay failed; bridge will only wake on chat");
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
                            handle_inbound_frame(&machine_id, text.as_str(), registry.as_ref(), store.as_ref()).await
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

    // For user-scoped bridge tokens, stamp `disconnected_at` so Settings
    // → Devices reflects the offline state immediately. Idempotent.
    if let Some(ref token_hash) = user_scoped_token_hash {
        if let Err(err) = store.mark_bridge_machine_disconnected(token_hash, &machine_id) {
            warn!(
                machine_id = %machine_id,
                error = %err,
                "bridge_ws: failed to mark bridge_machines disconnected"
            );
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

/// After the initial `bridge.target` snapshot, walk the agents owned by
/// this bridge and emit `agent.start` for any that have a pending wake:
/// unread messages, or a resolved-not-delivered decision (whose envelope
/// becomes `init_directive`). This restores "agent resumes when the
/// bridge comes back" without keeping a `paused` flag — the wake is
/// derived from persistent inbox + decision state every reconnect.
async fn replay_pending_starts(
    socket: &mut WebSocket,
    store: &Store,
    machine_id: &str,
) -> anyhow::Result<()> {
    let owned: Vec<Agent> = store
        .get_agents()?
        .into_iter()
        .filter(|a| a.machine_id == machine_id)
        .collect();
    for agent in owned {
        let resume = match store.latest_undelivered_resolved_for_agent(&agent.id)? {
            Some(row) => crate::server::handlers::decisions::build_resume_envelope_from_row(&row)
                .map(|prompt| DecisionResume {
                    decision_id: row.id.clone(),
                    prompt,
                }),
            None => None,
        };
        let has_unread = !store.get_unread_summary(&agent.id)?.is_empty();
        if resume.is_none() && !has_unread {
            continue;
        }
        let had_resume = resume.is_some();
        let payload = AgentStart {
            agent_id: agent.id.clone(),
            decision_resume: resume,
        };
        let text = build_lifecycle_frame_text(FRAME_AGENT_START, serde_json::to_value(&payload)?)?;
        socket.send(Message::Text(text.into())).await?;
        debug!(
            machine_id = %machine_id,
            agent_id = %agent.id,
            had_resume,
            has_unread,
            "bridge_ws: replayed agent.start on hello"
        );
    }
    Ok(())
}

async fn handle_inbound_frame(
    machine_id: &str,
    text: &str,
    registry: &BridgeRegistry,
    store: &Store,
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
        FRAME_AGENT_DECISION_DELIVERED => {
            let payload: AgentDecisionDelivered = serde_json::from_value(envelope.data)
                .map_err(|e| anyhow::anyhow!("failed to parse agent.decision_delivered: {e}"))?;
            // Mark exactly the named directive as delivered. The bridge
            // echoes the `directive_id` from the start/restart frame, so
            // a rapid re-emit of a different directive can't be falsely
            // marked delivered by an earlier ack.
            match store.mark_decision_delivered(&payload.decision_id) {
                Ok(n) => debug!(
                    machine_id = %machine_id,
                    agent_id = %payload.agent_id,
                    directive_id = %payload.decision_id,
                    delivered = n,
                    "bridge_ws: marked decision delivered after directive_consumed"
                ),
                Err(err) => warn!(
                    machine_id = %machine_id,
                    agent_id = %payload.agent_id,
                    directive_id = %payload.decision_id,
                    error = %err,
                    "bridge_ws: failed to mark decision delivered"
                ),
            }
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
/// agents whose `machine_id` matches are included.
///
/// Every agent has exactly one owner; fanning agents to bridges that
/// don't own them would cause dual-runtime contention.
pub fn build_target_frame_text_for_machine(
    store: &Store,
    machine_id: &str,
) -> anyhow::Result<String> {
    let agents: Vec<Agent> = store
        .get_agents()?
        .into_iter()
        .filter(|a| a.machine_id == machine_id)
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

/// Build a serialized lifecycle RPC frame (`agent.start`, `agent.stop`,
/// `agent.restart`).
fn build_lifecycle_frame_text(frame_type: &str, data: serde_json::Value) -> anyhow::Result<String> {
    let envelope = WireFrame {
        v: 1,
        frame_type: frame_type.to_string(),
        data,
    };
    Ok(serde_json::to_string(&envelope)?)
}

/// Find the bridge that owns this agent and send an `agent.start` frame.
/// Returns `Ok(true)` if the owning bridge is connected and the frame was
/// queued; `Ok(false)` if the bridge is offline (the platform's persisted
/// state — unread messages, undelivered decisions — covers reconnect
/// replay so this is not an error).
pub fn dispatch_agent_start(
    store: &Store,
    registry: &BridgeRegistry,
    agent_id: &str,
    decision_resume: Option<DecisionResume>,
) -> anyhow::Result<bool> {
    let owner = match resolve_owner(store, agent_id)? {
        Some(o) => o,
        None => return Ok(false),
    };
    let payload = AgentStart {
        agent_id: agent_id.to_string(),
        decision_resume,
    };
    let text = build_lifecycle_frame_text(FRAME_AGENT_START, serde_json::to_value(&payload)?)?;
    Ok(registry.send_to(&owner, &text) > 0)
}

pub fn dispatch_agent_stop(
    store: &Store,
    registry: &BridgeRegistry,
    agent_id: &str,
) -> anyhow::Result<bool> {
    let owner = match resolve_owner(store, agent_id)? {
        Some(o) => o,
        None => return Ok(false),
    };
    let payload = AgentStop {
        agent_id: agent_id.to_string(),
    };
    let text = build_lifecycle_frame_text(FRAME_AGENT_STOP, serde_json::to_value(&payload)?)?;
    Ok(registry.send_to(&owner, &text) > 0)
}

pub fn dispatch_agent_restart(
    store: &Store,
    registry: &BridgeRegistry,
    agent_id: &str,
    decision_resume: Option<DecisionResume>,
) -> anyhow::Result<bool> {
    let owner = match resolve_owner(store, agent_id)? {
        Some(o) => o,
        None => return Ok(false),
    };
    let payload = AgentRestart {
        agent_id: agent_id.to_string(),
        decision_resume,
    };
    let text = build_lifecycle_frame_text(FRAME_AGENT_RESTART, serde_json::to_value(&payload)?)?;
    Ok(registry.send_to(&owner, &text) > 0)
}

fn resolve_owner(store: &Store, agent_id: &str) -> anyhow::Result<Option<String>> {
    Ok(store
        .get_agent_by_id(agent_id, false)?
        .map(|a| a.machine_id))
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
/// recipient agent. Called from the event-bus subscriber spawned in
/// `build_router_with_services_and_auth`, fired on every `message.created`
/// event. Routes by the agent's `machine_id`; for `chorus serve` that
/// includes the in-process bridge client (registered under
/// `local_machine_id`). This is the only agent-delivery path — there
/// is no platform-local fallback.
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
        // Route to the bridge that owns this agent. The bridge that
        // registered with the matching `machine_id` — possibly the
        // in-process one inside `chorus serve` — receives the frame.
        // A missing record (deleted between channel-member lookup and
        // here) silently skips this recipient.
        let Some(agent) = store.get_agent_by_id(agent_id, false).ok().flatten() else {
            continue;
        };
        let owner_machine_id = agent.machine_id;
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
