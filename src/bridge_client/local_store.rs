//! Bridge-side persistence: synced agent records keyed by name. The bridge
//! does NOT store messages — chat lives on the platform; agents pull via
//! MCP. The bridge stores only what `AgentManager::start_agent` needs to
//! read (`store.get_agent(name)`).

use std::collections::HashMap;

use crate::store::agents::{AgentEnvVar, AgentRecordUpsert};
use crate::store::Store;

use super::ws::AgentTargetIn;

/// Mapping from local agent name → platform's agent UUID, kept in memory.
/// Used to translate `chat.message.received{agent_id: <platform_uuid>}` into
/// the local name we drive `AgentManager` with.
#[derive(Default)]
pub struct AgentIdMap {
    pub name_by_platform_id: HashMap<String, String>,
    pub platform_id_by_name: HashMap<String, String>,
}

impl AgentIdMap {
    pub fn record(&mut self, name: String, platform_id: String) {
        self.name_by_platform_id
            .insert(platform_id.clone(), name.clone());
        self.platform_id_by_name.insert(name, platform_id);
    }

    pub fn forget(&mut self, name: &str) {
        if let Some(platform_id) = self.platform_id_by_name.remove(name) {
            self.name_by_platform_id.remove(&platform_id);
        }
    }

    pub fn name_for(&self, platform_id: &str) -> Option<&str> {
        self.name_by_platform_id
            .get(platform_id)
            .map(String::as_str)
    }

    pub fn platform_id_for(&self, name: &str) -> Option<&str> {
        self.platform_id_by_name.get(name).map(String::as_str)
    }
}

/// Insert or update the local agent record so `AgentManager::start_agent`
/// can read it back. Returns true if the record was newly created.
pub fn upsert_from_target(store: &Store, target: &AgentTargetIn) -> anyhow::Result<bool> {
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

    if store.get_agent(&target.name)?.is_some() {
        store.update_agent_record(&record)?;
        Ok(false)
    } else {
        store.create_agent_record(&record)?;
        Ok(true)
    }
}

pub fn delete_record(store: &Store, name: &str) -> anyhow::Result<()> {
    store.delete_agent_record(name)?;
    Ok(())
}
