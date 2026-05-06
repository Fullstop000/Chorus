//! WebSocket client for the bridge↔platform protocol.
//!
//! Exchanges:
//!   - `bridge.hello` (B→P, on connect)
//!   - `bridge.target` (P→B, reply + on every CRUD)
//!   - `agent.state` (B→P, on transitions; first event after start carries pid)
//!   - `chat.message.received` (P→B, when a message lands for an agent on this machine)
//!   - `chat.ack` (B→P, after the bridge has handed the message to the agent)

use std::collections::HashMap;
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

use super::local_store::AgentIdMap;
use super::reconcile;
use super::BridgeClientConfig;

/// Per-agent instance counter the bridge advances on every `start_agent`.
/// The platform side uses `runtime_pid` only to disambiguate stop→start
/// races, not as a real OS pid. A monotonic counter is sufficient.
#[derive(Default)]
pub struct InstanceCounter {
    next: AtomicU32,
    by_name: Mutex<HashMap<String, u32>>,
}

impl InstanceCounter {
    pub async fn allocate(&self, name: &str) -> u32 {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.by_name.lock().await.insert(name.to_string(), id);
        id
    }

    pub async fn current(&self, name: &str) -> u32 {
        self.by_name.lock().await.get(name).copied().unwrap_or(0)
    }

    pub async fn forget(&self, name: &str) {
        self.by_name.lock().await.remove(name);
    }
}

const SUPPORTED_FRAMES: &[&str] = &["bridge.target", "chat.message.received"];
const BRIDGE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Serialize, Deserialize)]
struct WireFrame {
    v: u32,
    #[serde(rename = "type")]
    frame_type: String,
    data: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BridgeTargetIn {
    pub target_agents: Vec<AgentTargetIn>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct AgentTargetIn {
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
    #[serde(default)]
    pub pending_prompt: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct EnvVarIn {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatMessageReceived {
    pub agent_id: String,
    #[serde(default)]
    pub channel_id: Option<String>,
    pub seq: i64,
    #[serde(default)]
    pub messages: Value,
}

/// Public re-export so callers can pattern-match on frame kinds.
pub enum ServerFrame {
    Target(BridgeTargetIn),
    ChatMessage(ChatMessageReceived),
    Other(String, Value),
}

pub async fn run_ws_client_loop(
    cfg: BridgeClientConfig,
    manager: Arc<AgentManager>,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let id_map = Arc::new(Mutex::new(AgentIdMap::default()));
    let counter = Arc::new(InstanceCounter::default());

    let mut backoff_ms: u64 = 500;
    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }

        match run_one_session(
            &cfg,
            manager.clone(),
            id_map.clone(),
            counter.clone(),
            shutdown.clone(),
        )
        .await
        {
            Ok(()) => {
                tracing::info!("bridge: WS session ended cleanly; reconnecting");
                backoff_ms = 500;
            }
            Err(e) => {
                tracing::warn!(err = %e, backoff_ms, "bridge: WS session failed; reconnecting");
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(backoff_ms)) => {}
                    _ = shutdown.cancelled() => return Ok(()),
                }
                backoff_ms = (backoff_ms * 2).min(30_000);
            }
        }
    }
}

