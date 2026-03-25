# Team Feature Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a first-class Team concept to Chorus — a named group of agents + optional humans with a shared channel, shared workspace, and pluggable collaboration model (Leader+Operators or Swarm).

**Architecture:** Server owns the structural layer (team registry, channel creation, workspace setup, @mention forwarding); agent intelligence drives actual collaboration. A `CollaborationModel` trait abstracts Leader+Operators and Swarm protocols. Swarm adds a deliberation phase gated by consensus signals (`READY:`) tracked in SQLite.

**Tech Stack:** Rust/Axum (backend), SQLite/rusqlite, React/TypeScript (frontend), Vite

**Spec:** `docs/superpowers/specs/2026-03-25-team-design.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/store/mod.rs` | Modify | Add `teams` module, 4 new tables in schema, `forwarded_from` migration, `Store::teams_dir()` |
| `src/store/teams.rs` | Create | Team + member CRUD, signal + quorum tracking, `list_teams_for_agent` |
| `src/store/channels.rs` | Modify | `ChannelType::Team` variant + all guard updates |
| `src/store/messages.rs` | Modify | `ForwardedFrom` struct + field on `Message` + `ReceivedMessage` |
| `src/agent/workspace.rs` | Modify | `TeamWorkspace` struct + `AgentWorkspace::team_memory_path` |
| `src/agent/collaboration.rs` | Create | `CollaborationModel` trait + `LeaderOperators` + `Swarm` + `make_collaboration_model` |
| `src/agent/drivers/prompt.rs` | Modify | `TeamMembership` struct + `teams` field on `PromptOptions` + `## Your Teams` rendering |
| `src/agent/mod.rs` | Modify | Re-export `collaboration` module |
| `src/agent/manager.rs` | Modify | Populate `PromptOptions::teams` from store at spawn |
| `src/server/handlers/teams.rs` | Create | REST handlers for all 7 team endpoints |
| `src/server/handlers/mod.rs` | Modify | `pub mod teams` + `pub use teams::*` |
| `src/server/handlers/messages.rs` | Modify | @mention scan, forwarding, quorum snapshot, consensus signal detection |
| `src/server/mod.rs` | Modify | Register team routes |
| `tests/store_tests.rs` | Modify | Team store unit tests |
| `tests/server_tests.rs` | Modify | Team REST API integration tests + `MockLifecycle` stub update |
| `ui/src/types.ts` | Modify | `Team`, `TeamMember`, `ChannelInfo.channel_type = 'team'` |
| `ui/src/api.ts` | Modify | Team API functions |
| `ui/src/store.tsx` | Modify | Team state + actions |
| `ui/src/components/Sidebar.tsx` | Modify | `[team]` badge rendering |
| `ui/src/components/CreateChannelModal.tsx` | Modify | Channel/Team toggle; team fields |
| `ui/src/components/TeamSettings.tsx` | Create | Team management panel in channel header |
| `ui/src/components/MentionTextarea.tsx` | Modify | Include teams in @mention autocomplete |

---

## Task 1: Schema — new tables + ChannelType::Team + ForwardedFrom

**Files:**
- Modify: `src/store/mod.rs`
- Modify: `src/store/channels.rs`
- Modify: `src/store/messages.rs`
- Test: `tests/store_tests.rs`

- [ ] **Step 1: Write failing test for new tables existing**

Add to `tests/store_tests.rs`:

```rust
#[test]
fn test_team_tables_exist() {
    let store = Store::open(":memory:").unwrap();
    let conn = store.conn_for_test(); // we'll expose this below
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('teams','team_members','team_task_signals','team_task_quorum')",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(count, 4);
}
```

Run: `cargo test test_team_tables_exist 2>&1 | tail -5`
Expected: compile error (no `conn_for_test`)

- [ ] **Step 2: Expose `conn_for_test` in Store (test-only)**

In `src/store/mod.rs`, add after the `pub fn data_dir` method:

```rust
#[cfg(test)]
pub fn conn_for_test(&self) -> std::sync::MutexGuard<rusqlite::Connection> {
    self.conn.lock().unwrap()
}
```

- [ ] **Step 3: Add 4 new tables to `init_schema`**

In `src/store/mod.rs`, append to the `execute_batch` string inside `init_schema`:

```sql
CREATE TABLE IF NOT EXISTS teams (
    id TEXT PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    display_name TEXT NOT NULL,
    collaboration_model TEXT NOT NULL,
    leader_agent_name TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS team_members (
    team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    member_name TEXT NOT NULL,
    member_type TEXT NOT NULL,
    member_id TEXT NOT NULL,
    role TEXT NOT NULL,
    joined_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (team_id, member_name)
);
CREATE TABLE IF NOT EXISTS team_task_signals (
    id TEXT PRIMARY KEY,
    team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    trigger_message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    member_name TEXT NOT NULL,
    signal TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (trigger_message_id, member_name)
);
CREATE TABLE IF NOT EXISTS team_task_quorum (
    trigger_message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    member_name TEXT NOT NULL,
    resolved_at TEXT,
    PRIMARY KEY (trigger_message_id, member_name)
);
```

- [ ] **Step 4: Add `forwarded_from` migration**

After the existing `.ok()` migration calls in `init_schema`, add:

```rust
conn.execute(
    "ALTER TABLE messages ADD COLUMN forwarded_from TEXT",
    [],
).ok();
```

- [ ] **Step 5: Add `ChannelType::Team` variant**

In `src/store/channels.rs`:

```rust
pub enum ChannelType {
    Channel,
    Dm,
    System,
    Team,
}
```

Update `create_channel` match:
```rust
ChannelType::Team => "team",
```

Update `channel_from_row` in `src/store/mod.rs`:
```rust
"team" => ChannelType::Team,
"dm" => ChannelType::Dm,
"system" => ChannelType::System,
_ => ChannelType::Channel,
```

Update `list_channels` query:
```sql
WHERE channel_type IN ('channel', 'team') AND archived = 0
```

Update `archive_channel` guard:
```rust
if !matches!(channel.channel_type, ChannelType::Channel | ChannelType::Team) {
    return Err(anyhow!("only user and team channels can be archived"));
}
```

Update `update_channel` guard similarly.

Update `delete_channel` guard to explicitly reject team channels:
```rust
if channel.channel_type == ChannelType::Team {
    return Err(anyhow!("team channels cannot be deleted directly; delete the team instead"));
}
if channel.channel_type != ChannelType::Channel {
    return Err(anyhow!("only user channels can be deleted"));
}
```

