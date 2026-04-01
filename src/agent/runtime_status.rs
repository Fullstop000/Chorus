use std::sync::Arc;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::agent::drivers::all_runtime_drivers;

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

/// Production runtime status provider backed by per-driver probes.
pub struct SystemRuntimeStatusProvider;

impl RuntimeStatusProvider for SystemRuntimeStatusProvider {
    fn list_statuses(&self) -> anyhow::Result<Vec<RuntimeStatus>> {
        all_runtime_drivers()
            .into_iter()
            .map(|driver| driver.detect_runtime_status())
            .collect()
    }

    fn list_models(&self, runtime: &str) -> anyhow::Result<Vec<String>> {
        let driver = all_runtime_drivers()
            .into_iter()
            .find(|driver| driver.id() == runtime)
            .with_context(|| format!("unknown runtime: {runtime}"))?;
        driver.list_models()
    }
}
