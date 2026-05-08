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

/// Per-pass transition record: `(local_name, platform_agent_id)`. The
/// platform_id is captured at reconcile time so callers don't need to
/// look it up after a stop has already deleted the local row.
pub struct AgentTransition {
    pub name: String,
    pub platform_id: String,
}

pub struct ReconcileOutcome {
    pub started: Vec<AgentTransition>,
    pub stopped: Vec<AgentTransition>,
}

/// Materialise an `AgentTargetIn` (wire payload) into an `Agent` (store
/// row shape) without touching SQLite. The non-wire fields (`workspace_id`,
/// `created_at`, `machine_id`) are filled with placeholders since the
/// bridge does not consume them — `start_agent_from_record` reads only
/// the fields the runtime spec needs.
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

    // 1. Refresh the in-memory cache. Spec lookups read from here; the
    //    bridge's `agents` table stays empty.
    {
        let mut cache = targets_cache.lock().await;
        for target in &targets {
            desired.insert(target.name.clone());
            cache.upsert(target.clone());
        }
    }

    let running = manager.get_running_agent_names().await;
    let running_set: HashSet<String> = running.iter().cloned().collect();

    // 2. Start every desired agent that isn't already running. The spec
    //    is materialised from the wire target, not re-read from the store.
    for target in &targets {
        if running_set.contains(&target.name) {
            continue;
        }
        let agent = target_to_agent(target);
        match manager
            .start_agent_from_record(agent, target.init_directive.clone())
            .await
        {
            Ok(()) => {
                started.push(AgentTransition {
                    name: target.name.clone(),
                    platform_id: target.agent_id.clone(),
                });
            }
            Err(e) => {
                tracing::error!(agent = %target.name, err = %e, "start_agent failed during reconcile");
            }
        }
    }

    // 3. Stop any locally-running agent that is no longer desired. The
    //    manager-side stop runs unconditionally — leaving a runtime alive
    //    because the cache entry vanished would orphan an OS process. The
    //    upstream `agent.state{stopped}` frame is conditional on resolving
    //    platform_id from the cache.
    for name in running.iter() {
        if desired.contains(name) {
            continue;
        }
        let platform_id = {
            let mut cache = targets_cache.lock().await;
            cache.forget_by_name(name).map(|t| t.agent_id)
        };
        if let Err(e) = manager.stop_agent(name).await {
            tracing::warn!(agent = %name, err = %e, "stop_agent failed during reconcile");
        }
        match platform_id {
            Some(platform_id) => {
                if let Err(e) = forget_sessions_for_agent(store, &platform_id) {
                    tracing::warn!(
                        agent = %name,
                        platform_id = %platform_id,
                        err = %e,
                        "reconcile: failed to drop sessions for stopped agent"
                    );
                }
                stopped.push(AgentTransition {
                    name: name.clone(),
                    platform_id,
                });
            }
            None => {
                tracing::debug!(
                    agent = %name,
                    "reconcile: stopped runtime had no cache entry; skipping upstream stop event"
                );
            }
        }
    }

    Ok(ReconcileOutcome { started, stopped })
}
