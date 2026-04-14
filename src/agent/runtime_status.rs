use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::agent::drivers::v2::{ProbeAuth, RuntimeDriver};
use crate::agent::AgentRuntime;

/// Authentication state for a locally installed agent runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAuthStatus {
    Authed,
    Unauthed,
}

/// Installation and authentication summary for one supported runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub runtime: String,
    pub installed: bool,
    #[serde(rename = "authStatus", skip_serializing_if = "Option::is_none")]
    pub auth_status: Option<RuntimeAuthStatus>,
}

/// Backend service used by HTTP handlers to query local runtime availability.
pub trait RuntimeStatusProvider: Send + Sync {
    fn list_statuses(&self) -> anyhow::Result<Vec<RuntimeStatus>>;
    fn list_models(&self, runtime: &str) -> anyhow::Result<Vec<String>>;
}

/// Shared trait alias used by the server state.
pub type SharedRuntimeStatusProvider = Arc<dyn RuntimeStatusProvider>;

/// Production runtime status provider backed by v2 driver probes.
pub struct SystemRuntimeStatusProvider {
    drivers: HashMap<AgentRuntime, Arc<dyn RuntimeDriver>>,
}

impl SystemRuntimeStatusProvider {
    pub fn new(drivers: HashMap<AgentRuntime, Arc<dyn RuntimeDriver>>) -> Self {
        Self { drivers }
    }
}

impl RuntimeStatusProvider for SystemRuntimeStatusProvider {
    fn list_statuses(&self) -> anyhow::Result<Vec<RuntimeStatus>> {
        let rt = tokio::runtime::Handle::current();
        self.drivers
            .iter()
            .map(|(runtime, driver)| {
                let probe = rt.block_on(driver.probe())?;
                Ok(RuntimeStatus {
                    runtime: runtime.as_str().to_string(),
                    installed: probe.auth != ProbeAuth::NotInstalled,
                    auth_status: match probe.auth {
                        ProbeAuth::NotInstalled => None,
                        ProbeAuth::Unauthed => Some(RuntimeAuthStatus::Unauthed),
                        ProbeAuth::Authed => Some(RuntimeAuthStatus::Authed),
                    },
                })
            })
            .collect()
    }

    fn list_models(&self, runtime: &str) -> anyhow::Result<Vec<String>> {
        let rt_enum =
            AgentRuntime::parse(runtime).with_context(|| format!("unknown runtime: {runtime}"))?;
        let driver = self
            .drivers
            .get(&rt_enum)
            .with_context(|| format!("unknown runtime: {runtime}"))?;
        let rt = tokio::runtime::Handle::current();
        let models = rt.block_on(driver.list_models())?;
        Ok(models.into_iter().map(|m| m.id).collect())
    }
}
