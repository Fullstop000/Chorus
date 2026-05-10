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
    pub fn path_for(&self, workspace_id: &str, agent_name: &str, agent_id: &str) -> PathBuf {
        self.agents_dir
            .join(workspace_id)
            .join(format!("{}-{}", agent_name, agent_id))
    }

    /// Remove the workspace directory if it exists.
    pub fn delete_if_exists(
        &self,
        workspace_id: &str,
        agent_name: &str,
        agent_id: &str,
    ) -> std::io::Result<()> {
        let path = self.path_for(workspace_id, agent_name, agent_id);
        if path.exists() {
            std::fs::remove_dir_all(path)?;
        }
        Ok(())
    }

    /// Path for agent's private team memory:
    /// <agents_dir>/<workspace_id>/<agent>-<agent_id>/teams/<team>-<team_id>/
    pub fn team_memory_path(
        &self,
        workspace_id: &str,
        agent_name: &str,
        agent_id: &str,
        team_name: &str,
        team_id: &str,
    ) -> PathBuf {
        self.path_for(workspace_id, agent_name, agent_id)
            .join("teams")
            .join(format!("{}-{}", team_name, team_id))
    }

    /// Create per-team memory dir + ROLE.md stub for an agent.
    pub fn init_team_memory(
        &self,
        workspace_id: &str,
        agent_name: &str,
        agent_id: &str,
        team_name: &str,
        team_id: &str,
        role: &str,
    ) -> std::io::Result<()> {
        let dir = self.team_memory_path(workspace_id, agent_name, agent_id, team_name, team_id);
        std::fs::create_dir_all(&dir)?;
        let role_md = dir.join("ROLE.md");
        if !role_md.exists() {
            std::fs::write(
                &role_md,
                format!(
                    "# Role in {team_name}\n\n**Role:** {role}\n\n## Responsibilities\n\n_Document your responsibilities in this team here._\n",
                ),
            )?;
        }
        Ok(())
    }

    /// Update the persisted role line in an agent's private team memory.
    pub fn set_team_role(
        &self,
        workspace_id: &str,
        agent_name: &str,
        agent_id: &str,
        team_name: &str,
        team_id: &str,
        role: &str,
    ) -> std::io::Result<()> {
        let dir = self.team_memory_path(workspace_id, agent_name, agent_id, team_name, team_id);
        std::fs::create_dir_all(&dir)?;
        let role_md = dir.join("ROLE.md");
        if !role_md.exists() {
            return self.init_team_memory(
                workspace_id,
                agent_name,
                agent_id,
                team_name,
                team_id,
                role,
            );
        }

        let current = std::fs::read_to_string(&role_md)?;
        let mut replaced = false;
        let updated = current
            .lines()
            .map(|line| {
                if line.starts_with("**Role:** ") {
                    replaced = true;
                    format!("**Role:** {role}")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        let next = if replaced {
            updated
        } else {
            format!("{updated}\n\n**Role:** {role}\n")
        };
        std::fs::write(role_md, next)?;
        Ok(())
    }

    /// Remove agent's private team memory directory.
    pub fn delete_team_memory(
        &self,
        workspace_id: &str,
        agent_name: &str,
        agent_id: &str,
        team_name: &str,
        team_id: &str,
    ) -> std::io::Result<()> {
        let new_path =
            self.team_memory_path(workspace_id, agent_name, agent_id, team_name, team_id);
        if new_path.exists() {
            std::fs::remove_dir_all(new_path)?;
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

    pub fn team_path(&self, workspace_id: &str, team_name: &str, team_id: &str) -> PathBuf {
        self.teams_dir
            .join(workspace_id)
            .join(format!("{}-{}", team_name, team_id))
    }

    pub fn member_path(
        &self,
        workspace_id: &str,
        team_name: &str,
        team_id: &str,
        agent_name: &str,
        agent_id: &str,
    ) -> PathBuf {
        self.team_path(workspace_id, team_name, team_id)
            .join("members")
            .join(format!("{}-{}", agent_name, agent_id))
    }

    /// Create team workspace skeleton with TEAM.md stub.
    pub fn init_team(
        &self,
        workspace_id: &str,
        team_name: &str,
        team_id: &str,
        members: &[(&str, &str)],
    ) -> std::io::Result<()> {
        let team_dir = self.team_path(workspace_id, team_name, team_id);
        std::fs::create_dir_all(team_dir.join("shared"))?;
        for (agent_name, agent_id) in members {
            std::fs::create_dir_all(self.member_path(
                workspace_id,
                team_name,
                team_id,
                agent_name,
                agent_id,
            ))?;
        }
        let team_md = team_dir.join("TEAM.md");
        if !team_md.exists() {
            std::fs::write(
                &team_md,
                format!(
                    "# Team: {}\n\n## Purpose\n\n_Describe the team's purpose here._\n\n## Members\n\n{}\n",
                    team_name,
                    members.iter().map(|(m, _)| format!("- {m}")).collect::<Vec<_>>().join("\n")
                ),
            )?;
        }
        Ok(())
    }

    /// Add a member directory to an existing team workspace.
    pub fn init_member(
        &self,
        workspace_id: &str,
        team_name: &str,
        team_id: &str,
        agent_name: &str,
        agent_id: &str,
    ) -> std::io::Result<()> {
        std::fs::create_dir_all(self.member_path(
            workspace_id,
            team_name,
            team_id,
            agent_name,
            agent_id,
        ))
    }

    /// Remove the entire team workspace directory.
    pub fn delete_team(
        &self,
        workspace_id: &str,
        team_name: &str,
        team_id: &str,
    ) -> std::io::Result<()> {
        let path = self.team_path(workspace_id, team_name, team_id);
        if path.exists() {
            std::fs::remove_dir_all(path)?;
        }
        Ok(())
    }
}
