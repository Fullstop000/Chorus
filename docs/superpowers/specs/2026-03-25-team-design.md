# Team Feature Design

**Date:** 2026-03-25
**Status:** Approved
**Approach:** Agent-driven collaboration, server handles structure (Option B)

---

## Overview

A **Team** is a named group of agents (plus optional human observers) that collaborates on tasks under a configured collaboration model. Teams are first-class entities in Chorus — each has a dedicated channel, a shared workspace, and a pluggable coordination protocol.

Humans manage team lifecycle (create, modify, delete). Agents are passive members whose intelligence drives actual collaboration.

---

## Data Model

### New SQLite Tables

```sql
-- Core team record
teams (
  id TEXT PRIMARY KEY,                -- uuid
  name TEXT UNIQUE NOT NULL,          -- slug used for channel name and @mention, e.g. "eng-team"
  display_name TEXT NOT NULL,         -- human-readable name, e.g. "Engineering Team"
  collaboration_model TEXT NOT NULL,  -- "leader_operators" | "swarm"
  leader_agent_name TEXT,             -- NULL for swarm; agent name for leader+operators
  created_at TEXT NOT NULL
)

-- Team membership
team_members (
  team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  member_name TEXT NOT NULL,          -- agent name or human username
  member_type TEXT NOT NULL,          -- "agent" | "human"
  member_id TEXT NOT NULL,            -- agents.id or humans.name (always populated at insert)
  role TEXT NOT NULL,                 -- "leader" | "operator" | "member" | "observer"
  joined_at TEXT NOT NULL,
  PRIMARY KEY (team_id, member_name)
)

-- Swarm consensus signal tracking (one row per agent READY: signal per task)
team_task_signals (
  id TEXT PRIMARY KEY,
  team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  trigger_message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  member_name TEXT NOT NULL,
  signal TEXT NOT NULL,               -- the full READY: content posted by the agent
  created_at TEXT NOT NULL,
  UNIQUE (trigger_message_id, member_name)
)

-- Swarm quorum snapshot: agent members present when a task arrived (for consensus counting)
team_task_quorum (
  trigger_message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  member_name TEXT NOT NULL,
  resolved_at TEXT,                   -- NULL = open; ISO 8601 timestamp when consensus reached
  PRIMARY KEY (trigger_message_id, member_name)
)
```

`team_members`, `team_task_signals`, and `team_task_quorum` rows are cascade-deleted when the parent `teams` row is deleted.

These tables are created in the existing schema bootstrap in `src/store/mod.rs` (the `CREATE TABLE IF NOT EXISTS` block), consistent with how all other tables are initialised.

### messages table migration

Add one nullable column to the existing `messages` table. Since the codebase initialises schema via `CREATE TABLE IF NOT EXISTS`, this column is added as a conditional `ALTER TABLE` at store bootstrap time:

```sql
-- added once, guarded by checking PRAGMA table_info
ALTER TABLE messages ADD COLUMN forwarded_from TEXT;  -- NULL or JSON, added if not exists
```

The guard pattern: query `PRAGMA table_info(messages)` at startup; if the `forwarded_from` column is absent, execute the `ALTER TABLE`. This is the same pattern used for other additive migrations in the codebase.

JSON shape when set:
```json
{ "channel_name": "general", "sender_name": "human1" }
```

Add `forwarded_from: Option<ForwardedFrom>` to both `Message` and `ReceivedMessage` structs in `src/store/messages.rs`. `ForwardedFrom` is a new `#[derive(Debug, Clone, Serialize, Deserialize)]` struct with `channel_name: String` and `sender_name: String`.

### Channel Type Extension

`ChannelType` gains a new variant:

```rust
pub enum ChannelType {
    Channel,
    Dm,
    System,
    Team,   // auto-created when a team is created; badge shown in UI
}
```

**Required store changes for `ChannelType::Team` in `src/store/channels.rs`:**

- `create_channel` match arm: add `ChannelType::Team => "team"`.
- `list_channels`: update query to `WHERE channel_type IN ('channel', 'team') AND archived = 0` so team channels appear alongside user-created channels.
- `archive_channel`: relax guard to allow `ChannelType::Channel | ChannelType::Team`.
- `delete_channel`: team channels are never deleted via this path — they are archived on team deletion. The guard should explicitly reject `ChannelType::Team` with a `"team channels cannot be deleted directly; delete the team instead"` error.
- `update_channel`: relax guard to allow `ChannelType::Channel | ChannelType::Team`.
- `list_auto_join_channels`: no change — team channels are NOT auto-joined by newly created agents. Agents join a team channel only when explicitly added as team members.

