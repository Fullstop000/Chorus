//! Runtime lifecycle operations the HTTP server can trigger for agents.

use std::future::Future;
use std::pin::Pin;

use crate::agent::activity_log::ActivityLogResponse;
use crate::store::messages::ReceivedMessage;

pub trait AgentLifecycle: Send + Sync {
    /// Start (or wake) an agent process.
    ///
    /// `wake_message` carries the unread message that triggered this start, if
    /// any. `init_directive`, when `Some`, is delivered as the first prompt
    /// verbatim, overriding the auto-generated greeting/wake/resume prompt.
    /// Used by the agent-creation path to ask a brand-new agent to introduce
    /// itself; left `None` for restart, manual-start, and message-driven wake
    /// paths so existing behavior is unchanged.
    fn start_agent<'a>(
        &'a self,
        agent_name: &'a str,
        wake_message: Option<ReceivedMessage>,
        init_directive: Option<String>,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

    fn notify_agent<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

    /// Deliver a self-contained envelope to an agent's runtime session as
    /// the next turn prompt. Used by the decision-inbox resolve handler:
    /// when the human picks an option, the server builds the envelope
    /// text and calls this method.
    ///
    /// - Live agent (`Active`): prompt the live session directly.
    /// - Asleep / dead handle: respawn via `start_agent` with the
    ///   envelope as the init_directive.
    /// - In-flight or starting: `start_agent` short-circuits and the
    ///   envelope is lost. v2 adds a per-session FIFO queue. The system
    ///   prompt teaches agents to end their turn cleanly after
    ///   `chorus_create_decision`, so this race should be rare.
    fn resume_with_prompt<'a>(
        &'a self,
        agent_name: &'a str,
        envelope: String,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

    fn stop_agent<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

    /// Returns the runtime `ProcessState` for `agent_name` if a managed
    /// process exists, else `None`. Single source of truth for runtime
    /// liveness; replaces every read of the persisted `agents.status`
    /// column from this phase onward.
    fn process_state<'a>(
        &'a self,
        agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<crate::agent::drivers::ProcessState>> + Send + 'a>>;

    fn get_activity_log_data(
        &self,
        agent_name: &str,
        after_seq: Option<u64>,
    ) -> ActivityLogResponse;

    fn get_all_agent_activity_states(&self) -> Vec<(String, String, String)>;

    /// Get the active trace run id for an agent, if any.
    fn active_run_id(&self, _agent_name: &str) -> Option<String> {
        None
    }

    /// Associate a channel with the agent's current or next trace run.
    fn set_run_channel(&self, _agent_name: &str, _channel_id: &str) {}

    /// Read the channel associated with the agent's current trace run,
    /// if any. Used by the decision-inbox handler to infer the channel
    /// for a `chorus_create_decision` call when the agent's run was
    /// kicked off by a channel message.
    fn run_channel_id(&self, _agent_name: &str) -> Option<String> {
        None
    }
}
