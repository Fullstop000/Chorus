//! WebSocket client for the bridge↔platform protocol.
//!
//! Exchanges:
//!   - `bridge.hello` (B→P, on connect)
//!   - `bridge.target` (P→B, reply + on every CRUD)
//!   - `agent.state` (B→P, on transitions; first event after start carries pid)
//!   - `chat.message.received` (P→B, when a message lands for an agent on this machine)
//!   - `chat.ack` (B→P, after the bridge has handed the message to the agent)

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::agent::drivers::ProcessState;
use crate::agent::manager::AgentManager;
use crate::bridge::protocol::{
    AgentRestart, AgentStart, AgentStop, WireFrame, FRAME_AGENT_DECISION_DELIVERED,
    FRAME_AGENT_RESTART, FRAME_AGENT_START, FRAME_AGENT_STATE, FRAME_AGENT_STOP,
    FRAME_BRIDGE_HELLO, FRAME_BRIDGE_TARGET, FRAME_CHAT_ACK, FRAME_CHAT_MESSAGE_RECEIVED,
};

use super::reconcile;
use super::BridgeClientConfig;

/// Per-agent instance counter the bridge advances on every `start_agent`.
/// The platform side uses `runtime_pid` only to disambiguate stop→start
/// races, not as a real OS pid. A monotonic counter is sufficient.
///
/// Pid 0 is reserved as "no instance recorded" — `current` returns it for
/// agents we haven't allocated for yet. `allocate` starts at 1 so the
/// reserved value never collides with a live id.
#[derive(Default)]
struct InstanceCounter {
    next: AtomicU32,
    by_agent_id: Mutex<HashMap<String, u32>>,
}

impl InstanceCounter {
    async fn allocate(&self, agent_id: &str) -> u32 {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.by_agent_id
            .lock()
            .await
            .insert(agent_id.to_string(), id);
        id
    }

    async fn current(&self, agent_id: &str) -> u32 {
        self.by_agent_id
            .lock()
            .await
            .get(agent_id)
            .copied()
            .unwrap_or(0)
    }

    async fn forget(&self, agent_id: &str) {
        self.by_agent_id.lock().await.remove(agent_id);
    }
}

const SUPPORTED_FRAMES: &[&str] = &[
    FRAME_BRIDGE_TARGET,
    FRAME_AGENT_START,
    FRAME_AGENT_STOP,
    FRAME_AGENT_RESTART,
    FRAME_CHAT_MESSAGE_RECEIVED,
];
const BRIDGE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Initial reconnect delay after a WS session ends. Doubles up to
/// [`MAX_BACKOFF_MS`] after each failure; resets to this on a clean exit.
const INITIAL_BACKOFF_MS: u64 = 500;
/// Cap on reconnect delay — 30 s strikes a balance between not hammering
/// a degraded platform and not waiting forever after the platform recovers.
const MAX_BACKOFF_MS: u64 = 30_000;
/// Cadence the bridge polls each agent's `process_state` and emits
/// transitions upstream. Bridge runtimes don't expose a transition stream
/// today, so this is the polling tax.
const STATE_PUSHER_INTERVAL: Duration = Duration::from_millis(500);

// All wire payload types live in `crate::bridge::protocol` and are
// re-exported here for the rest of the bridge client to consume.
pub(super) use crate::bridge::protocol::{
    AgentTarget as AgentTargetIn, BridgeTarget as BridgeTargetIn, ChatMessageReceived,
};

/// Cap on chats buffered per (still-unknown) platform agent_id. The
/// bridge sees `chat.message.received` for a platform agent before
/// `bridge.target` creates the local agent row when the platform fires
/// channel-join stream events ahead of the broadcast_target_update on
/// agent create. Buffered frames replay on the next `bridge.target`
/// once the agent is known. The cap stops a misrouted-chat storm from
/// pinning unbounded memory.
const PENDING_CHATS_PER_AGENT: usize = 32;

