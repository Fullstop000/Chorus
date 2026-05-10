//! Runtime observability the HTTP server reads for activity feeds and
//! status badges. The bridge client owns runtime lifecycle entirely;
//! the platform's only role here is to *observe* — `process_state` for
//! status, `get_activity_log_data` / `get_all_agent_activity_states`
//! for the activity feed, `active_run_id` / `set_run_channel` /
//! `run_channel_id` for trace routing.

use std::future::Future;
use std::pin::Pin;

use crate::agent::activity_log::ActivityLogResponse;

pub trait AgentLifecycle: Send + Sync {
    /// Returns the runtime `ProcessState` for `agent_id` if a managed
    /// process exists, else `None`. Single source of truth for runtime
    /// liveness; the persisted `agents.status` column is gone.
    fn process_state<'a>(
        &'a self,
        agent_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<crate::agent::drivers::ProcessState>> + Send + 'a>>;

    /// Activity log read for one agent. Keyed by `agent_id` end-to-end:
    /// the underlying `activity_log` store keys by id, and so does the
    /// trace store. The wire `TraceEvent.agent_id` field is consumed by
    /// the UI's traceStore as the per-agent map key; display names come
    /// from the agent record loaded separately.
    fn get_activity_log_data(&self, agent_id: &str, after_seq: Option<u64>) -> ActivityLogResponse;

    /// Snapshot of all agents' current activity states. Returns
    /// `(agent_id, activity, detail)` tuples — first column is the id,
    /// not the name.
    fn get_all_agent_activity_states(&self) -> Vec<(String, String, String)>;

    /// Get the active trace run id for an agent, if any.
    fn active_run_id(&self, _agent_id: &str) -> Option<String> {
        None
    }

    /// Associate a channel with the agent's current or next trace run.
    fn set_run_channel(&self, _agent_id: &str, _channel_id: &str) {}

    /// Return the channel id of the agent's most recent or in-flight run,
    /// if known. Used by the decision-inbox handler to infer which channel
    /// a `dispatch_decision` emission belongs to (the agent doesn't
    /// pass a channel — channel context is implicit in the active run).
    fn run_channel_id<'a>(
        &'a self,
        _agent_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<String>> + Send + 'a>> {
        Box::pin(async { None })
    }
}
