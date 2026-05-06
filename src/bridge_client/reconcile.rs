//! Reconcile a `bridge.target` frame against the locally-running agent set.
//!
//! On every target update: insert/update local records, start any newly
//! desired agents, stop any that vanished from the target. Returns the set
//! of `(name, platform_id, transition)` events the WS sender should push
//! upstream as `agent.state` frames.

use std::collections::HashSet;
use std::sync::Arc;

use crate::agent::manager::AgentManager;
use crate::store::Store;

use super::local_store::{self, AgentIdMap};
use super::ws::AgentTargetIn;

/// Result of one reconcile pass. `started`/`stopped` carry agent names that
/// the caller should report upstream as `agent.state` frames; `pids` is the
/// pid we should attach to a `started` event when the runtime exposes one.
pub struct ReconcileOutcome {
    pub started: Vec<String>,
    pub stopped: Vec<String>,
}

pub async fn apply(
    store: &Arc<Store>,
    manager: &Arc<AgentManager>,
    id_map: &mut AgentIdMap,
    targets: Vec<AgentTargetIn>,
) -> anyhow::Result<ReconcileOutcome> {
    let mut desired: HashSet<String> = HashSet::new();

    let mut started = Vec::new();
    let mut stopped = Vec::new();

    for target in &targets {
        desired.insert(target.name.clone());
        local_store::upsert_from_target(store, target)?;
        id_map.record(target.name.clone(), target.agent_id.clone());
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
                started.push(target.name.clone());
            }
            Err(e) => {
                tracing::error!(agent = %target.name, err = %e, "start_agent failed during reconcile");
            }
        }
    }

    // Stop any locally-running agent that is no longer in the desired set.
    for name in running.iter() {
        if desired.contains(name) {
            continue;
        }
        if let Err(e) = manager.stop_agent(name).await {
            tracing::warn!(agent = %name, err = %e, "stop_agent failed during reconcile");
        }
        stopped.push(name.clone());
        local_store::delete_record(store, name)?;
        id_map.forget(name);
    }

    Ok(ReconcileOutcome { started, stopped })
}