/// Per-platform-agent_id buffer of `chat.message.received` frames that
/// arrived before the bridge knew about the agent. Drained on every
/// `handle_target` that adds a new local agent row.
type PendingChats = Arc<Mutex<HashMap<String, VecDeque<ChatMessageReceived>>>>;

/// In-memory snapshot of the most recent `bridge.target` payload, keyed
/// by `agent_id`. The authoritative spec/identity view on the bridge side:
/// `start_agent` reads the spec from here, not SQLite. Renames update the
/// cached `name` field in place; the id key is stable. The bridge's
/// `agents` table is empty by design — agent rows live only on the platform.
#[derive(Default)]
pub(super) struct TargetCache {
    by_agent_id: HashMap<String, AgentTargetIn>,
}

impl TargetCache {
    pub(super) fn upsert(&mut self, target: AgentTargetIn) {
        self.by_agent_id.insert(target.agent_id.clone(), target);
    }

    pub(super) fn forget(&mut self, agent_id: &str) -> Option<AgentTargetIn> {
        self.by_agent_id.remove(agent_id)
    }

    pub(super) fn get(&self, agent_id: &str) -> Option<&AgentTargetIn> {
        self.by_agent_id.get(agent_id)
    }
}

/// Shared session-scoped state passed to every frame handler. All fields
/// are `Arc`-backed (or `Sender`-cloneable), so `Clone` is cheap and lets
/// each spawned handler task own a snapshot without lifetime juggling.
/// Adding a new B→P or P→B frame doesn't grow the handler signature —
/// new fields land here once.
#[derive(Clone)]
struct SessionCtx {
    store: Arc<crate::store::Store>,
    manager: Arc<AgentManager>,
    counter: Arc<InstanceCounter>,
    pending_chats: PendingChats,
    targets: Arc<Mutex<TargetCache>>,
    state_tx: tokio::sync::mpsc::Sender<String>,
}

// ── Outbound frame helpers ────────────────────────────────────────────────
//
// Each B→P frame goes through `queue_frame`: serialise once, push via
// `try_send`, log on overflow. Using `try_send` (rather than `.send().await`)
// here matters because all callers of these helpers run on tasks spawned
// from the WS select-loop; a blocking await would create the same kind of
// hold-the-loop deadlock the spawn-handlers refactor was designed to avoid.

/// Send an `agent.state` frame upstream. Drops the frame with a warning
/// when the outbound channel is full (the platform will eventually re-sync
/// via the next reconcile round-trip).
async fn send_agent_state(
    state_tx: &tokio::sync::mpsc::Sender<String>,
    platform_id: &str,
    state: &str,
    runtime_pid: u32,
) {
    queue_frame(
        state_tx,
        FRAME_AGENT_STATE,
        json!({
            "agent_id": platform_id,
            "state": state,
            "runtime_pid": runtime_pid,
            "ts": chrono::Utc::now().to_rfc3339(),
        }),
    )
    .await;
}

/// Send a `chat.ack` frame upstream after a chat batch was queued for
/// the local agent. See `send_agent_state` for overflow semantics.
async fn send_chat_ack(
    state_tx: &tokio::sync::mpsc::Sender<String>,
    platform_id: &str,
    last_seq: i64,
) {
    queue_frame(
        state_tx,
        FRAME_CHAT_ACK,
        json!({
            "agent_id": platform_id,
            "last_seq": last_seq,
            "ts": chrono::Utc::now().to_rfc3339(),
        }),
    )
    .await;
}

/// Send `agent.decision_delivered` upstream after an `agent.start` /
/// `agent.restart` frame's directive was passed into a fresh runtime
/// instance. The platform marks exactly the named directive as delivered;
/// echoing `directive_id` (not just `agent_id`) avoids the race where a
/// rapid re-emit of a different directive would be falsely marked
/// delivered by an earlier ack.
async fn send_decision_delivered(
    state_tx: &tokio::sync::mpsc::Sender<String>,
    platform_id: &str,
    directive_id: &str,
) {
    queue_frame(
        state_tx,
        FRAME_AGENT_DECISION_DELIVERED,
        json!({ "agent_id": platform_id, "directive_id": directive_id }),
    )
    .await;
}