async fn run_one_session(
    cfg: &BridgeClientConfig,
    manager: Arc<AgentManager>,
    id_map: Arc<Mutex<AgentIdMap>>,
    counter: Arc<InstanceCounter>,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
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
    let agents_alive = build_agents_alive(&manager, &id_map, &counter).await;
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

    // 2. Spawn the agent.state pusher task.
    let (state_tx, mut state_rx) = tokio::sync::mpsc::channel::<String>(64);
    let state_tx_for_pusher = state_tx.clone();
    let manager_for_pusher = manager.clone();
    let id_map_for_pusher = id_map.clone();
    let counter_for_pusher = counter.clone();
    let shutdown_for_pusher = shutdown.clone();
    let pusher = tokio::spawn(async move {
        agent_state_pusher(
            manager_for_pusher,
            id_map_for_pusher,
            counter_for_pusher,
            state_tx_for_pusher,
            shutdown_for_pusher,
        )
        .await;
    });

    // 3. Drive the read+write select loop.
    loop {
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
                                handle_target(
                                    &cfg.store,
                                    &manager,
                                    id_map.clone(),
                                    counter.clone(),
                                    target,
                                    &state_tx,
                                )
                                .await;
                            }
                            "chat.message.received" => {
                                let payload: ChatMessageReceived = match serde_json::from_value(frame.data) {
                                    Ok(p) => p,
                                    Err(e) => {
                                        tracing::warn!(err = %e, "bridge: bad chat.message.received");
                                        continue;
                                    }
                                };
                                handle_chat(
                                    &manager,
                                    id_map.clone(),
                                    counter.clone(),
                                    payload,
                                    &state_tx,
                                )
                                .await;
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
    Ok(())
}

async fn build_agents_alive(
    manager: &Arc<AgentManager>,
    id_map: &Arc<Mutex<AgentIdMap>>,
    counter: &Arc<InstanceCounter>,
) -> Vec<Value> {
    let names = manager.get_running_agent_names().await;
    let id_map = id_map.lock().await;
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let Some(platform_id) = id_map.platform_id_for(&name).map(str::to_owned) else {
            continue;
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

async fn handle_target(
    store: &Arc<crate::store::Store>,
    manager: &Arc<AgentManager>,
    id_map: Arc<Mutex<AgentIdMap>>,
    counter: Arc<InstanceCounter>,
    target: BridgeTargetIn,
    state_tx: &tokio::sync::mpsc::Sender<String>,
) {
    let mut id_map_guard = id_map.lock().await;
    let outcome =
        match reconcile::apply(store, manager, &mut id_map_guard, target.target_agents).await {
            Ok(o) => o,
            Err(e) => {
                tracing::error!(err = %e, "bridge: reconcile failed");
                return;
            }
        };
    drop(id_map_guard);

    for name in outcome.started {
        let platform_id = match id_map.lock().await.platform_id_for(&name) {
            Some(id) => id.to_string(),
            None => continue,
        };
        let pid = counter.allocate(&name).await;
        let frame = WireFrame {
            v: 1,
            frame_type: "agent.state".into(),
            data: json!({
                "agent_id": platform_id,
                "state": "started",
                "runtime_pid": pid,
                "ts": chrono::Utc::now().to_rfc3339(),
            }),
        };
        if let Ok(text) = serde_json::to_string(&frame) {
            let _ = state_tx.send(text).await;
        }
    }
    for name in outcome.stopped {
        let platform_id = match id_map.lock().await.platform_id_for(&name) {
            Some(id) => id.to_string(),
            None => continue,
        };
        let pid = counter.current(&name).await;
        counter.forget(&name).await;
        let frame = WireFrame {
            v: 1,
            frame_type: "agent.state".into(),
            data: json!({
                "agent_id": platform_id,
                "state": "stopped",
                "runtime_pid": pid,
                "ts": chrono::Utc::now().to_rfc3339(),
            }),
        };
        if let Ok(text) = serde_json::to_string(&frame) {
            let _ = state_tx.send(text).await;
        }
    }
}

async fn handle_chat(
    manager: &Arc<AgentManager>,
    id_map: Arc<Mutex<AgentIdMap>>,
    counter: Arc<InstanceCounter>,
    payload: ChatMessageReceived,
    state_tx: &tokio::sync::mpsc::Sender<String>,
) {
    let name = {
        let map = id_map.lock().await;
        map.name_for(&payload.agent_id).map(str::to_owned)
    };
    let Some(name) = name else {
        tracing::warn!(agent_id = %payload.agent_id, "bridge: chat for unknown agent");
        return;
    };

    let process_state = manager.process_state(&name).await;
    let result = match process_state {
        Some(ProcessState::Active { .. }) | Some(ProcessState::PromptInFlight { .. }) => {
            manager.notify_agent(&name).await
        }
        _ => {
            let r = manager.start_agent(&name, None, None).await;
            if r.is_ok() {
                let pid = counter.allocate(&name).await;
                let started = WireFrame {
                    v: 1,
                    frame_type: "agent.state".into(),
                    data: json!({
                        "agent_id": payload.agent_id,
                        "state": "started",
                        "runtime_pid": pid,
                        "ts": chrono::Utc::now().to_rfc3339(),
                    }),
                };
                if let Ok(text) = serde_json::to_string(&started) {
                    let _ = state_tx.send(text).await;
                }
            }
            r
        }
    };
    if let Err(e) = result {
        tracing::warn!(agent = %name, err = %e, "bridge: failed to deliver chat");
        return;
    }

    let ack = WireFrame {
        v: 1,
        frame_type: "chat.ack".into(),
        data: json!({
            "agent_id": payload.agent_id,
            "last_seq": payload.seq,
            "ts": chrono::Utc::now().to_rfc3339(),
        }),
    };
    if let Ok(text) = serde_json::to_string(&ack) {
        let _ = state_tx.send(text).await;
    }
}

/// Background loop that polls each managed agent's process state and emits
/// `agent.state` transitions upstream. Lightweight; runs until shutdown.
async fn agent_state_pusher(
    manager: Arc<AgentManager>,
    id_map: Arc<Mutex<AgentIdMap>>,
    counter: Arc<InstanceCounter>,
    state_tx: tokio::sync::mpsc::Sender<String>,
    shutdown: CancellationToken,
) {
    let mut last: HashMap<String, String> = HashMap::new();

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
        }

        let names = manager.get_running_agent_names().await;
        for name in &names {
            let Some(state) = manager.process_state(name).await else {
                continue;
            };
            let label = process_state_label(&state);
            if last.get(name).map(String::as_str) == Some(label) {
                continue;
            }
            let platform_id = {
                let map = id_map.lock().await;
                map.platform_id_for(name).map(str::to_owned)
            };
            let Some(platform_id) = platform_id else {
                continue;
            };
            let pid = counter.current(name).await;
            let frame = WireFrame {
                v: 1,
                frame_type: "agent.state".into(),
                data: json!({
                    "agent_id": platform_id,
                    "state": label,
                    "runtime_pid": pid,
                    "ts": chrono::Utc::now().to_rfc3339(),
                }),
            };
            if let Ok(text) = serde_json::to_string(&frame) {
                let _ = state_tx.send(text).await;
            }
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
