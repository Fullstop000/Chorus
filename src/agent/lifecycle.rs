//! Runtime lifecycle operations the HTTP server can trigger for agents.

use std::future::Future;
use std::pin::Pin;

use crate::agent::activity_log::ActivityLogResponse;
use crate::store::messages::ReceivedMessage;

pub trait AgentLifecycle: Send + Sync {
    fn start_agent<'a>(
        &'a self,
        agent_name: &'a str,
        wake_message: Option<ReceivedMessage>,
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
}