async fn queue_frame(
    state_tx: &tokio::sync::mpsc::Sender<String>,
    frame_type: &'static str,
    data: Value,
) {
    let frame = WireFrame {
        v: 1,
        frame_type: frame_type.to_string(),
        data,
    };
    let text = match serde_json::to_string(&frame) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(err = %e, frame_type, "bridge: failed to encode outbound frame");
            return;
        }
    };
    match state_tx.try_send(text) {
        Ok(()) => {}
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            // Outbound channel is bounded (cap 64). On overflow we drop
            // and surface the loss — the alternative (blocking await) is
            // the deadlock pattern the per-frame spawn refactor avoided.
            tracing::warn!(frame_type, "bridge: outbound channel full; frame dropped");
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            // Session is shutting down; nothing actionable.
        }
    }
}

pub async fn run_ws_client_loop(
    cfg: BridgeClientConfig,
    manager: Arc<AgentManager>,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let counter = Arc::new(InstanceCounter::default());
    let pending_chats: PendingChats = Arc::new(Mutex::new(HashMap::new()));
    let targets = Arc::new(Mutex::new(TargetCache::default()));

    let mut backoff_ms: u64 = INITIAL_BACKOFF_MS;
    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }

        // Clear stale pending chats on each reconnect: the platform will
        // re-emit anything we still need on its next push, and chats from
        // before the disconnect are already on the platform side. The
        // targets cache is preserved — if the platform's view hasn't
        // changed, we want the bridge to keep serving the same set; the
        // first `bridge.target` after reconnect refreshes any drift.
        pending_chats.lock().await.clear();

        match run_one_session(
            &cfg,
            manager.clone(),
            counter.clone(),
            pending_chats.clone(),
            targets.clone(),
            shutdown.clone(),
        )
        .await
        {
            Ok(()) => {
                tracing::info!("bridge: WS session ended cleanly; reconnecting");
                backoff_ms = INITIAL_BACKOFF_MS;
            }
            Err(e) => {
                tracing::warn!(err = %e, backoff_ms, "bridge: WS session failed; reconnecting");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(backoff_ms)) => {}
                    _ = shutdown.cancelled() => return Ok(()),
                }
                backoff_ms = (backoff_ms * 2).min(MAX_BACKOFF_MS);
            }
        }
    }
}