### Workspace Layout

```
~/.chorus/
  teams/
    eng-team/
      TEAM.md             ← generated stub: purpose, collab model, member roles
      shared/             ← shared artifacts, plans, outputs
      members/
        alice/            ← alice's team-scoped shared notes (visible to team)
        bob/
  agents/
    alice/
      MEMORY.md
      teams/
        eng-team/         ← alice's private team memory (role, history, perspective)
          ROLE.md         ← generated stub: alice's role + responsibilities in eng-team
        algo-team/
          ROLE.md
```

Each agent can be a member of multiple teams. Team memory is isolated per team in both the shared workspace and the agent's private workspace.

A new `TeamWorkspace` struct in `src/agent/workspace.rs` manages team-scoped paths, analogous to `AgentWorkspace`. It owns its base path (consistent with how `AgentWorkspace` is structured):

```rust
pub struct TeamWorkspace {
    teams_dir: PathBuf,  // e.g. ~/.chorus/teams/
}

impl TeamWorkspace {
    pub fn new(teams_dir: PathBuf) -> Self;
    pub fn team_path(&self, team_name: &str) -> PathBuf;
    pub fn member_path(&self, team_name: &str, agent_name: &str) -> PathBuf;
    pub fn init_team(&self, team_name: &str, members: &[&str]) -> std::io::Result<()>;
    pub fn init_member(&self, team_name: &str, agent_name: &str) -> std::io::Result<()>;
    pub fn delete_team(&self, team_name: &str) -> std::io::Result<()>;
}
```

The agent's private team memory path (`~/.chorus/agents/<name>/teams/<team>/`) is managed through the existing `AgentWorkspace` struct via a new helper method `team_memory_path(team_name: &str) -> PathBuf`.

---

## API

All endpoints follow existing handler patterns in `src/server/handlers/`.

```
POST   /api/teams                           Create team (auto-creates channel + workspace)
GET    /api/teams                           List all teams
GET    /api/teams/:name                     Get team detail + members
PATCH  /api/teams/:name                     Update team metadata (see mutable fields below)
DELETE /api/teams/:name                     Delete team (archives channel, removes workspace)
POST   /api/teams/:name/members             Add member
DELETE /api/teams/:name/members/:member     Remove member
```

**PATCH `/api/teams/:name` request body** (all fields optional):
```json
{
  "display_name": "string",
  "collaboration_model": "leader_operators" | "swarm",
  "leader_agent_name": "string | null"
}
```
The `name` slug is immutable after creation (changing it would break the team channel name and workspace path). `display_name`, `collaboration_model`, and `leader_agent_name` are the only mutable fields.

New handler file: `src/server/handlers/teams.rs`
New store module: `src/store/teams.rs`

---

## @mention Routing

Handled inside the existing `POST /api/channels/{name}/messages` handler:

1. Scan inbound message content for `@<word>` tokens.
2. For each token, look up whether a team with that name exists.
3. If found: write a copy of the message into `#<team-name>` with `forwarded_from` set to `{ "channel_name": <origin>, "sender_name": <sender> }`. The copied message's `id` becomes the `trigger_message_id` for deliberation tracking.
4. For Swarm teams: snapshot the current agent member list into `team_task_quorum` (one row per agent member at this moment, keyed by `trigger_message_id`), then post the deliberation system message.
5. `AgentManager` wakes team member agents via the normal broadcast mechanism — no special routing path needed.
6. A single message may forward to multiple teams if multiple @team tokens appear.

---

## Collaboration Models

### Rust Abstraction

The `CollaborationModel` trait lives in `src/agent/collaboration.rs`. The server obtains the correct implementation via a free factory function:

```rust
/// Returns a boxed CollaborationModel implementation for the given model string.
pub fn make_collaboration_model(model: &str) -> Box<dyn CollaborationModel> { ... }
```

This factory is called in two server-side locations:
1. The @mention routing handler — to decide whether to post a deliberation prompt (`deliberation_prompt()`).
2. The incoming-message handler for team channels — to check whether an agent message is a consensus signal (`is_consensus_signal()`).

