//! Reconcile a `bridge.target` frame against the locally-running agent set.
//!
//! On every target update: insert/update local records, start any newly
//! desired agents, stop any that vanished from the target. Returns the
//! `(name, platform_id)` pairs the caller should report upstream as
//! `agent.state` frames.

use std::collections::HashSet;

use crate::agent::manager::AgentManager;
use crate::store::Store;

use super::local_store::upsert_from_target;
use super::ws::AgentTargetIn;

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

pub async fn apply(
    store: &Store,
    manager: &AgentManager,
    targets: Vec<AgentTargetIn>,
) -> anyhow::Result<ReconcileOutcome> {
    let mut desired: HashSet<String> = HashSet::new();

    let mut started = Vec::new();
    let mut stopped = Vec::new();

    for target in &targets {
        desired.insert(target.name.clone());
        upsert_from_target(store, target)?;
    }

    let running = manager.get_running_agent_names().await;
    let running_set: HashSet<String> = running.iter().cloned().collect();

    // Start every desired agent that isn't already running.
    for target in &targets {
        if running_set.contains(&target.name) {
            continue;
        }
        match manager
            .start_agent(&target.name, None, target.init_directive.clone())
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

    // Stop any locally-running agent that is no longer in the desired set.
    // The manager-side stop runs unconditionally — leaving a runtime alive
    // because the local DB row vanished early would orphan an OS process.
    // The upstream `agent.state{stopped}` frame is conditional: skip it
    // (with a debug log) when we can't resolve the platform_id, since
    // there's nothing the platform can do with a nameless transition.
    for name in running.iter() {
        if desired.contains(name) {
            continue;
        }
        let platform_id = store.get_agent(name)?.map(|a| a.id);
        if let Err(e) = manager.stop_agent(name).await {
            tracing::warn!(agent = %name, err = %e, "stop_agent failed during reconcile");
        }
        match platform_id {
            Some(platform_id) => {
                stopped.push(AgentTransition {
                    name: name.clone(),
                    platform_id,
                });
            }
            None => {
                tracing::debug!(
                    agent = %name,
                    "reconcile: stopped runtime had no local row; skipping upstream stop event"
                );
            }
        }
        store.delete_agent_record(name)?;
    }

    Ok(ReconcileOutcome { started, stopped })
}
