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

    /// Return the channel id of the agent's most recent or in-flight run,
    /// if known. Used by the decision-inbox handler to infer which channel
    /// a `chorus_create_decision` emission belongs to (the agent doesn't
    /// pass a channel — channel context is implicit in the active run).
    fn run_channel_id<'a>(
        &'a self,
        _agent_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + 'a>> {
        Box::pin(async { None })
    }

    /// Deliver a self-contained envelope to the agent so it can act on a
    /// human's pick. Routes to the live session's prompt channel when the
    /// agent is `Active`; otherwise starts the agent with the envelope as
    /// the `init_directive` so the same payload arrives on first turn.
    ///
    /// The envelope is built by the decision handler and contains the
    /// original headline + question, the picked option's full label and
    /// body, and any human note. The agent treats it as a new prompt and
    /// continues its work without needing to re-read history.
    fn resume_with_prompt<'a>(
        &'a self,
        _agent_name: &'a str,
        _envelope: String,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async {
            Err(anyhow::anyhow!(
                "resume_with_prompt not implemented on this AgentLifecycle"
            ))
        })
    }
}