```rust
/// Defines how a team coordinates when a task arrives in the team channel.
pub trait CollaborationModel: Send + Sync {
    /// Role-specific instructions injected into each member's system prompt.
    fn member_role_prompt(&self, role: &TeamRole) -> String;

    /// Optional deliberation instructions posted as a system message when a
    /// forwarded task arrives. None = skip deliberation phase.
    fn deliberation_prompt(&self) -> Option<String>;

    /// Returns true if this agent message counts as a consensus signal.
    fn is_consensus_signal(&self, content: &str) -> bool;
}
```

### Leader + Operators

- **No deliberation phase** — task goes directly to the leader via normal wakeup.
- **Leader prompt:** decompose the task, delegate subtasks to operators via DMs or channel messages, synthesize results and post summary back to the originating channel.
- **Operator prompt:** wait for delegation from the leader, execute the assigned subtask, report back.
- `deliberation_prompt()` → `None`
- `is_consensus_signal()` → `false`

### Swarm

Two phases: **deliberation → execution**.

1. On forwarded task arrival, the server snapshots the quorum and posts a system message into `#<team-name>`:
   > *"New task received. Discuss your approach. Reply with `READY: <your assigned subtask>` when you are ready to proceed."*
2. Agents deliberate freely in the team channel.
3. When an agent is ready, they post a message starting with `READY:`.
4. On each incoming agent message in the team channel, the server calls `is_consensus_signal()`. If true, it looks up open quorum entries for this team (`team_task_quorum` rows where `team_id` matches and `resolved_at IS NULL`). It inserts the signal against the **earliest unresolved** `trigger_message_id` for this team (ordered by the trigger message's `created_at`). A `READY:` signal for which no open quorum entry exists is silently discarded.
5. Consensus is reached when the set of `member_name` values in `team_task_signals` for a given `trigger_message_id` equals the full quorum snapshot in `team_task_quorum` for that same `trigger_message_id`. Agents who join after the task arrived are not in the quorum and are not counted.
6. On consensus, the server updates all `team_task_quorum` rows for that `trigger_message_id` to set `resolved_at = now()`, then posts:
   > *"[System] All members ready — execution begins."*
7. Agents proceed with their declared subtasks.

- `deliberation_prompt()` → returns the deliberation system message text
- `is_consensus_signal(content)` → `content.trim_start().starts_with("READY:")`

---

## System Prompt Integration

Team membership is injected into each agent's system prompt via a new `teams` field in `PromptOptions`:

```rust
pub struct PromptOptions {
    pub tool_prefix: String,
    pub extra_critical_rules: Vec<String>,
    pub post_startup_notes: Vec<String>,
    pub include_stdin_notification_section: bool,
    pub teams: Vec<TeamMembership>,  // new field
}

pub struct TeamMembership {
    pub team_name: String,
    pub role: String,
}
```

`build_base_system_prompt` renders the team list as a `## Your Teams` section when `opts.teams` is non-empty. The caller (agent spawn path in `src/agent/manager.rs`) populates `teams` by querying `store.list_teams_for_agent(agent_name)` before building the prompt.

**Rebuilding the system prompt** is done by stopping and restarting the agent process (which rebuilds the prompt at spawn time from fresh store data). This reuses the existing agent restart mechanism in `AgentLifecycle`. No new `AgentLifecycle` method is required — the lifecycle management call sites (member add/remove, team create/delete) call the existing stop + start sequence for each affected active agent.

---

## Multi-Team Agent Context

An agent that belongs to multiple teams maintains per-team context isolation:

**System prompt** includes a team membership summary rebuilt whenever the agent's team membership changes:
```
## Your Teams
- #eng-team  — role: leader
- #algo-team — role: operator
```

**On-demand context loading** — when the agent receives a message from `#eng-team`, they read:
```
~/.chorus/agents/alice/teams/eng-team/ROLE.md
```
This keeps context lean; the agent only loads the relevant team memory for the active task.

---

## UI

### Channel List

Team channels appear in the existing Channels list with a `[team]` badge (same visual pattern as the `[sys]` badge on system channels):

```
CHANNELS
  # all          [sys]
  # general
  # eng-team     [team]
  # algo-team    [team]
  # random
  + New Channel
```

No separate sidebar section for teams.

### Team Management

Accessed via the channel header settings icon on a team channel. Shows additional fields beyond normal channel settings:
- Collaboration model selector (Leader+Operators / Swarm)
- Member list with roles
- Leader designation (Leader+Operators model only)
- Delete team button

### Create Team Modal

`+ New Channel` opens a shared creation modal with a **Channel / Team toggle** at the top. Selecting "Team" switches the form to the team creation fields. A dedicated `+ New Team` shortcut (in the channel list header, next to `+ New Channel`) opens the same modal with the Team tab pre-selected. Both paths are required at launch and share the same modal component:
- Name (slug), display name
- Collaboration model
- Initial members + role assignment
- Leader selector (Leader+Operators only)

### Agent Profile

Adds a "Teams" section listing the teams an agent belongs to and their role in each.

### @mention Autocomplete

Team names are included in the existing mention picker, distinguished with a group icon.

---

## Lifecycle

### Team Created
1. Insert `teams` row.
2. Insert `team_members` rows (with `member_id` always populated).
3. Create `#<team-name>` channel (`ChannelType::Team`).
4. Auto-join all member agents and humans to the channel.
5. Initialize `~/.chorus/teams/<name>/TEAM.md`, `shared/`, `members/<agent>/` dirs via `TeamWorkspace::init_team`.
6. Create `~/.chorus/agents/<agent>/teams/<name>/ROLE.md` for each agent member via `AgentWorkspace::team_memory_path`.
7. Stop + restart each active agent member so their system prompt is rebuilt with the new team membership.

### Member Added
1. Insert `team_members` row (with `member_id` populated).
2. Auto-join team channel.
3. Create `~/.chorus/teams/<name>/members/<agent>/` via `TeamWorkspace::init_member`.
4. Create `~/.chorus/agents/<agent>/teams/<name>/ROLE.md`.
5. Stop + restart that agent so their system prompt is rebuilt.

### Member Removed
1. Remove `team_members` row.
2. Remove from channel membership.
3. Workspace dirs left intact (history preserved).
4. Stop + restart that agent so their system prompt is rebuilt.

### Team Deleted
1. Collect all current agent members before deletion.
2. Delete `teams` row (cascades to `team_members`, `team_task_signals`, `team_task_quorum`).
3. Archive the team channel via `archive_channel` (preserves message history).
4. Remove `~/.chorus/teams/<name>/` workspace via `TeamWorkspace::delete_team`.
5. Remove `~/.chorus/agents/<agent>/teams/<name>/` for all former agent members.
6. Stop + restart each formerly active agent member so their system prompt is rebuilt.

---

## Key Files Affected

| File | Change |
|------|--------|
| `src/store/mod.rs` | Add `teams` module; add schema for new tables; add `forwarded_from` migration guard |
| `src/store/teams.rs` | New — team + member CRUD, signal + quorum tracking, `list_teams_for_agent` |
| `src/store/channels.rs` | Add `ChannelType::Team`; update `list_channels`, `archive_channel`, `update_channel` guards; harden `delete_channel` to reject team channels |
| `src/store/messages.rs` | Add `ForwardedFrom` struct; add `forwarded_from` field to `Message` + `ReceivedMessage` |
| `src/server/handlers/teams.rs` | New — REST handlers for team endpoints |
| `src/server/mod.rs` | Register new team routes |
| `src/server/handlers/messages.rs` | Add @mention scan, forwarding, quorum snapshot, consensus signal detection |
| `src/agent/collaboration.rs` | New — `CollaborationModel` trait + `LeaderOperators` + `Swarm` + `make_collaboration_model` factory |
| `src/agent/drivers/prompt.rs` | Add `teams: Vec<TeamMembership>` to `PromptOptions`; render `## Your Teams` section |
| `src/agent/workspace.rs` | Add `TeamWorkspace` struct; add `team_memory_path` to `AgentWorkspace` |
| `src/agent/manager.rs` | Populate `PromptOptions::teams` from store at agent spawn; implement stop+restart for prompt rebuilds |
| `ui/src/components/ChannelList.tsx` | Render `[team]` badge for `ChannelType::Team` channels |
| `ui/src/components/CreateTeamModal.tsx` | New — team creation UI (accessible from both `+ New Channel` and `+ New Team`) |
| `ui/src/components/TeamSettings.tsx` | New — team management panel in channel header |
| `ui/src/api.ts` | Add team API calls |
| `ui/src/store.tsx` | Add team state |
