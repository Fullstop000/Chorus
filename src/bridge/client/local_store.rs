//! Bridge-side persistence: synced agent records keyed by name. The bridge
//! does NOT store messages — chat lives on the platform; agents pull via
//! MCP. The bridge stores only what `AgentManager::start_agent` needs to
//! read (`store.get_agent(name)`).
//!
//! `agents.id` on the bridge side is the platform's `agent_id`. Reusing the
//! platform UUID locally lets the bridge translate name↔platform_id via
//! the store directly, with no separate cache.

use crate::store::agents::{AgentEnvVar, AgentRecordUpsert};
use crate::store::Store;

use super::ws::AgentTargetIn;

/// Insert or update the local agent record so `AgentManager::start_agent`
/// can read it back. Uses `target.agent_id` as the row's `id`.
pub fn upsert_from_target(store: &Store, target: &AgentTargetIn) -> anyhow::Result<()> {
    let env_vars: Vec<AgentEnvVar> = target
        .env_vars
        .iter()
        .enumerate()
        .map(|(i, e)| AgentEnvVar {
            key: e.key.clone(),
            value: e.value.clone(),
            position: i as i64,
        })
        .collect();

    let record = AgentRecordUpsert {
        name: &target.name,
        display_name: &target.display_name,
        description: target.description.as_deref(),
        system_prompt: target.system_prompt.as_deref(),
        runtime: &target.runtime,
        model: &target.model,
        reasoning_effort: target.reasoning_effort.as_deref(),
        machine_id: None,
        env_vars: &env_vars,
    };

    match store.get_agent(&target.name)? {
        Some(existing) if existing.id == target.agent_id => {
            store.update_agent_record(&record)?;
        }
        _ => {
            // Either no local row yet, or a stale row from a prior reconcile
            // whose id no longer matches the platform's. `create_agent_record_with_id`
            // handles the stale case by deleting and re-inserting.
            store.create_agent_record_with_id(&target.agent_id, &record)?;
        }
    }
    Ok(())
}
