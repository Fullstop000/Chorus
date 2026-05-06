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
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::agent::drivers::ProcessState;
use crate::agent::manager::AgentManager;

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
    by_name: Mutex<HashMap<String, u32>>,
}

impl InstanceCounter {
    async fn allocate(&self, name: &str) -> u32 {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.by_name.lock().await.insert(name.to_string(), id);
        id
    }

    async fn current(&self, name: &str) -> u32 {
        self.by_name.lock().await.get(name).copied().unwrap_or(0)
    }

    async fn forget(&self, name: &str) {
        self.by_name.lock().await.remove(name);
    }
}

const SUPPORTED_FRAMES: &[&str] = &["bridge.target", "chat.message.received"];
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

#[derive(Debug, Serialize, Deserialize)]
struct WireFrame {
    v: u32,
    #[serde(rename = "type")]
    frame_type: String,
    data: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct BridgeTargetIn {
    pub target_agents: Vec<AgentTargetIn>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub(super) struct AgentTargetIn {
    pub agent_id: String,
    pub name: String,
    pub display_name: String,
    pub runtime: String,
    pub model: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub env_vars: Vec<EnvVarIn>,
    #[serde(default)]
    pub init_directive: Option<String>,
    /// Reserved for a future slice that funnels a queued user prompt
    /// through `bridge.target` so the bridge can deliver it on first turn.
    /// Currently unused; kept on the deserializer so platforms emitting
    /// the field don't fail-load on this bridge.
    #[serde(default)]
    #[allow(dead_code)]
    pub pending_prompt: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub(super) struct EnvVarIn {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
struct ChatMessageReceived {
    agent_id: String,
    #[serde(default)]
    #[allow(dead_code)]
    channel_id: Option<String>,
    seq: i64,
    #[serde(default)]
    #[allow(dead_code)]
    messages: Value,
}

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
        "agent.state",
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
        "chat.ack",
        json!({
            "agent_id": platform_id,
            "last_seq": last_seq,
            "ts": chrono::Utc::now().to_rfc3339(),
        }),
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

    let mut backoff_ms: u64 = INITIAL_BACKOFF_MS;
    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }

        // Clear stale pending chats on each reconnect: the platform will
        // re-emit anything we still need on its next push, and chats from
        // before the disconnect are already on the platform side.
        pending_chats.lock().await.clear();

        match run_one_session(
            &cfg,
            manager.clone(),
            counter.clone(),
            pending_chats.clone(),
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
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    // The session-level state (counter, pending_chats) is owned by
    // run_ws_client_loop and reused across reconnects so a brief drop
    // doesn't lose pid bookkeeping. The state_tx channel is per-session —
    // we re-create it here and bundle into a SessionCtx after the
    // handshake.
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
    let agents_alive = build_agents_alive(&cfg.store, &manager, &counter).await;
    let hello = WireFrame {
        v: 1,
        frame_type: "bridge.hello".into(),
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
                            "bridge.target" => {
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
                            "chat.message.received" => {
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
    store: &Arc<crate::store::Store>,
    manager: &Arc<AgentManager>,
    counter: &Arc<InstanceCounter>,
) -> Vec<Value> {
    let names = manager.get_running_agent_names().await;
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let platform_id = match store.get_agent(&name) {
            Ok(Some(agent)) => agent.id,
            _ => continue,
        };
        let pid = counter.current(&name).await;
        out.push(json!({
            "agent_id": platform_id,
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
    let outcome = match reconcile::apply(&ctx.store, &ctx.manager, target_agents).await {
        Ok(o) => o,
        Err(e) => {
            tracing::error!(err = %e, "bridge: reconcile failed");
            return;
        }
    };

    // Replay any chats that arrived before this reconcile created the
    // local row. We only drain entries for platform_ids that are now in
    // the target — others remain buffered (or stay until they age out via
    // reconnect). Replays are dispatched as detached tasks so this
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

    for transition in outcome.started {
        let pid = ctx.counter.allocate(&transition.name).await;
        send_agent_state(&ctx.state_tx, &transition.platform_id, "started", pid).await;
    }
    for transition in outcome.stopped {
        let pid = ctx.counter.current(&transition.name).await;
        ctx.counter.forget(&transition.name).await;
        send_agent_state(&ctx.state_tx, &transition.platform_id, "stopped", pid).await;
    }
}

async fn handle_chat(ctx: &SessionCtx, payload: ChatMessageReceived) {
    let name = match ctx.store.get_agent_by_id(&payload.agent_id, false) {
        Ok(Some(agent)) => Some(agent.name),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(err = %e, agent_id = %payload.agent_id, "bridge: store lookup failed for chat");
            return;
        }
    };
    let Some(name) = name else {
        // Stash for replay on the next handle_target that knows this
        // platform_id. Cap per-agent so a misrouted-chat storm can't
        // pin unbounded memory.
        let mut buf = ctx.pending_chats.lock().await;
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
    };

    let process_state = ctx.manager.process_state(&name).await;
    let result = match process_state {
        Some(ProcessState::Active { .. }) | Some(ProcessState::PromptInFlight { .. }) => {
            ctx.manager.notify_agent(&name).await
        }
        _ => {
            let r = ctx.manager.start_agent(&name, None, None).await;
            if r.is_ok() {
                let pid = ctx.counter.allocate(&name).await;
                send_agent_state(&ctx.state_tx, &payload.agent_id, "started", pid).await;
            }
            r
        }
    };
    if let Err(e) = result {
        tracing::warn!(agent = %name, err = %e, "bridge: failed to deliver chat");
        return;
    }

    send_chat_ack(&ctx.state_tx, &payload.agent_id, payload.seq).await;
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

        let names = ctx.manager.get_running_agent_names().await;
        for name in &names {
            let Some(state) = ctx.manager.process_state(name).await else {
                continue;
            };
            let label = process_state_label(&state);
            if last.get(name).map(String::as_str) == Some(label) {
                continue;
            }
            let platform_id = match ctx.store.get_agent(name) {
                Ok(Some(agent)) => agent.id,
                _ => continue,
            };
            let pid = ctx.counter.current(name).await;
            send_agent_state(&ctx.state_tx, &platform_id, label, pid).await;
            last.insert(name.clone(), label.to_string());
        }

        last.retain(|n, _| names.iter().any(|m| m == n));
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
