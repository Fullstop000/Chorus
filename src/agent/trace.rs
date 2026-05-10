use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Unique identifier for a single agent execution run.
pub type RunId = String;

/// Classification of a trace event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TraceEventKind {
    /// Agent has been notified and is reading/processing messages.
    Reading,
    Thinking {
        text: String,
    },
    ToolCall {
        tool_name: String,
        tool_input: String,
    },
    ToolResult {
        tool_name: String,
        content: String,
    },
    Text {
        text: String,
    },
    TurnEnd,
    Error {
        message: String,
    },
}

/// A single trace event emitted by an agent during a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEvent {
    pub run_id: RunId,
    pub agent_id: String,
    pub channel_id: Option<String>,
    pub seq: u64,
    pub timestamp_ms: u64,
    #[serde(flatten)]
    pub kind: TraceEventKind,
}

/// Per-agent run tracking state.
struct AgentRunState {
    active_run: Option<RunId>,
    channel_id: Option<String>,
    next_seq: AtomicU64,
}

impl AgentRunState {
    fn new() -> Self {
        Self {
            active_run: None,
            channel_id: None,
            next_seq: AtomicU64::new(0),
        }
    }

    fn next_seq(&self) -> u64 {
        self.next_seq.fetch_add(1, Ordering::Relaxed)
    }

    fn start_run(&mut self) -> RunId {
        let run_id = uuid::Uuid::new_v4().to_string();
        self.active_run = Some(run_id.clone());
        self.next_seq = AtomicU64::new(0);
        run_id
    }

    fn end_run(&mut self) {
        self.active_run = None;
        self.channel_id = None;
    }
}

/// Thread-safe store for all agents' trace run state.
pub struct AgentTraceStore {
    agents: std::sync::Mutex<HashMap<String, AgentRunState>>,
}

impl AgentTraceStore {
    pub fn new() -> Self {
        Self {
            agents: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Get or start a run for the agent. Returns (run_id, is_new_run).
    pub fn ensure_run(&self, agent_id: &str) -> (RunId, bool) {
        let mut agents = self.agents.lock().unwrap();
        let state = agents
            .entry(agent_id.to_string())
            .or_insert_with(AgentRunState::new);
        match &state.active_run {
            Some(run_id) => (run_id.clone(), false),
            None => {
                let run_id = state.start_run();
                (run_id, true)
            }
        }
    }

    /// Set the channel_id for the agent's current or next run.
    pub fn set_run_channel(&self, agent_id: &str, channel_id: &str) {
        let mut agents = self.agents.lock().unwrap();
        let state = agents
            .entry(agent_id.to_string())
            .or_insert_with(AgentRunState::new);
        state.channel_id = Some(channel_id.to_string());
    }

    /// Get the channel_id for the agent's current run, if set.
    pub fn run_channel_id(&self, agent_id: &str) -> Option<String> {
        let agents = self.agents.lock().unwrap();
        agents.get(agent_id).and_then(|s| s.channel_id.clone())
    }

    /// Get the next sequence number for the agent's current run.
    pub fn next_seq(&self, agent_id: &str) -> u64 {
        let agents = self.agents.lock().unwrap();
        agents.get(agent_id).map(|s| s.next_seq()).unwrap_or(0)
    }

    /// End the current run for the agent.
    pub fn end_run(&self, agent_id: &str) {
        let mut agents = self.agents.lock().unwrap();
        if let Some(state) = agents.get_mut(agent_id) {
            state.end_run();
        }
    }

    /// Get the active run id for the agent, if any.
    pub fn active_run_id(&self, agent_id: &str) -> Option<RunId> {
        let agents = self.agents.lock().unwrap();
        agents.get(agent_id).and_then(|s| s.active_run.clone())
    }

    /// Prepare to emit a trace event for `agent_id`, starting a run if
    /// none is active. Returns `(run_id, seq, channel_id)` under a single
    /// lock acquisition. Replaces the legacy three-call
    /// `ensure_run` → `next_seq` → `run_channel_id` sequence, which took
    /// three separate locks per event.
    pub fn begin_event(&self, agent_id: &str) -> (RunId, u64, Option<String>) {
        let mut agents = self.agents.lock().unwrap();
        let state = agents
            .entry(agent_id.to_string())
            .or_insert_with(AgentRunState::new);
        let run_id = match &state.active_run {
            Some(r) => r.clone(),
            None => state.start_run(),
        };
        let seq = state.next_seq();
        let ch = state.channel_id.clone();
        (run_id, seq, ch)
    }

    /// Prepare to emit a trace event only if a run is already active; never
    /// starts a new one. Used for terminal events (`TurnEnd`, `Error`) that
    /// shouldn't resurrect a run. Returns `None` if no active run.
    pub fn begin_active_event(&self, agent_id: &str) -> Option<(RunId, u64, Option<String>)> {
        let agents = self.agents.lock().unwrap();
        let state = agents.get(agent_id)?;
        let run_id = state.active_run.clone()?;
        let seq = state.next_seq();
        let ch = state.channel_id.clone();
        Some((run_id, seq, ch))
    }
}

impl Default for AgentTraceStore {
    fn default() -> Self {
        Self::new()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Build a TraceEvent with the current timestamp.
pub fn build_trace_event(
    run_id: RunId,
    agent_id: &str,
    channel_id: Option<String>,
    seq: u64,
    kind: TraceEventKind,
) -> TraceEvent {
    TraceEvent {
        run_id,
        agent_id: agent_id.to_string(),
        channel_id,
        seq,
        timestamp_ms: now_ms(),
        kind,
    }
}

/// Emit a trace event for `agent_id`, starting a run if none is active.
/// Collapses the common `begin_event` + `build_trace_event` + `tx.send`
/// sequence into one call. Non-fatal if no subscriber; the send return is
/// dropped.
pub fn emit_event(
    store: &AgentTraceStore,
    tx: &tokio::sync::broadcast::Sender<TraceEvent>,
    agent_id: &str,
    kind: TraceEventKind,
) {
    let (run_id, seq, ch) = store.begin_event(agent_id);
    let _ = tx.send(build_trace_event(run_id, agent_id, ch, seq, kind));
}

/// Emit a trace event only if a run is already active. No-op otherwise.
/// Used for `TurnEnd` / `Error` events that shouldn't resurrect a run.
pub fn emit_active_event(
    store: &AgentTraceStore,
    tx: &tokio::sync::broadcast::Sender<TraceEvent>,
    agent_id: &str,
    kind: TraceEventKind,
) {
    if let Some((run_id, seq, ch)) = store.begin_active_event(agent_id) {
        let _ = tx.send(build_trace_event(run_id, agent_id, ch, seq, kind));
    }
}
