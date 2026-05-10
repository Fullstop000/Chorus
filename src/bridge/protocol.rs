//! Bridge ↔ platform WebSocket protocol: shared frame names, envelope,
//! and payload structs.
//!
//! Both sides of `/api/bridge/ws` speak the same wire shape, so the
//! payload types are defined once here with `Serialize + Deserialize`
//! and used by both the platform handler (`server::transport::bridge_ws`)
//! and the bridge client (`bridge::client::ws`). A typo on either side
//! becomes a compile error rather than a silently-dropped frame.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Frame type tags ──────────────────────────────────────────────────────

/// Bridge → Platform: first frame after WS upgrade. Identifies the
/// machine and reports currently-running agents for resume.
pub const FRAME_BRIDGE_HELLO: &str = "bridge.hello";

/// Platform → Bridge: full desired runtime config for every agent that
/// should run on this bridge. Sent in reply to `bridge.hello` and again
/// on every change to desired state.
pub const FRAME_BRIDGE_TARGET: &str = "bridge.target";

/// Bridge → Platform: runtime state transition for one agent. Carries
/// `runtime_pid` as the instance discriminator that filters out
/// stop→start race events.
pub const FRAME_AGENT_STATE: &str = "agent.state";

/// Platform → Bridge: chat batch destined for an agent owned by this
/// bridge. The bridge wakes the runtime and acks via `chat.ack`.
pub const FRAME_CHAT_MESSAGE_RECEIVED: &str = "chat.message.received";

/// Bridge → Platform: cursor advance after a `chat.message.received`
/// batch was buffered for the local runtime.
pub const FRAME_CHAT_ACK: &str = "chat.ack";

/// Platform → Bridge: start an agent's runtime. Idempotent — if the
/// agent is already running, the bridge no-ops. Carries an optional
/// `init_directive` (e.g., a decision-resume envelope) that becomes the
/// agent's first turn prompt.
pub const FRAME_AGENT_START: &str = "agent.start";

/// Platform → Bridge: stop an agent's runtime. Idempotent — if no
/// process exists, the bridge no-ops. The agent stays in `bridge.target`
/// (still owned by this bridge), it just isn't running.
pub const FRAME_AGENT_STOP: &str = "agent.stop";

/// Platform → Bridge: stop+start the agent's runtime to pick up a spec
/// change or deliver a one-shot `init_directive`. Equivalent to a stop
/// followed by a start, but carried as one frame so the bridge can
/// guarantee the directive lands on the new instance.
pub const FRAME_AGENT_RESTART: &str = "agent.restart";

/// Bridge → Platform: confirms the bridge delivered a decision-resume
/// prompt to the agent on its next start. The platform marks exactly
/// that decision as delivered, so reconnect-replay won't re-emit it.
pub const FRAME_AGENT_DECISION_DELIVERED: &str = "agent.decision_delivered";

// ── Envelope ─────────────────────────────────────────────────────────────

/// JSON envelope wrapping every WS frame in either direction.
///
/// `frame_type` is read as a `String` rather than a typed enum so unknown
/// / future frame types can be logged-and-skipped without failing the
/// session — both sides intentionally tolerate frames they don't know yet.
#[derive(Debug, Serialize, Deserialize)]
pub struct WireFrame {
    pub v: u32,
    #[serde(rename = "type")]
    pub frame_type: String,
    pub data: Value,
}

// ── Bridge → Platform payloads ───────────────────────────────────────────

/// `bridge.hello` payload — bridge introduces itself.
///
/// `supported_frames` and `agents_alive` are accepted by the deserializer
/// but not yet consumed by the platform; reserved for frame-compat
/// negotiation and target-vs-alive reconciliation.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Hello {
    pub machine_id: String,
    pub bridge_version: String,
    #[serde(default)]
    pub supported_frames: Vec<String>,
    #[serde(default)]
    pub agents_alive: Vec<AgentAlive>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentAlive {
    pub agent_id: String,
    pub state: String,
    #[serde(default)]
    pub runtime_pid: Option<u32>,
    #[serde(default)]
    pub last_acked_seq: Option<u64>,
}

/// `agent.state` payload — bridge reports a runtime transition.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AgentState {
    pub agent_id: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<String>,
    pub runtime_pid: u32,
}

/// `chat.ack` payload — bridge cursor advance for a delivered chat batch.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ChatAck {
    pub agent_id: String,
    pub last_seq: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<String>,
}

// ── Platform → Bridge payloads ───────────────────────────────────────────

/// `bridge.target` payload — desired runtime config for this bridge.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BridgeTarget {
    pub target_agents: Vec<AgentTarget>,
}

/// One agent's full runtime config inside a `bridge.target`. `bridge.target`
/// carries identity + spec only; lifecycle intent (start / stop / restart)
/// flows as discrete `FRAME_AGENT_START` / `FRAME_AGENT_STOP` /
/// `FRAME_AGENT_RESTART` frames.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct AgentTarget {
    pub agent_id: String,
    pub name: String,
    pub display_name: String,
    pub runtime: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub env_vars: Vec<EnvVar>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct EnvVar {
    pub key: String,
    pub value: String,
}

/// `chat.message.received` payload — chat batch destined for an agent.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct ChatMessageReceived {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    pub seq: i64,
    /// Opaque payload — the platform serializes the messages it wants the
    /// bridge to hand off to the runtime. The bridge currently does not
    /// inspect this field, only forwards it via the agent's mailbox.
    #[serde(default)]
    pub messages: Value,
}

/// A decision-resume prompt the bridge delivers as the agent's first
/// turn after a human resolves a decision in the inbox. `decision_id`
/// is the resolved decision's row id; the bridge echoes it back via
/// `agent.decision_delivered` so the platform marks exactly that row as
/// delivered (not "all undelivered decisions for this agent", which
/// races with rapid re-resolves).
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct DecisionResume {
    pub decision_id: String,
    pub prompt: String,
}

/// `agent.start` payload — platform asks the bridge to launch this
/// agent's runtime. If the agent is already running, the bridge no-ops.
/// `decision_resume`, when present, supplies the agent's first turn
/// prompt and the `decision_id` the bridge must echo back via
/// `agent.decision_delivered`.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct AgentStart {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_resume: Option<DecisionResume>,
}

/// `agent.stop` payload — platform asks the bridge to terminate this
/// agent's runtime. If no process exists, the bridge no-ops.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct AgentStop {
    pub agent_id: String,
}

/// `agent.restart` payload — stop+start the agent's runtime to pick up
/// a spec change or deliver a queued decision resume. Equivalent to a
/// stop followed by a start, but issued as one frame so the resume
/// lands on the new instance.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct AgentRestart {
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_resume: Option<DecisionResume>,
}

/// `agent.decision_delivered` payload — bridge confirms it delivered a
/// specific decision-resume prompt to the agent on its next start. The
/// platform marks exactly the named decision as delivered.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct AgentDecisionDelivered {
    pub agent_id: String,
    pub decision_id: String,
}