- [ ] **Step 6: Add `ForwardedFrom` struct and field**

In `src/store/messages.rs`, add after the imports:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardedFrom {
    pub channel_name: String,
    pub sender_name: String,
}
```

Add field to `Message`:
```rust
pub forwarded_from: Option<ForwardedFrom>,
```

Add field to `ReceivedMessage`:
```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub forwarded_from: Option<ForwardedFrom>,
```

Update all places in `messages.rs` where `Message` is constructed from a row to parse `forwarded_from` from the new column (use `serde_json::from_str` on the TEXT value, default to `None` on parse failure).

- [ ] **Step 7: Add `Store::teams_dir()`**

In `src/store/mod.rs`:
```rust
pub fn teams_dir(&self) -> PathBuf {
    self.data_dir.join("teams")
}
```

- [ ] **Step 8: Run test to confirm it passes**

Run: `cargo test test_team_tables_exist 2>&1 | tail -5`
Expected: `test test_team_tables_exist ... ok`

- [ ] **Step 9: Run full test suite to confirm no regressions**

Run: `cargo test 2>&1 | tail -10`
Expected: all tests pass

- [ ] **Step 10: Commit**

```bash
git add src/store/mod.rs src/store/channels.rs src/store/messages.rs tests/store_tests.rs
git commit -m "feat(store): add team tables, ChannelType::Team, ForwardedFrom"
```

---

## Task 2: Store — teams module (CRUD + signal/quorum)

**Files:**
- Create: `src/store/teams.rs`
- Modify: `src/store/mod.rs`
- Test: `tests/store_tests.rs`

- [ ] **Step 1: Write failing tests for team CRUD**

Add to `tests/store_tests.rs`:

```rust
#[test]
fn test_create_and_get_team() {
    let store = Store::open(":memory:").unwrap();
    let id = store.create_team("eng-team", "Engineering Team", "leader_operators", Some("alice")).unwrap();
    let team = store.get_team("eng-team").unwrap().unwrap();
    assert_eq!(team.id, id);
    assert_eq!(team.name, "eng-team");
    assert_eq!(team.display_name, "Engineering Team");
    assert_eq!(team.collaboration_model, "leader_operators");
    assert_eq!(team.leader_agent_name.as_deref(), Some("alice"));
}

#[test]
fn test_add_and_list_team_members() {
    let store = Store::open(":memory:").unwrap();
    let team_id = store.create_team("eng-team", "Eng", "swarm", None).unwrap();
    store.add_team_member(&team_id, "alice", "agent", "agent-uuid-1", "operator").unwrap();
    store.add_team_member(&team_id, "bob", "human", "bob", "observer").unwrap();
    let members = store.get_team_members(&team_id).unwrap();
    assert_eq!(members.len(), 2);
}

#[test]
fn test_list_teams_for_agent() {
    let store = Store::open(":memory:").unwrap();
    let team_id = store.create_team("eng-team", "Eng", "swarm", None).unwrap();
    store.add_team_member(&team_id, "alice", "agent", "agent-uuid-1", "operator").unwrap();
    let teams = store.list_teams_for_agent("alice").unwrap();
    assert_eq!(teams.len(), 1);
    assert_eq!(teams[0].name, "eng-team");
}

#[test]
fn test_delete_team_cascades() {
    let store = Store::open(":memory:").unwrap();
    let team_id = store.create_team("eng-team", "Eng", "swarm", None).unwrap();
    store.add_team_member(&team_id, "alice", "agent", "uuid-1", "operator").unwrap();
    store.delete_team(&team_id).unwrap();
    assert!(store.get_team("eng-team").unwrap().is_none());
    // member row should be gone
    let conn = store.conn_for_test();
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM team_members WHERE team_id = ?1",
        rusqlite::params![team_id],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(count, 0);
}
```

Run: `cargo test test_create_and_get_team 2>&1 | tail -5`
Expected: compile error (methods don't exist yet)

- [ ] **Step 2: Create `src/store/teams.rs`**

```rust
use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{parse_datetime, Store};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub collaboration_model: String,
    pub leader_agent_name: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    pub team_id: String,
    pub member_name: String,
    pub member_type: String,
    pub member_id: String,
    pub role: String,
    pub joined_at: DateTime<Utc>,
}

/// Summary of a team membership for use in agent system prompts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMembership {
    pub team_name: String,
    pub role: String,
}

fn team_from_row(row: &rusqlite::Row) -> rusqlite::Result<Team> {
    Ok(Team {
        id: row.get(0)?,
        name: row.get(1)?,
        display_name: row.get(2)?,
        collaboration_model: row.get(3)?,
        leader_agent_name: row.get(4)?,
        created_at: parse_datetime(&row.get::<_, String>(5)?),
    })
}

