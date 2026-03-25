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

    /// Path for agent's private team memory: <agents_dir>/<agent>/teams/<team>/
    pub fn team_memory_path(&self, agent_name: &str, team_name: &str) -> PathBuf {
        self.agents_dir
            .join(agent_name)
            .join("teams")
            .join(team_name)
    }

    /// Create per-team memory dir + ROLE.md stub for an agent.
    pub fn init_team_memory(
        &self,
        agent_name: &str,
        team_name: &str,
        role: &str,
    ) -> std::io::Result<()> {
        let dir = self.team_memory_path(agent_name, team_name);
        std::fs::create_dir_all(&dir)?;
        let role_md = dir.join("ROLE.md");
        if !role_md.exists() {
            std::fs::write(
                &role_md,
                format!("# Role in {team_name}\n\n**Role:** {role}\n\n## Responsibilities\n\n_Document your responsibilities in this team here._\n"),
            )?;
        }
        Ok(())
    }

    /// Remove agent's private team memory directory.
    pub fn delete_team_memory(&self, agent_name: &str, team_name: &str) -> std::io::Result<()> {
        let path = self.team_memory_path(agent_name, team_name);
        if path.exists() {
            std::fs::remove_dir_all(path)?;
        }
        Ok(())
    }
}

/// Filesystem helper for team workspace operations.
pub struct TeamWorkspace {
    teams_dir: PathBuf,
}

impl TeamWorkspace {
    pub fn new(teams_dir: PathBuf) -> Self {
        Self { teams_dir }
    }

    pub fn team_path(&self, team_name: &str) -> PathBuf {
        self.teams_dir.join(team_name)
    }

    pub fn member_path(&self, team_name: &str, agent_name: &str) -> PathBuf {
        self.teams_dir
            .join(team_name)
            .join("members")
            .join(agent_name)
    }

    /// Create team workspace skeleton with TEAM.md stub.
    pub fn init_team(&self, team_name: &str, members: &[&str]) -> std::io::Result<()> {
        let team_dir = self.team_path(team_name);
        std::fs::create_dir_all(team_dir.join("shared"))?;
        for member in members {
            std::fs::create_dir_all(self.member_path(team_name, member))?;
        }
        let team_md = team_dir.join("TEAM.md");
        if !team_md.exists() {
            std::fs::write(
                &team_md,
                format!("# Team: {}\n\n## Purpose\n\n_Describe the team's purpose here._\n\n## Members\n\n{}\n",
                    team_name,
                    members.iter().map(|m| format!("- {m}")).collect::<Vec<_>>().join("\n")),
            )?;
        }
        Ok(())
    }

    /// Add a member directory to an existing team workspace.
    pub fn init_member(&self, team_name: &str, agent_name: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(self.member_path(team_name, agent_name))
    }

    /// Remove the entire team workspace directory.
    pub fn delete_team(&self, team_name: &str) -> std::io::Result<()> {
        let path = self.team_path(team_name);
        if path.exists() {
            std::fs::remove_dir_all(path)?;
        }
        Ok(())
    }
}
