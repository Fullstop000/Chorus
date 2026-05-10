//! Reconcile a `bridge.target` frame against the locally-running agent set.
//!
//! Each pass populates the in-memory [`super::ws::TargetCache`] (the
//! authoritative spec/identity cache on the bridge), starts any newly
//! desired agents, stops any that vanished. The bridge's local store
//! holds only `agent_sessions` rows; agent records live in the cache.
//! `forget_sessions_for_agent` is called on stop to drop resume cursors
//! since FK cascade no longer fires (the bridge runs with FK off, #145).

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::agent::manager::AgentManager;
use crate::store::agents::{Agent, AgentEnvVar};
use crate::store::Store;

use super::local_store::forget_sessions_for_agent;
use super::ws::{AgentTargetIn, TargetCache};

/// Per-pass transition record. After #142 the bridge keys runtimes by
/// `agent_id` directly (the platform's UUID), so a transition is just
/// the id.
pub struct AgentTransition {
    pub agent_id: String,
}

pub struct ReconcileOutcome {
    pub started: Vec<AgentTransition>,
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
        machine_id: None,
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

    let mut started = Vec::new();
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

    let running = manager.get_running_agent_ids().await;
    let running_set: HashSet<String> = running.iter().cloned().collect();

    // 3. Start every desired agent that isn't already running. The spec
    //    is materialised from the wire target, not re-read from the store.
    for target in &targets {
        if running_set.contains(&target.agent_id) {
            continue;
        }
        let agent = target_to_agent(target);
        match manager
            .start_agent(&agent, None, target.init_directive.clone())
            .await
        {
            Ok(()) => {
                started.push(AgentTransition {
                    agent_id: target.agent_id.clone(),
                });
            }
            Err(e) => {
                tracing::error!(agent = %target.name, agent_id = %target.agent_id, err = %e, "start_agent failed during reconcile");
            }
        }
    }

    // 4. Stop any locally-running agent that is no longer desired. The
    //    manager-side stop runs unconditionally — leaving a runtime alive
    //    because the cache entry vanished would orphan an OS process. The
    //    upstream `agent.state{stopped}` frame still goes out even if the
    //    cache entry is missing, since we have the id directly.
    for agent_id in running.iter() {
        if desired.contains(agent_id) {
            continue;
        }
        {
            let mut cache = targets_cache.lock().await;
            cache.forget(agent_id);
        }
        if let Err(e) = manager.stop_agent(agent_id).await {
            tracing::warn!(agent_id = %agent_id, err = %e, "stop_agent failed during reconcile");
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

    Ok(ReconcileOutcome { started, stopped })
}