async fn run_one_session(
    cfg: &BridgeClientConfig,
    manager: Arc<AgentManager>,
    counter: Arc<InstanceCounter>,
    pending_chats: PendingChats,
    targets: Arc<Mutex<TargetCache>>,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    // The session-level state (counter, pending_chats, targets) is owned by
    // run_ws_client_loop and reused across reconnects so a brief drop
    // doesn't lose pid bookkeeping or the desired-state snapshot. The
    // state_tx channel is per-session — we re-create it here and bundle
    // into a SessionCtx after the handshake.
    let mut request = cfg.platform_ws.clone().into_client_request()?;
    if let Some(token) = cfg.token.as_deref() {
        let header_value = format!("Bearer {token}");
        request.headers_mut().insert(
            "authorization",
            tokio_tungstenite::tungstenite::http::HeaderValue::from_str(&header_value)?,
        );
    }

    tracing::info!(url = %cfg.platform_ws, "bridge: dialing platform");
    let (ws_stream, _) = tokio_tungstenite::connect_async(request).await?;
    let (mut write, mut read) = ws_stream.split();

    // 1. Send bridge.hello with currently-known agents.
    let agents_alive = build_agents_alive(&manager, &targets, &counter).await;
    let hello = WireFrame {
        v: 1,
        frame_type: FRAME_BRIDGE_HELLO.into(),
        data: json!({
            "machine_id": cfg.machine_id,
            "bridge_version": BRIDGE_VERSION,
            "supported_frames": SUPPORTED_FRAMES,
            "agents_alive": agents_alive,
        }),
    };
    write
        .send(Message::Text(serde_json::to_string(&hello)?.into()))
        .await?;
    tracing::info!(machine = %cfg.machine_id, "bridge: hello sent");

    // 2. Bundle session-scoped state into a SessionCtx. All fields are
    // Arc / Sender, so cloning is cheap; each spawned handler captures a
    // private clone via `async move`.
    let (state_tx, mut state_rx) = tokio::sync::mpsc::channel::<String>(64);
    let ctx = SessionCtx {
        store: cfg.store.clone(),
        manager,
        counter,
        pending_chats,
        targets,
        state_tx,
    };

    // 3. Spawn the agent.state pusher task.
    let pusher_ctx = ctx.clone();
    let pusher_shutdown = shutdown.clone();
    let pusher = tokio::spawn(async move {
        agent_state_pusher(pusher_ctx, pusher_shutdown).await;
    });

    // 4. Drive the read+write select loop.
    //
    // Frame handlers (`handle_target`, `handle_chat`) are dispatched to
    // independent tasks so the select arm returns immediately. If we
    // awaited them inline they could call `state_tx.send().await` on the
    // bounded channel — when the channel is full (state_pusher tail +
    // burst of inbound frames), that .await blocks the select loop, which
    // means `state_rx.recv()` never runs to drain the channel: a deadlock.
    let mut handler_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    loop {
        // Drop finished handler tasks so the vec doesn't grow unboundedly.
        handler_tasks.retain(|h| !h.is_finished());
        tokio::select! {
            _ = shutdown.cancelled() => {
                tracing::info!("bridge: shutdown signaled");
                break;
            }
            Some(frame_text) = state_rx.recv() => {
                if let Err(e) = write.send(Message::Text(frame_text.into())).await {
                    return Err(anyhow::anyhow!("send agent.state: {e}"));
                }
            }
            msg = read.next() => {
                let Some(msg) = msg else {
                    return Err(anyhow::anyhow!("WS read returned None (peer closed)"));
                };
                let msg = msg?;
                match msg {
                    Message::Text(text) => {
                        let frame: WireFrame = match serde_json::from_str(&text) {
                            Ok(f) => f,
                            Err(e) => {
                                tracing::warn!(err = %e, raw = %text, "bridge: malformed frame");
                                continue;
                            }
                        };
                        match frame.frame_type.as_str() {
                            FRAME_BRIDGE_TARGET => {
                                let target: BridgeTargetIn = match serde_json::from_value(frame.data) {
                                    Ok(t) => t,
                                    Err(e) => {
                                        tracing::warn!(err = %e, "bridge: bad bridge.target payload");
                                        continue;
                                    }
                                };
                                let ctx = ctx.clone();
                                handler_tasks.push(tokio::spawn(async move {
                                    handle_target(&ctx, target).await;
                                }));
                            }
                            FRAME_CHAT_MESSAGE_RECEIVED => {
                                let payload: ChatMessageReceived = match serde_json::from_value(frame.data) {
                                    Ok(p) => p,
                                    Err(e) => {
                                        tracing::warn!(err = %e, "bridge: bad chat.message.received");
                                        continue;
                                    }
                                };
                                let ctx = ctx.clone();
                                handler_tasks.push(tokio::spawn(async move {
                                    handle_chat(&ctx, payload).await;
                                }));
                            }
                            FRAME_AGENT_START => {
                                let payload: AgentStart = match serde_json::from_value(frame.data) {
                                    Ok(p) => p,
                                    Err(e) => {
                                        tracing::warn!(err = %e, "bridge: bad agent.start");
                                        continue;
                                    }
                                };
                                let ctx = ctx.clone();
                                handler_tasks.push(tokio::spawn(async move {
                                    handle_agent_start_frame(&ctx, payload).await;
                                }));
                            }
                            FRAME_AGENT_STOP => {
                                let payload: AgentStop = match serde_json::from_value(frame.data) {
                                    Ok(p) => p,
                                    Err(e) => {
                                        tracing::warn!(err = %e, "bridge: bad agent.stop");
                                        continue;
                                    }
                                };
                                let ctx = ctx.clone();
                                handler_tasks.push(tokio::spawn(async move {
                                    handle_agent_stop_frame(&ctx, payload).await;
                                }));
                            }
                            FRAME_AGENT_RESTART => {
                                let payload: AgentRestart = match serde_json::from_value(frame.data) {
                                    Ok(p) => p,
                                    Err(e) => {
                                        tracing::warn!(err = %e, "bridge: bad agent.restart");
                                        continue;
                                    }
                                };
                                let ctx = ctx.clone();
                                handler_tasks.push(tokio::spawn(async move {
                                    handle_agent_restart_frame(&ctx, payload).await;
                                }));
                            }
                            other => {
                                tracing::debug!(kind = %other, "bridge: ignoring unknown frame");
                            }
                        }
                    }
                    Message::Close(_) => {
                        tracing::info!("bridge: peer sent Close");
                        break;
                    }
                    Message::Ping(p) => {
                        let _ = write.send(Message::Pong(p)).await;
                    }
                    _ => {}
                }
            }
        }
    }

    pusher.abort();
    for h in handler_tasks {
        h.abort();
    }
    Ok(())
}

