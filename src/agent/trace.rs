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
    pub agent_name: String,
    pub seq: u64,
    pub timestamp_ms: u64,
    #[serde(flatten)]
    pub kind: TraceEventKind,
}

/// Per-agent run tracking state.
struct AgentRunState {
    active_run: Option<RunId>,
    next_seq: AtomicU64,
}

impl AgentRunState {
    fn new() -> Self {
        Self {
            active_run: None,
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
    pub fn ensure_run(&self, agent_name: &str) -> (RunId, bool) {
        let mut agents = self.agents.lock().unwrap();
        let state = agents
            .entry(agent_name.to_string())
            .or_insert_with(AgentRunState::new);
        match &state.active_run {
            Some(run_id) => (run_id.clone(), false),
            None => {
                let run_id = state.start_run();
                (run_id, true)
            }
        }
    }

    /// Get the next sequence number for the agent's current run.
    pub fn next_seq(&self, agent_name: &str) -> u64 {
        let agents = self.agents.lock().unwrap();
        agents.get(agent_name).map(|s| s.next_seq()).unwrap_or(0)
    }

    /// End the current run for the agent.
    pub fn end_run(&self, agent_name: &str) {
        let mut agents = self.agents.lock().unwrap();
        if let Some(state) = agents.get_mut(agent_name) {
            state.end_run();
        }
    }

    /// Get the active run id for the agent, if any.
    pub fn active_run_id(&self, agent_name: &str) -> Option<RunId> {
        let agents = self.agents.lock().unwrap();
        agents.get(agent_name).and_then(|s| s.active_run.clone())
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
    agent_name: &str,
    seq: u64,
    kind: TraceEventKind,
) -> TraceEvent {
    TraceEvent {
        run_id,
        agent_name: agent_name.to_string(),
        seq,
        timestamp_ms: now_ms(),
        kind,
    }
}
