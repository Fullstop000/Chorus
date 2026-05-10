//! Reconcile a `bridge.target` frame against the locally-running agent set.
//!
//! Each pass populates the in-memory [`super::ws::TargetCache`] (the
//! authoritative spec/identity cache on the bridge), starts any newly
//! desired agents, stops any that vanished. The bridge's local store
//! holds only `agent_sessions` rows; agent records live in the cache.
//! `forget_sessions_for_agent` is called on stop to drop resume cursors
//! since FK cascade no longer fires (the bridge runs with FK off, #145).

use std::collections::{HashMap, HashSet};
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
        paused: target.paused,
        restart_seq: target.restart_seq,
        pending_init_directive: target.init_directive.clone(),
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
    restart_seen: &Arc<Mutex<HashMap<String, i64>>>,
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

    // 3. Walk the desired set:
    //    - paused targets must be stopped if running.
    //    - non-paused targets whose `restart_seq` climbed above the
    //      last-applied value must be stopped+started (delivering the
    //      `init_directive` if the platform queued one).
    //    - non-paused targets that aren't running must be started.
    //    Without the restart-seq branch, the platform couldn't ask the
    //    bridge to re-launch a runtime after a spec change or decision
    //    resume — those lived on the now-gone direct lifecycle calls.
    for target in &targets {
        let already_running = running_set.contains(&target.agent_id);
        if target.paused {
            if already_running {
                if let Err(e) = manager.stop_agent(&target.agent_id).await {
                    tracing::warn!(agent_id = %target.agent_id, err = %e, "stop_agent failed during paused reconcile");
                }
                stopped.push(AgentTransition {
                    agent_id: target.agent_id.clone(),
                });
            }
            continue;
        }
        let last_applied = {
            let seen = restart_seen.lock().await;
            seen.get(&target.agent_id).copied().unwrap_or(0)
        };
        let needs_restart = target.restart_seq > last_applied && already_running;
        if needs_restart {
            if let Err(e) = manager.stop_agent(&target.agent_id).await {
                tracing::warn!(agent_id = %target.agent_id, err = %e, "stop_agent failed during restart-seq reconcile");
            }
        }
        if !needs_restart && already_running {
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
                let mut seen = restart_seen.lock().await;
                seen.insert(target.agent_id.clone(), target.restart_seq);
            }
            Err(e) => {
                tracing::error!(agent = %target.name, agent_id = %target.agent_id, err = %e, "start_agent failed during reconcile");
            }
        }
    }

    // 4. Stop any locally-running agent that is no longer in the desired
    //    set at all (row deleted on the platform). The manager-side stop
    //    runs unconditionally — leaving a runtime alive because the cache
    //    entry vanished would orphan an OS process. The upstream
    //    `agent.state{stopped}` frame still goes out even if the cache
    //    entry is missing, since we have the id directly.
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