async fn build_agents_alive(
    manager: &Arc<AgentManager>,
    targets: &Arc<Mutex<TargetCache>>,
    counter: &Arc<InstanceCounter>,
) -> Vec<Value> {
    let ids = manager.get_running_agent_ids().await;
    let cache = targets.lock().await;
    let mut out = Vec::with_capacity(ids.len());
    for agent_id in ids {
        if cache.get(&agent_id).is_none() {
            // Agent is running locally but no longer in the desired-state
            // snapshot — happens transiently on first reconcile after
            // reconnect. Skip; the next bridge.target restores it.
            continue;
        }
        let pid = counter.current(&agent_id).await;
        out.push(json!({
            "agent_id": agent_id,
            "state": "started",
            "runtime_pid": pid,
        }));
    }
    out
}

async fn handle_target(ctx: &SessionCtx, target: BridgeTargetIn) {
    let target_agents = target.target_agents;
    let known_platform_ids: Vec<String> =
        target_agents.iter().map(|a| a.agent_id.clone()).collect();
    let outcome =
        match reconcile::apply(&ctx.store, &ctx.manager, &ctx.targets, target_agents).await {
            Ok(o) => o,
            Err(e) => {
                tracing::error!(err = %e, "bridge: reconcile failed");
                return;
            }
        };

    // Replay any chats that arrived before this reconcile populated the
    // targets cache. We only drain entries for platform_ids that are now
    // in the target — others remain buffered (or stay until they age out
    // via reconnect). Replays are dispatched as detached tasks so this
    // handler can return to the select loop quickly.
    let replays: Vec<ChatMessageReceived> = {
        let mut buf = ctx.pending_chats.lock().await;
        let mut out = Vec::new();
        for pid in &known_platform_ids {
            if let Some(queue) = buf.remove(pid) {
                out.extend(queue);
            }
        }
        out
    };
    for chat in replays {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            handle_chat(&ctx, chat).await;
        });
    }

    // Reconcile only stops orphans (agents removed from the desired set).
    // Agent starts come via discrete `agent.start` / `agent.restart`
    // frames or chat-driven wakes — never from a target snapshot.
    for transition in outcome.stopped {
        let pid = ctx.counter.current(&transition.agent_id).await;
        ctx.counter.forget(&transition.agent_id).await;
        send_agent_state(&ctx.state_tx, &transition.agent_id, "stopped", pid).await;
    }
}

/// Resolve the agent spec from the target cache, returning a runnable
/// `Agent` shape. `None` means the bridge has no record of this agent_id
/// — the caller should warn and skip; the next `bridge.target` will
/// either include it (then a follow-up RPC re-applies) or confirm the
/// platform never knew about it.
async fn lookup_target(ctx: &SessionCtx, agent_id: &str) -> Option<crate::store::agents::Agent> {
    let cache = ctx.targets.lock().await;
    cache.get(agent_id).map(reconcile::target_to_agent)
}

