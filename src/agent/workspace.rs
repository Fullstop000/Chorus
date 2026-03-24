use std::path::{Path, PathBuf};

/// Filesystem helper for per-agent workspace operations.
pub struct AgentWorkspace<'a> {
    agents_dir: &'a Path,
}

impl<'a> AgentWorkspace<'a> {
    pub fn new(agents_dir: &'a Path) -> Self {
        Self { agents_dir }
    }

    /// Resolve the stable workspace path for an agent's persisted files.
    pub fn path_for(&self, agent_name: &str) -> PathBuf {
        self.agents_dir.join(agent_name)
    }

    /// Remove the workspace directory if it exists.
    pub fn delete_if_exists(&self, agent_name: &str) -> std::io::Result<()> {
        let path = self.path_for(agent_name);
        if path.exists() {
            std::fs::remove_dir_all(path)?;
        }
        Ok(())
    }
}
