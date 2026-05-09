//! Bridge-side persistence: only `agent_sessions` (resume cursors).
//!
//! Agent records are not written here — they live in-memory in
//! `bridge::client::ws::TargetCache`, populated from `bridge.target`.
//! The bridge opens its store with `foreign_keys=OFF` so session writes
//! work without a corresponding `agents` row. See #145.

use crate::store::Store;

/// Drop any persisted resume cursor for an agent that just left the
/// desired-state set. The bridge's `agent_sessions` rows are the only
/// per-agent state that survives across reconciles; with FK enforcement
/// off, no cascade fires automatically.
pub fn forget_sessions_for_agent(store: &Store, agent_id: &str) -> anyhow::Result<()> {
    store.delete_sessions_for_agent(agent_id)?;
    Ok(())
}