async fn handle_agent_start_frame(ctx: &SessionCtx, payload: AgentStart) {
    let agent_id = payload.agent_id.as_str();
    let Some(agent) = lookup_target(ctx, agent_id).await else {
        tracing::warn!(
            agent_id = %agent_id,
            "agent.start: no spec in target cache; skipping"
        );
        return;
    };
    // Idempotent: if the runtime is already up, do nothing.
    if matches!(
        ctx.manager.process_state(agent_id).await,
        Some(ProcessState::Active { .. }) | Some(ProcessState::PromptInFlight { .. })
    ) {
        tracing::debug!(agent_id = %agent_id, "agent.start: agent already running; no-op");
        return;
    }
    let directive = payload.decision_resume.clone();
    if let Err(e) = ctx
        .manager
        .start_agent(&agent, None, directive.as_ref().map(|d| d.prompt.clone()))
        .await
    {
        tracing::warn!(agent_id = %agent_id, err = %e, "agent.start: start_agent failed");
        return;
    }
    let pid = ctx.counter.allocate(agent_id).await;
    send_agent_state(&ctx.state_tx, agent_id, "started", pid).await;
    if let Some(d) = directive {
        send_decision_delivered(&ctx.state_tx, agent_id, &d.decision_id).await;
    }
}

async fn handle_agent_stop_frame(ctx: &SessionCtx, payload: AgentStop) {
    let agent_id = payload.agent_id.as_str();
    if let Err(e) = ctx.manager.stop_agent(agent_id).await {
        tracing::warn!(agent_id = %agent_id, err = %e, "agent.stop: stop_agent failed");
        return;
    }
    let pid = ctx.counter.current(agent_id).await;
    ctx.counter.forget(agent_id).await;
    send_agent_state(&ctx.state_tx, agent_id, "stopped", pid).await;
}

async fn handle_agent_restart_frame(ctx: &SessionCtx, payload: AgentRestart) {
    let agent_id = payload.agent_id.as_str();
    let Some(agent) = lookup_target(ctx, agent_id).await else {
        tracing::warn!(
            agent_id = %agent_id,
            "agent.restart: no spec in target cache; skipping"
        );
        return;
    };
    // Stop first so the directive lands on the new instance, not the old one.
    if matches!(
        ctx.manager.process_state(agent_id).await,
        Some(ProcessState::Active { .. }) | Some(ProcessState::PromptInFlight { .. })
    ) {
        if let Err(e) = ctx.manager.stop_agent(agent_id).await {
            tracing::warn!(agent_id = %agent_id, err = %e, "agent.restart: stop_agent failed");
            return;
        }
        let prev_pid = ctx.counter.current(agent_id).await;
        ctx.counter.forget(agent_id).await;
        send_agent_state(&ctx.state_tx, agent_id, "stopped", prev_pid).await;
    }
    let directive = payload.decision_resume.clone();
    if let Err(e) = ctx
        .manager
        .start_agent(&agent, None, directive.as_ref().map(|d| d.prompt.clone()))
        .await
    {
        tracing::warn!(agent_id = %agent_id, err = %e, "agent.restart: start_agent failed");
        return;
    }
    let pid = ctx.counter.allocate(agent_id).await;
    send_agent_state(&ctx.state_tx, agent_id, "started", pid).await;
    if let Some(d) = directive {
        send_decision_delivered(&ctx.state_tx, agent_id, &d.decision_id).await;
    }
}

