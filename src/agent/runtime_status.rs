use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::agent::drivers::{ProbeAuth, RuntimeDriver};
use crate::agent::runtime_catalog::runtime_metadata;
use crate::agent::AgentRuntime;

/// HTTP/UI response shape for one runtime catalog entry plus local auth probe.
/// Returned by [`RuntimeStatusProvider::list_statuses`] and serialized directly to JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCatalogEntry {
    pub runtime: String,
    pub label: String,
    pub order: u32,
    pub reasoning_efforts: Vec<String>,
    pub auth: ProbeAuth,
}

impl RuntimeCatalogEntry {
    pub fn new(runtime: AgentRuntime, auth: ProbeAuth) -> Self {
        let metadata = runtime_metadata(runtime);
        Self {
            runtime: runtime.as_str().to_string(),
            label: metadata.label.to_string(),
            order: metadata.order,
            reasoning_efforts: metadata
                .reasoning_efforts
                .iter()
                .map(|effort| (*effort).to_string())
                .collect(),
            auth,
        }
    }
}

/// Backend service used by HTTP handlers to query local runtime availability.
#[async_trait::async_trait]
pub trait RuntimeStatusProvider: Send + Sync {
    async fn list_statuses(&self) -> anyhow::Result<Vec<RuntimeCatalogEntry>>;
    async fn list_models(&self, runtime: AgentRuntime) -> anyhow::Result<Vec<String>>;
}

/// Shared trait alias used by the server state.
pub type SharedRuntimeStatusProvider = Arc<dyn RuntimeStatusProvider>;

/// Production runtime status provider backed by driver probes.
pub struct SystemRuntimeStatusProvider {
    drivers: HashMap<AgentRuntime, Arc<dyn RuntimeDriver>>,
}

impl SystemRuntimeStatusProvider {
    pub fn new(drivers: HashMap<AgentRuntime, Arc<dyn RuntimeDriver>>) -> Self {
        Self { drivers }
    }
}

#[async_trait::async_trait]
impl RuntimeStatusProvider for SystemRuntimeStatusProvider {
    async fn list_statuses(&self) -> anyhow::Result<Vec<RuntimeCatalogEntry>> {
        let mut statuses = Vec::with_capacity(self.drivers.len());
        for (runtime, driver) in &self.drivers {
            let probe = driver.probe().await?;
            statuses.push(RuntimeCatalogEntry::new(*runtime, probe.auth));
        }
        statuses.sort_by_key(|status| status.order);
        Ok(statuses)
    }

    async fn list_models(&self, runtime: AgentRuntime) -> anyhow::Result<Vec<String>> {
        let driver = self
            .drivers
            .get(&runtime)
            .with_context(|| format!("no driver registered for runtime: {}", runtime.as_str()))?;
        let models = driver.list_models().await?;
        Ok(models.into_iter().map(|m| m.id).collect())
    }
}