impl Store {
    pub fn create_team(
        &self,
        name: &str,
        display_name: &str,
        collaboration_model: &str,
        leader_agent_name: Option<&str>,
    ) -> Result<String> {
        let conn = self.conn.lock().unwrap();
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO teams (id, name, display_name, collaboration_model, leader_agent_name)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, name, display_name, collaboration_model, leader_agent_name],
        )?;
        Ok(id)
    }

    pub fn get_team(&self, name: &str) -> Result<Option<Team>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, display_name, collaboration_model, leader_agent_name, created_at
             FROM teams WHERE name = ?1",
        )?;
        let mut rows = stmt.query_map(params![name], team_from_row)?;
        Ok(rows.next().transpose()?)
    }

    pub fn get_team_by_id(&self, id: &str) -> Result<Option<Team>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, display_name, collaboration_model, leader_agent_name, created_at
             FROM teams WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], team_from_row)?;
        Ok(rows.next().transpose()?)
    }

    pub fn list_teams(&self) -> Result<Vec<Team>> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .prepare(
                "SELECT id, name, display_name, collaboration_model, leader_agent_name, created_at
                 FROM teams ORDER BY name",
            )?
            .query_map([], team_from_row)?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    pub fn update_team(
        &self,
        id: &str,
        display_name: &str,
        collaboration_model: &str,
        leader_agent_name: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "UPDATE teams SET display_name = ?1, collaboration_model = ?2, leader_agent_name = ?3
             WHERE id = ?4",
            params![display_name, collaboration_model, leader_agent_name, id],
        )?;
        if n == 0 {
            return Err(anyhow!("team not found: {}", id));
        }
        Ok(())
    }

    pub fn delete_team(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM teams WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn add_team_member(
        &self,
        team_id: &str,
        member_name: &str,
        member_type: &str,
        member_id: &str,
        role: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO team_members (team_id, member_name, member_type, member_id, role)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![team_id, member_name, member_type, member_id, role],
        )?;
        Ok(())
    }

    pub fn remove_team_member(&self, team_id: &str, member_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM team_members WHERE team_id = ?1 AND member_name = ?2",
            params![team_id, member_name],
        )?;
        Ok(())
    }

    /// Remove a member from a channel by channel name. Used when removing a team member.
    pub fn leave_channel(&self, channel_name: &str, member_name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        if let Some(ch) = Self::find_channel_by_name_inner(&conn, channel_name)? {
            conn.execute(
                "DELETE FROM channel_members WHERE channel_id = ?1 AND member_name = ?2",
                params![ch.id, member_name],
            )?;
        }
        Ok(())
    }

    pub fn get_team_members(&self, team_id: &str) -> Result<Vec<TeamMember>> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .prepare(
                "SELECT team_id, member_name, member_type, member_id, role, joined_at
                 FROM team_members WHERE team_id = ?1 ORDER BY member_name",
            )?
            .query_map(params![team_id], |row| {
                Ok(TeamMember {
                    team_id: row.get(0)?,
                    member_name: row.get(1)?,
                    member_type: row.get(2)?,
                    member_id: row.get(3)?,
                    role: row.get(4)?,
                    joined_at: parse_datetime(&row.get::<_, String>(5)?),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// List all teams an agent belongs to, with the agent's role in each.
    pub fn list_teams_for_agent(&self, agent_name: &str) -> Result<Vec<TeamMembership>> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .prepare(
                "SELECT t.name, tm.role FROM team_members tm
                 JOIN teams t ON t.id = tm.team_id
                 WHERE tm.member_name = ?1 AND tm.member_type = 'agent'
                 ORDER BY t.name",
            )?
            .query_map(params![agent_name], |row| {
                Ok(TeamMembership {
                    team_name: row.get(0)?,
                    role: row.get(1)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }

    /// Snapshot current agent members into team_task_quorum for a new task.
    pub fn snapshot_swarm_quorum(&self, team_id: &str, trigger_message_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO team_task_quorum (trigger_message_id, team_id, member_name)
             SELECT ?1, team_id, member_name FROM team_members
             WHERE team_id = ?2 AND member_type = 'agent'",
            params![trigger_message_id, team_id],
        )?;
        Ok(())
    }

    /// Record a READY: signal from an agent for a swarm task.
    /// Finds the earliest unresolved trigger for this team and inserts.
    /// Returns true if this signal completes the quorum (consensus reached).
    pub fn record_swarm_signal(
        &self,
        team_id: &str,
        member_name: &str,
        signal: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().unwrap();

        // Find earliest unresolved trigger_message_id for this team.
        let trigger_id: Option<String> = conn
            .prepare(
                "SELECT q.trigger_message_id FROM team_task_quorum q
                 JOIN messages m ON m.id = q.trigger_message_id
                 WHERE q.team_id = ?1 AND q.resolved_at IS NULL
                 ORDER BY m.created_at ASC
                 LIMIT 1",
            )?
            .query_row(params![team_id], |r| r.get(0))
            .ok();

        let trigger_id = match trigger_id {
            None => return Ok(false), // no open quorum — discard signal
            Some(id) => id,
        };

        let signal_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT OR IGNORE INTO team_task_signals (id, team_id, trigger_message_id, member_name, signal)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![signal_id, team_id, trigger_id, member_name, signal],
        )?;

        // Check if quorum is now complete.
        let quorum_size: i64 = conn.query_row(
            "SELECT COUNT(*) FROM team_task_quorum WHERE trigger_message_id = ?1",
            params![trigger_id],
            |r| r.get(0),
        )?;
        let signal_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM team_task_signals WHERE trigger_message_id = ?1",
            params![trigger_id],
            |r| r.get(0),
        )?;

        if signal_count >= quorum_size {
            conn.execute(
                "UPDATE team_task_quorum SET resolved_at = datetime('now')
                 WHERE trigger_message_id = ?1",
                params![trigger_id],
            )?;
            return Ok(true);
        }

        Ok(false)
    }
}
```

- [ ] **Step 3: Register `teams` module in `src/store/mod.rs`**

Add at top:
```rust
pub mod teams;
```

Add to re-exports:
```rust
pub use teams::{Team, TeamMember, TeamMembership};
```

- [ ] **Step 4: Run tests**

Run: `cargo test test_create_and_get_team test_add_and_list_team_members test_list_teams_for_agent test_delete_team_cascades 2>&1 | tail -10`
Expected: all 4 pass

- [ ] **Step 5: Commit**

```bash
git add src/store/teams.rs src/store/mod.rs tests/store_tests.rs
git commit -m "feat(store): add teams module with CRUD and swarm signal tracking"
```

---

## Task 3: Workspace — TeamWorkspace + AgentWorkspace::team_memory_path

**Files:**
- Modify: `src/agent/workspace.rs`

- [ ] **Step 1: Add `TeamWorkspace` and `team_memory_path` to `src/agent/workspace.rs`**

Append to the existing file:

```rust
use std::path::PathBuf;

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
        self.teams_dir.join(team_name).join("members").join(agent_name)
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
```

Also add to the existing `AgentWorkspace` impl:

```rust
/// Path for agent's private team memory: <agents_dir>/<agent>/teams/<team>/
pub fn team_memory_path(&self, agent_name: &str, team_name: &str) -> PathBuf {
    self.agents_dir.join(agent_name).join("teams").join(team_name)
}

/// Create per-team memory dir + ROLE.md stub for an agent.
pub fn init_team_memory(&self, agent_name: &str, team_name: &str, role: &str) -> std::io::Result<()> {
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
```

- [ ] **Step 2: Compile check**

Run: `cargo build 2>&1 | grep -E "^error" | head -10`
Expected: no errors

- [ ] **Step 3: Commit**

```bash
git add src/agent/workspace.rs
git commit -m "feat(workspace): add TeamWorkspace and agent team memory helpers"
```

---

## Task 4: Collaboration models trait

**Files:**
- Create: `src/agent/collaboration.rs`
- Modify: `src/agent/mod.rs`

- [ ] **Step 1: Check existing `src/agent/mod.rs`**

Run: `cat src/agent/mod.rs`

- [ ] **Step 2: Create `src/agent/collaboration.rs`**

```rust
use crate::store::teams::TeamMembership;

/// Pluggable coordination protocol for a team.
pub trait CollaborationModel: Send + Sync {
    /// Role-specific instructions injected into a member's system prompt section.
    fn member_role_prompt(&self, role: &str) -> String;

    /// System message posted into the team channel when a forwarded task arrives.
    /// Returns None if no deliberation phase (e.g. Leader+Operators).
    fn deliberation_prompt(&self) -> Option<String>;

    /// Returns true if the message content is a consensus signal (e.g. starts with "READY:").
    fn is_consensus_signal(&self, content: &str) -> bool;
}

/// Factory: returns the right CollaborationModel for the stored string.
/// Unknown values fall back to LeaderOperators.
pub fn make_collaboration_model(model: &str) -> Box<dyn CollaborationModel> {
    match model {
        "swarm" => Box::new(Swarm),
        _ => Box::new(LeaderOperators),
    }
}

// ── Leader + Operators ──

pub struct LeaderOperators;

impl CollaborationModel for LeaderOperators {
    fn member_role_prompt(&self, role: &str) -> String {
        match role {
            "leader" => {
                "You are the **leader** of this team. When a task arrives:\n\
                 1. Decompose it into subtasks.\n\
                 2. Delegate each subtask to an operator via DM or a channel message.\n\
                 3. Synthesize the results and post a summary back to the channel where the task originated.\n\
                 Do not execute subtasks yourself — delegate and coordinate.".to_string()
            }
            _ => {
                "You are an **operator** in this team. Wait for task delegation from the leader. \
                 Execute your assigned subtask and report back to the leader when done.".to_string()
            }
        }
    }

    fn deliberation_prompt(&self) -> Option<String> {
        None
    }

    fn is_consensus_signal(&self, _content: &str) -> bool {
        false
    }
}

// ── Swarm ──

pub struct Swarm;

impl CollaborationModel for Swarm {
    fn member_role_prompt(&self, _role: &str) -> String {
        "You are a **swarm member** of this team. When a task arrives:\n\
         1. Read the task and discuss the best approach with your teammates in the channel.\n\
         2. When you have decided what your part of the work is, post a message starting with \
            `READY: ` followed by a brief description of your assigned subtask.\n\
         3. Once all members have posted READY:, the system will confirm and you should begin your subtask.\n\
         Do not start work before the system posts the GO message.".to_string()
    }

    fn deliberation_prompt(&self) -> Option<String> {
        Some(
            "**New task received.** Discuss the best approach with your teammates. \
             When you are ready to proceed, reply with `READY: <brief description of your assigned subtask>`. \
             Execution will begin once all members have confirmed.".to_string(),
        )
    }

    fn is_consensus_signal(&self, content: &str) -> bool {
        content.trim_start().starts_with("READY:")
    }
}

/// Build the `## Your Teams` section for an agent's system prompt.
pub fn build_teams_prompt_section(memberships: &[TeamMembership]) -> String {
    if memberships.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = memberships
        .iter()
        .map(|m| format!("- #{} — role: {}", m.team_name, m.role))
        .collect();
    format!("## Your Teams\n{}\n", lines.join("\n"))
}
```

- [ ] **Step 3: Register module in `src/agent/mod.rs`**

Add:
```rust
pub mod collaboration;
```

- [ ] **Step 4: Compile check**

Run: `cargo build 2>&1 | grep "^error" | head -10`
Expected: no errors

- [ ] **Step 5: Commit**

```bash
git add src/agent/collaboration.rs src/agent/mod.rs
git commit -m "feat(agent): add CollaborationModel trait with LeaderOperators and Swarm"
```

---

## Task 5: System prompt — team membership injection

**Files:**
- Modify: `src/agent/drivers/prompt.rs`
- Modify: `src/agent/manager.rs`

- [ ] **Step 1: Read the full `prompt.rs` to understand current `PromptOptions`**

Run: `cat src/agent/drivers/prompt.rs`

- [ ] **Step 2: Add `teams` field to `PromptOptions`**

In `src/agent/drivers/prompt.rs`, update the struct:

```rust
use crate::store::teams::TeamMembership;

pub struct PromptOptions {
    pub tool_prefix: String,
    pub extra_critical_rules: Vec<String>,
    pub post_startup_notes: Vec<String>,
    pub include_stdin_notification_section: bool,
    pub teams: Vec<TeamMembership>,  // new
}
```

- [ ] **Step 3: Render `## Your Teams` section in `build_base_system_prompt`**

Find where the prompt string is assembled. Before the return, append the teams section:

```rust
let teams_section = crate::agent::collaboration::build_teams_prompt_section(&opts.teams);
```

Add `teams_section` into the prompt body after the identity/startup section (exact placement depends on current structure — insert as a new `##` section near the end of the identity block).

- [ ] **Step 4: Update all callers of `PromptOptions` to add `teams: vec![]`**

Run: `cargo build 2>&1 | grep "^error" | head -20`

For each compile error about missing `teams` field, add `teams: vec![]` as a default. This will be updated in the next step for the real spawn path.

- [ ] **Step 5: Populate `teams` at agent spawn in `manager.rs`**

In `src/agent/manager.rs`, find where `PromptOptions` is constructed before spawn. Add:

```rust
let team_memberships = self.store.list_teams_for_agent(agent_name).unwrap_or_default();
```

Then pass `teams: team_memberships` into `PromptOptions`.

- [ ] **Step 6: Compile and run tests**

Run: `cargo test 2>&1 | tail -10`
Expected: all pass

- [ ] **Step 7: Commit**

```bash
git add src/agent/drivers/prompt.rs src/agent/manager.rs
git commit -m "feat(prompt): inject team membership into agent system prompt"
```

---

## Task 6: Team REST handlers

**Files:**
- Create: `src/server/handlers/teams.rs`
- Modify: `src/server/handlers/mod.rs`
- Modify: `src/server/mod.rs`
- Test: `tests/server_tests.rs`

- [ ] **Step 1: Write failing server tests for team endpoints**

Add to `tests/server_tests.rs`:

```rust
#[tokio::test]
async fn test_create_team_endpoint() {
    let (store, app, _lifecycle) = setup_with_lifecycle();
    let body = serde_json::json!({
        "name": "eng-team",
        "display_name": "Engineering Team",
        "collaboration_model": "leader_operators",
        "leader_agent_name": null,
        "members": []
    });
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/teams")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    // Channel should exist
    let ch = store.find_channel_by_name("eng-team").unwrap();
    assert!(ch.is_some());
    assert_eq!(ch.unwrap().channel_type, crate::store::channels::ChannelType::Team);
}

#[tokio::test]
async fn test_list_teams_endpoint() {
    let (_store, app, _lifecycle) = setup_with_lifecycle();
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("GET")
                .uri("/api/teams")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}
```

Run: `cargo test test_create_team_endpoint 2>&1 | tail -5`
Expected: compile error (route not registered)

- [ ] **Step 2: Create `src/server/handlers/teams.rs`**

```rust
use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use super::{api_err, internal_err, ApiResult, AppState};
use crate::store::channels::ChannelType;
use crate::store::teams::{Team, TeamMember};

// ── DTOs ──

#[derive(Debug, Deserialize)]
pub struct CreateTeamRequest {
    pub name: String,
    pub display_name: String,
    pub collaboration_model: String,
    pub leader_agent_name: Option<String>,
    #[serde(default)]
    pub members: Vec<CreateTeamMemberRequest>,
}

#[derive(Debug, Deserialize)]
pub struct CreateTeamMemberRequest {
    pub member_name: String,
    pub member_type: String,
    pub member_id: String,
    pub role: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTeamRequest {
    pub display_name: Option<String>,
    pub collaboration_model: Option<String>,
    pub leader_agent_name: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
pub struct AddMemberRequest {
    pub member_name: String,
    pub member_type: String,
    pub member_id: String,
    pub role: String,
}

#[derive(Debug, Serialize)]
pub struct TeamResponse {
    pub team: Team,
    pub members: Vec<TeamMember>,
}

// ── Handlers ──

pub async fn handle_create_team(
    State(state): State<AppState>,
    Json(req): Json<CreateTeamRequest>,
) -> ApiResult<TeamResponse> {
    // Validate collab model
    if !matches!(req.collaboration_model.as_str(), "leader_operators" | "swarm") {
        return Err(api_err("collaboration_model must be 'leader_operators' or 'swarm'"));
    }

    // Create team record
    let team_id = state
        .store
        .create_team(
            &req.name,
            &req.display_name,
            &req.collaboration_model,
            req.leader_agent_name.as_deref(),
        )
        .map_err(|e| api_err(e.to_string()))?;

    // Create team channel
    state
        .store
        .create_channel(&req.name, None, ChannelType::Team)
        .map_err(|e| api_err(e.to_string()))?;

    // Add initial members + auto-join channel
    for m in &req.members {
        state
            .store
            .add_team_member(&team_id, &m.member_name, &m.member_type, &m.member_id, &m.role)
            .map_err(|e| api_err(e.to_string()))?;

        let sender_type = if m.member_type == "agent" {
            crate::store::messages::SenderType::Agent
        } else {
            crate::store::messages::SenderType::Human
        };
        let _ = state.store.join_channel(&req.name, &m.member_name, sender_type);

        // Restart active agent members to rebuild system prompt
        if m.member_type == "agent" {
            let _ = state.lifecycle.stop_agent(&m.member_name).await;
            let _ = state.lifecycle.start_agent(&m.member_name, None).await;
        }
    }

    // Initialize team workspace on disk
    let agent_members: Vec<&str> = req.members.iter()
        .filter(|m| m.member_type == "agent")
        .map(|m| m.member_name.as_str())
        .collect();
    let teams_dir = state.store.teams_dir();
    let agents_dir = state.store.agents_dir();
    let tw = crate::agent::workspace::TeamWorkspace::new(teams_dir);
    let aw = crate::agent::workspace::AgentWorkspace::new(&agents_dir);
    let _ = tw.init_team(&req.name, &agent_members);
    for m in req.members.iter().filter(|m| m.member_type == "agent") {
        let _ = aw.init_team_memory(&m.member_name, &req.name, &m.role);
    }

    let team = state.store.get_team(&req.name).map_err(|e| api_err(e.to_string()))?.unwrap();
    let members = state.store.get_team_members(&team_id).map_err(|e| api_err(e.to_string()))?;
    Ok(Json(TeamResponse { team, members }))
}

pub async fn handle_list_teams(State(state): State<AppState>) -> ApiResult<Vec<Team>> {
    let teams = state.store.list_teams().map_err(|e| internal_err(e.to_string()))?;
    Ok(Json(teams))
}

pub async fn handle_get_team(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<TeamResponse> {
    let team = state
        .store
        .get_team(&name)
        .map_err(|e| internal_err(e.to_string()))?
        .ok_or_else(|| api_err(format!("team not found: {name}")))?;
    let members = state.store.get_team_members(&team.id).map_err(|e| internal_err(e.to_string()))?;
    Ok(Json(TeamResponse { team, members }))
}

pub async fn handle_update_team(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<UpdateTeamRequest>,
) -> ApiResult<Team> {
    let team = state
        .store
        .get_team(&name)
        .map_err(|e| internal_err(e.to_string()))?
        .ok_or_else(|| api_err(format!("team not found: {name}")))?;

    let display_name = req.display_name.unwrap_or_else(|| team.display_name.clone());
    let collaboration_model = req.collaboration_model.unwrap_or_else(|| team.collaboration_model.clone());
    let leader_agent_name = req.leader_agent_name
        .map(|v| v)
        .unwrap_or(team.leader_agent_name.clone());

    state
        .store
        .update_team(&team.id, &display_name, &collaboration_model, leader_agent_name.as_deref())
        .map_err(|e| api_err(e.to_string()))?;

    let updated = state.store.get_team(&name).map_err(|e| internal_err(e.to_string()))?.unwrap();
    Ok(Json(updated))
}

pub async fn handle_delete_team(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<serde_json::Value> {
    let team = state
        .store
        .get_team(&name)
        .map_err(|e| internal_err(e.to_string()))?
        .ok_or_else(|| api_err(format!("team not found: {name}")))?;

    // Collect agent members before deletion
    let members = state.store.get_team_members(&team.id).map_err(|e| internal_err(e.to_string()))?;
    let agent_members: Vec<String> = members
        .iter()
        .filter(|m| m.member_type == "agent")
        .map(|m| m.member_name.clone())
        .collect();

    // Delete team (cascades to team_members, signals, quorum)
    state.store.delete_team(&team.id).map_err(|e| internal_err(e.to_string()))?;

    // Archive the team channel
    if let Ok(Some(ch)) = state.store.find_channel_by_name(&name) {
        let _ = state.store.archive_channel(&ch.id);
    }

    // Tear down workspace
    let tw = crate::agent::workspace::TeamWorkspace::new(state.store.teams_dir());
    let aw = crate::agent::workspace::AgentWorkspace::new(&state.store.agents_dir());
    let _ = tw.delete_team(&name);
    for agent_name in &agent_members {
        let _ = aw.delete_team_memory(agent_name, &name);
    }

    // Restart former agent members to rebuild their system prompts
    for agent_name in &agent_members {
        let _ = state.lifecycle.stop_agent(agent_name).await;
        let _ = state.lifecycle.start_agent(agent_name, None).await;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_add_team_member(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(req): Json<AddMemberRequest>,
) -> ApiResult<serde_json::Value> {
    let team = state
        .store
        .get_team(&name)
        .map_err(|e| internal_err(e.to_string()))?
        .ok_or_else(|| api_err(format!("team not found: {name}")))?;

    state
        .store
        .add_team_member(&team.id, &req.member_name, &req.member_type, &req.member_id, &req.role)
        .map_err(|e| api_err(e.to_string()))?;

    let sender_type = if req.member_type == "agent" {
        crate::store::messages::SenderType::Agent
    } else {
        crate::store::messages::SenderType::Human
    };
    let _ = state.store.join_channel(&name, &req.member_name, sender_type);

    // Init workspace dirs for the new agent member
    if req.member_type == "agent" {
        let tw = crate::agent::workspace::TeamWorkspace::new(state.store.teams_dir());
        let aw = crate::agent::workspace::AgentWorkspace::new(&state.store.agents_dir());
        let _ = tw.init_member(&name, &req.member_name);
        let _ = aw.init_team_memory(&req.member_name, &name, &req.role);
        let _ = state.lifecycle.stop_agent(&req.member_name).await;
        let _ = state.lifecycle.start_agent(&req.member_name, None).await;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn handle_remove_team_member(
    State(state): State<AppState>,
    Path((name, member_name)): Path<(String, String)>,
) -> ApiResult<serde_json::Value> {
    let team = state
        .store
        .get_team(&name)
        .map_err(|e| internal_err(e.to_string()))?
        .ok_or_else(|| api_err(format!("team not found: {name}")))?;

    state
        .store
        .remove_team_member(&team.id, &member_name)
        .map_err(|e| api_err(e.to_string()))?;

    // Remove from channel membership
    let _ = state.store.leave_channel(&name, &member_name);

    // Rebuild system prompt for the removed agent
    let _ = state.lifecycle.stop_agent(&member_name).await;
    let _ = state.lifecycle.start_agent(&member_name, None).await;

    Ok(Json(serde_json::json!({ "ok": true })))
}
```

- [ ] **Step 3: Register module in `src/server/handlers/mod.rs`**

Add:
```rust
pub mod teams;
pub use teams::*;
```

- [ ] **Step 4: Register routes in `src/server/mod.rs`**

In `build_router_with_lifecycle`, add team routes (follow the existing `.route(...)` pattern):

```rust
.route("/api/teams", get(handle_list_teams).post(handle_create_team))
.route("/api/teams/:name", get(handle_get_team).patch(handle_update_team).delete(handle_delete_team))
.route("/api/teams/:name/members", post(handle_add_team_member))
.route("/api/teams/:name/members/:member", axum::routing::delete(handle_remove_team_member))
```

- [ ] **Step 5: Update `MockLifecycle` in `tests/server_tests.rs` if needed**

`stop_agent` is already defined on `AgentLifecycle`. Verify `MockLifecycle` implements it. If not, add the stub following the pattern of other `MockLifecycle` methods.

- [ ] **Step 6: Run server tests**

Run: `cargo test test_create_team_endpoint test_list_teams_endpoint 2>&1 | tail -10`
Expected: both pass

- [ ] **Step 7: Run full test suite**

Run: `cargo test 2>&1 | tail -10`
Expected: all pass

- [ ] **Step 8: Commit**

```bash
git add src/server/handlers/teams.rs src/server/handlers/mod.rs src/server/mod.rs tests/server_tests.rs
git commit -m "feat(server): add team REST handlers and routes"
```

---

## Task 7: @mention routing + Swarm consensus detection

**Files:**
- Modify: `src/server/handlers/messages.rs`
- Test: `tests/server_tests.rs`

- [ ] **Step 1: Read the full `messages.rs` handler to understand the send flow**

Run: `cat src/server/handlers/messages.rs`

- [ ] **Step 2: Write a failing test for @mention forwarding**

Add to `tests/server_tests.rs`:

```rust
#[tokio::test]
async fn test_at_mention_forwards_to_team_channel() {
    let (store, app, _lifecycle) = setup_with_lifecycle();

    // Set up team and its channel
    let team_id = store.create_team("eng-team", "Eng", "leader_operators", None).unwrap();
    store.create_channel("eng-team", None, crate::store::channels::ChannelType::Team).unwrap();
    store.ensure_builtin_channels("testuser").unwrap();

    // Post a message mentioning @eng-team in #all
    let body = serde_json::json!({ "target": "all", "content": "hey @eng-team build something" });
    let resp = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/channels/all/messages")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Check team channel has the forwarded message
    let ch = store.find_channel_by_name("eng-team").unwrap().unwrap();
    let history = store.get_message_history(&ch.id, 10, None, None).unwrap();
    assert_eq!(history.len(), 1);
    assert!(history[0].forwarded_from.is_some());
}
```

Run: `cargo test test_at_mention_forwards_to_team_channel 2>&1 | tail -5`
Expected: FAIL (no forwarding logic yet)

- [ ] **Step 3: Add @mention scan + forwarding to the send message handler**

In `src/server/handlers/messages.rs`, find the handler that persists a message (the UI send path). After the message is saved, add:

```rust
// @mention forwarding to team channels
let mention_re = regex::Regex::new(r"@([A-Za-z0-9_-]+)").unwrap();
for cap in mention_re.captures_iter(&req.content) {
    let mention = &cap[1];
    if let Ok(Some(team)) = state.store.get_team(mention) {
        if let Ok(Some(team_ch)) = state.store.find_channel_by_name(&team.name) {
            // Write a forwarded copy into the team channel
            let forwarded_from = crate::store::messages::ForwardedFrom {
                channel_name: channel_name.clone(),
                sender_name: sender_name.clone(),
            };
            if let Ok(fwd_msg_id) = state.store.post_message_with_forwarded_from(
                &team_ch.id,
                &sender_name,
                sender_type,
                &req.content,
                &[],
                Some(forwarded_from),
            ) {
                let collab = crate::agent::collaboration::make_collaboration_model(&team.collaboration_model);

                // For Swarm: snapshot quorum + post deliberation prompt
                if let Some(prompt) = collab.deliberation_prompt() {
                    let _ = state.store.snapshot_swarm_quorum(&team.id, &fwd_msg_id);
                    let _ = state.store.post_system_message(&team_ch.id, &prompt);
                }

                // Wake team member agents
                let members = state.store.get_team_members(&team.id).unwrap_or_default();
                for m in members.iter().filter(|m| m.member_type == "agent") {
                    let _ = state.lifecycle.notify_agent(&m.member_name).await;
                }
            }
        }
    }
}
```

You will need to add `post_message_with_forwarded_from` and `post_system_message` helpers to the store. Add them in `src/store/messages.rs` following the pattern of the existing message insert function. The key difference: `post_message_with_forwarded_from` accepts an `Option<ForwardedFrom>` and serializes it to JSON for the `forwarded_from` column.

Also add `regex = "1"` to `Cargo.toml` if not already present.

- [ ] **Step 4: Add Swarm consensus signal detection**

In the same send handler, after storing the message but before (or after) the mention forwarding block, add for messages sent by agents into team channels:

```rust
// Swarm consensus signal detection
if sender_type == SenderType::Agent {
    if let Ok(Some(ch)) = state.store.find_channel_by_name(&channel_name) {
        if ch.channel_type == ChannelType::Team {
            if let Ok(Some(team)) = state.store.get_team(&channel_name) {
                let collab = crate::agent::collaboration::make_collaboration_model(&team.collaboration_model);
                if collab.is_consensus_signal(&req.content) {
                    match state.store.record_swarm_signal(&team.id, &sender_name, &req.content) {
                        Ok(true) => {
                            // All members ready — post the GO message
                            let _ = state.store.post_system_message(
                                &ch.id,
                                "[System] All members ready — execution begins.",
                            );
                        }
                        Ok(false) => {}
                        Err(e) => tracing::warn!("swarm signal error: {e}"),
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 5: Run the forwarding test**

Run: `cargo test test_at_mention_forwards_to_team_channel 2>&1 | tail -10`
Expected: PASS

- [ ] **Step 6: Run full test suite**

Run: `cargo test 2>&1 | tail -10`
Expected: all pass

- [ ] **Step 7: Commit**

```bash
git add src/server/handlers/messages.rs src/store/messages.rs Cargo.toml tests/server_tests.rs
git commit -m "feat(messages): add @mention team forwarding and swarm consensus detection"
```

---

## Task 8: UI — types, API, and store state

**Files:**
- Modify: `ui/src/types.ts`
- Modify: `ui/src/api.ts`
- Modify: `ui/src/store.tsx`

- [ ] **Step 1: Add team types to `ui/src/types.ts`**

Append:

```typescript
export interface Team {
  id: string
  name: string
  display_name: string
  collaboration_model: 'leader_operators' | 'swarm'
  leader_agent_name: string | null
  created_at: string
}

export interface TeamMember {
  team_id: string
  member_name: string
  member_type: 'agent' | 'human'
  member_id: string
  role: string
  joined_at: string
}

export interface TeamResponse {
  team: Team
  members: TeamMember[]
}
```

Also ensure `ChannelInfo` in `types.ts` has `channel_type?: string` (or update to `'channel' | 'dm' | 'system' | 'team'`).

- [ ] **Step 2: Add team API functions to `ui/src/api.ts`**

```typescript
export async function listTeams(): Promise<Team[]> {
  return json(await fetch(`${BASE}/api/teams`))
}

export async function createTeam(payload: {
  name: string
  display_name: string
  collaboration_model: string
  leader_agent_name: string | null
  members: Array<{ member_name: string; member_type: string; member_id: string; role: string }>
}): Promise<TeamResponse> {
  return json(await fetch(`${BASE}/api/teams`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  }))
}

export async function updateTeam(name: string, payload: {
  display_name?: string
  collaboration_model?: string
  leader_agent_name?: string | null
}): Promise<Team> {
  return json(await fetch(`${BASE}/api/teams/${name}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  }))
}

export async function deleteTeam(name: string): Promise<void> {
  await fetch(`${BASE}/api/teams/${name}`, { method: 'DELETE' })
}

export async function addTeamMember(teamName: string, member: {
  member_name: string; member_type: string; member_id: string; role: string
}): Promise<void> {
  await fetch(`${BASE}/api/teams/${teamName}/members`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(member),
  })
}

export async function removeTeamMember(teamName: string, memberName: string): Promise<void> {
  await fetch(`${BASE}/api/teams/${teamName}/members/${memberName}`, { method: 'DELETE' })
}
```

- [ ] **Step 3: Add team state to `ui/src/store.tsx`**

Read current store to understand pattern. Add a `teams` array and `refreshTeams` action. Wire `refreshTeams` into the existing `refreshServerInfo` call so teams are loaded alongside server info.

```typescript
// In the store state interface:
teams: Team[]
refreshTeams: () => Promise<void>

// In the store implementation:
teams: [],
refreshTeams: async () => {
  try {
    const teams = await listTeams()
    set({ teams })
  } catch (e) {
    console.error('Failed to load teams', e)
  }
},
```

Call `refreshTeams()` inside `refreshServerInfo` after the server info fetch.

- [ ] **Step 4: Build check**

Run: `cd ui && npm run build 2>&1 | tail -10`
Expected: no TypeScript errors

- [ ] **Step 5: Commit**

```bash
git add ui/src/types.ts ui/src/api.ts ui/src/store.tsx
git commit -m "feat(ui): add team types, API functions, and store state"
```

---

## Task 9: UI — Sidebar team badge + Create modal toggle

**Files:**
- Modify: `ui/src/components/Sidebar.tsx`
- Modify: `ui/src/components/Sidebar.css` (if needed)
- Modify: `ui/src/components/CreateChannelModal.tsx`

- [ ] **Step 1: Read `CreateChannelModal.tsx` fully before editing**

Run: `cat ui/src/components/CreateChannelModal.tsx`

- [ ] **Step 2: Add `+ New Team` shortcut to `Sidebar.tsx`**

In the Channels section header (where `+ New Channel` lives), add a second icon button labeled `+ New Team` (or a `Users` icon with tooltip "New Team"). On click, call `setShowCreateChannel(true)` with a `defaultMode='team'` prop so the modal opens with the Team tab pre-selected.

Update `CreateChannelModal` to accept an optional `defaultMode?: 'channel' | 'team'` prop and initialize `mode` state from it:

```tsx
const [mode, setMode] = useState<'channel' | 'team'>(props.defaultMode ?? 'channel')
```

- [ ] **Step 3: Add `[team]` badge to channel list in `Sidebar.tsx`**

Find the channel list rendering. For each channel where `ch.channel_type === 'team'`, render a `[team]` badge after the channel name — same style as the existing `[sys]` badge. Example:

```tsx
{ch.channel_type === 'system' && <span className="channel-badge sys">sys</span>}
{ch.channel_type === 'team' && <span className="channel-badge team">team</span>}
```

Add `.channel-badge.team` CSS rule matching the `.sys` badge style.

- [ ] **Step 3: Add Channel/Team toggle to `CreateChannelModal.tsx`**

`mode` state is already initialized in Step 2 — do **not** add another `useState` for it. The toggle is built on top of the existing `mode` state.

Render two buttons at the top of the modal: `Channel` and `Team` (wired to `setMode`). When `mode === 'team'`, show additional fields:
- `display_name` (text input)
- `collaboration_model` select: `leader_operators` | `swarm`
- `leader_agent_name` select (shown only when `collaboration_model === 'leader_operators'`) — populated from `serverInfo.agents`
- Members picker (multi-select from `serverInfo.agents` + `serverInfo.humans`, with role dropdown per member)

On submit when `mode === 'team'`, call `createTeam(...)` instead of the regular channel create. After success, call `refreshServerInfo()` and `refreshTeams()`.

- [ ] **Step 4: Build and verify no TypeScript errors**

Run: `cd ui && npm run build 2>&1 | tail -10`

- [ ] **Step 5: Commit**

```bash
git add ui/src/components/Sidebar.tsx ui/src/components/Sidebar.css ui/src/components/CreateChannelModal.tsx
git commit -m "feat(ui): add team badge to sidebar and create team modal toggle"
```

---

## Task 10: UI — Team settings panel

**Files:**
- Create: `ui/src/components/TeamSettings.tsx`
- Create: `ui/src/components/TeamSettings.css`
- Modify: `ui/src/components/ChatPanel.tsx` (or wherever channel header lives)

- [ ] **Step 1: Read `ChatPanel.tsx` to understand channel header structure**

Run: `cat ui/src/components/ChatPanel.tsx`

- [ ] **Step 2: Create `TeamSettings.tsx`**

Panel showing, for a team channel:
- Display name (editable text field)
- Collaboration model selector
- Leader agent selector (when `collaboration_model === 'leader_operators'`)
- Member list with roles (add button + remove button per member)
- Delete team button (with confirmation)

Wire each field to the corresponding API call (`updateTeam`, `addTeamMember`, `removeTeamMember`, `deleteTeam`). After mutations, call `refreshServerInfo()` and `refreshTeams()`.

```tsx
import { useState } from 'react'
import { updateTeam, deleteTeam, addTeamMember, removeTeamMember } from '../api'
import { useApp } from '../store'
import type { Team, TeamMember } from '../types'
import './TeamSettings.css'

interface Props {
  team: Team
  members: TeamMember[]
  onClose: () => void
}

export function TeamSettings({ team, members, onClose }: Props) {
  // ... form state, handlers, JSX
}
```

- [ ] **Step 3: Render `TeamSettings` from the channel header**

In `ChatPanel.tsx` (or equivalent), detect when the selected channel is a team channel (`channel_type === 'team'`). In the settings icon click handler, render `<TeamSettings>` instead of (or in addition to) the regular channel edit modal.

You will need to load the team data (`GET /api/teams/:name`) when the panel opens — add a `getTeam` function to `api.ts`:

```typescript
export async function getTeam(name: string): Promise<TeamResponse> {
  return json(await fetch(`${BASE}/api/teams/${name}`))
}
```

- [ ] **Step 4: Build check**

Run: `cd ui && npm run build 2>&1 | tail -10`
Expected: no errors

- [ ] **Step 5: Commit**

```bash
git add ui/src/components/TeamSettings.tsx ui/src/components/TeamSettings.css ui/src/components/ChatPanel.tsx ui/src/api.ts
git commit -m "feat(ui): add team settings panel"
```

---

## Task 11: UI — @mention autocomplete for teams

**Files:**
- Modify: `ui/src/components/MentionTextarea.tsx`

- [ ] **Step 1: Read `MentionTextarea.tsx` to understand current mention list**

Run: `cat ui/src/components/MentionTextarea.tsx`

- [ ] **Step 2: Add teams to the mention picker**

The mention picker currently shows agents (and possibly channels). Add teams from `useApp().teams`. Display team mentions with a group icon (e.g. `Users` from `lucide-react`) to distinguish them from agent names.

The `@` token for a team should insert `@<team-name>` (the slug), matching what the server scans for.

- [ ] **Step 3: Build check**

Run: `cd ui && npm run build 2>&1 | tail -10`

- [ ] **Step 4: Commit**

```bash
git add ui/src/components/MentionTextarea.tsx
git commit -m "feat(ui): include teams in @mention autocomplete"
```

---

## Task 12: End-to-end verification

- [ ] **Step 1: Run all Rust tests**

Run: `cargo test 2>&1 | tail -15`
Expected: all pass

- [ ] **Step 2: Run e2e tests**

Run: `cargo test --test e2e_tests 2>&1 | tail -10`
Expected: all pass

- [ ] **Step 3: Production UI build**

Run: `cd ui && npm run build 2>&1 | tail -5`
Expected: no errors

- [ ] **Step 4: Smoke test — start server and manually verify**

```bash
cargo build && ./target/debug/chorus serve
```

In a browser at `http://localhost:5173`:
- Create a team `eng-team` (Leader+Operators) via the `+ New Channel → Team` toggle
- Verify `#eng-team` appears in channels list with `[team]` badge
- Verify settings icon on `#eng-team` shows team management fields
- Post a message in `#general` containing `@eng-team` and verify it forwards to `#eng-team`
- Create a second team with Swarm model, post a task @mention, verify deliberation prompt appears

- [ ] **Step 5: Final commit (if anything fixed)**

```bash
git add -p  # stage only relevant files
git commit -m "fix(team): address smoke test findings"
```