async fn handle_chat(ctx: &SessionCtx, payload: ChatMessageReceived) {
    // Hold the pending_chats lock across the targets-cache check so
    // handle_target's drain can never miss a chat that's about to be
    // buffered. Without this, the sequence (chat: cache miss) → (target:
    // populate cache + drain empty) → (chat: lock + push) leaves the
    // chat sitting in the buffer until the next reconcile.
    //
    // Lock ordering: pending_chats (tokio async) then targets (tokio
    // async, brief). handle_target acquires them in the opposite order
    // (targets via reconcile::apply, then pending_chats); both locks are
    // brief, so there's no interlock — they serialize on whichever is
    // contended first.
    let target_clone = {
        let mut buf = ctx.pending_chats.lock().await;
        match ctx.targets.lock().await.get(&payload.agent_id).cloned() {
            Some(target) => target,
            None => {
                let queue = buf.entry(payload.agent_id.clone()).or_default();
                if queue.len() >= PENDING_CHATS_PER_AGENT {
                    queue.pop_front();
                }
                queue.push_back(payload.clone());
                tracing::debug!(
                    agent_id = %payload.agent_id,
                    buffered = queue.len(),
                    "bridge: chat arrived before target — buffered for replay"
                );
                return;
            }
        }
    };
    let agent_id = payload.agent_id.as_str();
    let name = target_clone.name.as_str();

    let process_state = ctx.manager.process_state(agent_id).await;
    let result = match process_state {
        Some(ProcessState::Active { .. }) | Some(ProcessState::PromptInFlight { .. }) => {
            ctx.manager.notify_agent(agent_id).await
        }
        _ => {
            // Wake the agent. The cached target was captured above so the
            // manager doesn't need to re-read the store.
            //
            // Pull the unread message that triggered this wake so the
            // agent's first prompt has the message context. Without this,
            // the agent boots, runs `check_messages` as a tool call, then
            // replies — costing an extra LLM round-trip on every cold
            // start. With it, the message is the init prompt directly.
            let wake_message = ctx
                .store
                .get_messages_for_agent_id(agent_id, false)
                .ok()
                .and_then(|msgs| msgs.into_iter().next());
            let agent = super::reconcile::target_to_agent(&target_clone);
            let r = ctx.manager.start_agent(&agent, wake_message, None).await;
            if r.is_ok() {
                let pid = ctx.counter.allocate(agent_id).await;
                send_agent_state(&ctx.state_tx, agent_id, "started", pid).await;
            }
            r
        }
    };
    if let Err(e) = result {
        tracing::warn!(agent = %name, agent_id = %agent_id, err = %e, "bridge: failed to deliver chat");
        return;
    }

    send_chat_ack(&ctx.state_tx, agent_id, payload.seq).await;
}

/// Background loop that polls each managed agent's process state and emits
/// `agent.state` transitions upstream. Lightweight; runs until shutdown.
async fn agent_state_pusher(ctx: SessionCtx, shutdown: CancellationToken) {
    let mut last: HashMap<String, String> = HashMap::new();

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tokio::time::sleep(STATE_PUSHER_INTERVAL) => {}
        }

        let ids = ctx.manager.get_running_agent_ids().await;
        for agent_id in &ids {
            let Some(state) = ctx.manager.process_state(agent_id).await else {
                continue;
            };
            let label = process_state_label(&state);
            if last.get(agent_id).map(String::as_str) == Some(label) {
                continue;
            }
            // Filter agents with no cache entry — same reconcile-window guard
            // as build_agents_alive. Skip until the next bridge.target replays.
            if ctx.targets.lock().await.get(agent_id).is_none() {
                continue;
            }
            let pid = ctx.counter.current(agent_id).await;
            send_agent_state(&ctx.state_tx, agent_id, label, pid).await;
            last.insert(agent_id.clone(), label.to_string());
        }

        last.retain(|id, _| ids.iter().any(|other| other == id));
    }
}

fn process_state_label(state: &ProcessState) -> &'static str {
    match state {
        ProcessState::Idle => "idle",
        ProcessState::Starting => "starting",
        ProcessState::Active { .. } => "active",
        ProcessState::PromptInFlight { .. } => "active",
        ProcessState::Closed => "stopped",
        ProcessState::Failed(_) => "crashed",
    }
}
