//! Reconcile a `bridge.target` frame against the locally-cached spec view.
//!
//! `bridge.target` carries identity + spec only; lifecycle intent flows
//! through `agent.start` / `agent.stop` / `agent.restart` RPCs. Each pass:
//!
//!   1. Refreshes [`super::ws::TargetCache`] with the new spec snapshot.
//!   2. Stops any locally-running agent whose `agent_id` left the desired set
//!      (orphan sweep — without this a deleted agent's process would linger).
//!   3. Drops session rows for agents not in the desired set, since the
//!      bridge runs with `foreign_keys=OFF` and there's no cascade.

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::agent::manager::AgentManager;
use crate::store::agents::{Agent, AgentEnvVar};
use crate::store::Store;

use super::local_store::forget_sessions_for_agent;
use super::ws::{AgentTargetIn, TargetCache};

/// Per-pass transition record. Runtimes are keyed by `agent_id`
/// (the platform's UUID), so a transition is just the id.
pub struct AgentTransition {
    pub agent_id: String,
}

pub struct ReconcileOutcome {
    pub stopped: Vec<AgentTransition>,
}

/// Materialise an `AgentTargetIn` (wire payload) into an `Agent` (store
/// row shape) without touching SQLite. The non-wire fields (`workspace_id`,
/// `created_at`, `machine_id`) are filled with placeholders since the
/// bridge does not consume them — `start_agent` reads only the fields
/// the runtime spec needs.
pub(super) fn target_to_agent(target: &AgentTargetIn) -> Agent {
    Agent {
        id: target.agent_id.clone(),
        workspace_id: String::new(),
        name: target.name.clone(),
        display_name: target.display_name.clone(),
        description: target.description.clone(),
        system_prompt: target.system_prompt.clone(),
        runtime: target.runtime.clone(),
        model: target.model.clone(),
        reasoning_effort: target.reasoning_effort.clone(),
        // Bridge-side stub: the manager doesn't read `machine_id` (the
        // bridge IS the machine), so an empty string is fine. Filling
        // it with the bridge's own id would also be correct but adds a
        // dependency on cfg threading.
        machine_id: String::new(),
        env_vars: target
            .env_vars
            .iter()
            .enumerate()
            .map(|(i, e)| AgentEnvVar {
                key: e.key.clone(),
                value: e.value.clone(),
                position: i as i64,
            })
            .collect(),
        created_at: chrono::Utc::now(),
    }
}

pub async fn apply(
    store: &Store,
    manager: &AgentManager,
    targets_cache: &Arc<Mutex<TargetCache>>,
    targets: Vec<AgentTargetIn>,
) -> anyhow::Result<ReconcileOutcome> {
    let mut desired: HashSet<String> = HashSet::new();
    let mut stopped = Vec::new();

    // 1. Bulk-prune session rows whose agent_id isn't in this target.
    //    The per-stop cleanup below would miss any agent that got
    //    removed from desired while the bridge was offline (running set
    //    is empty after restart, so the stop loop never fires for it).
    //    Running this once at the top of every reconcile catches that
    //    case AND legitimate online removals that the stop loop also
    //    handles — both paths converge here, the per-stop call is a
    //    redundant safety net we keep for symmetry with stop events.
    let desired_ids: Vec<String> = targets.iter().map(|t| t.agent_id.clone()).collect();
    if let Err(e) = store.delete_sessions_for_agents_not_in(&desired_ids) {
        tracing::warn!(err = %e, "reconcile: bulk session cleanup failed; per-stop will pick up online removals");
    }

    // 2. Refresh the in-memory cache. Spec lookups read from here; the
    //    bridge's `agents` table stays empty.
    {
        let mut cache = targets_cache.lock().await;
        for target in &targets {
            desired.insert(target.agent_id.clone());
            cache.upsert(target.clone());
        }
    }

    // 3. Stop any locally-running agent that is no longer in the desired
    //    set (row deleted on the platform). Leaving a runtime alive
    //    because the cache entry vanished would orphan an OS process.
    //    Lifecycle intent (start / stop / restart of agents that DO
    //    remain in the set) flows through `agent.start` / `agent.stop`
    //    / `agent.restart` frames, not through reconcile.
    let running = manager.get_running_agent_ids().await;
    for agent_id in running.iter() {
        if desired.contains(agent_id) {
            continue;
        }
        {
            let mut cache = targets_cache.lock().await;
            cache.forget(agent_id);
        }
        if let Err(e) = manager.stop_agent(agent_id).await {
            tracing::warn!(agent_id = %agent_id, err = %e, "stop_agent failed during orphan sweep");
        }
        if let Err(e) = forget_sessions_for_agent(store, agent_id) {
            tracing::warn!(
                agent_id = %agent_id,
                err = %e,
                "reconcile: failed to drop sessions for stopped agent"
            );
        }
        stopped.push(AgentTransition {
            agent_id: agent_id.clone(),
        });
    }

    Ok(ReconcileOutcome { stopped })
}
