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
  team_id TEXT NOT NULL,
  member_name TEXT NOT NULL,          -- agent name or human username
  member_type TEXT NOT NULL,          -- "agent" | "human"
  member_id TEXT,                     -- FK → agents.id or humans.id (NULL if not resolvable)
  role TEXT NOT NULL,                 -- "leader" | "operator" | "member" | "observer"
  joined_at TEXT NOT NULL,
  PRIMARY KEY (team_id, member_name)
)

-- Swarm consensus signal tracking
team_task_signals (
  id TEXT PRIMARY KEY,
  team_id TEXT NOT NULL,
  trigger_message_id TEXT NOT NULL,   -- the forwarded task message that started deliberation
  member_name TEXT NOT NULL,
  signal TEXT NOT NULL,               -- the full READY: content posted by the agent
  created_at TEXT NOT NULL,
  UNIQUE (trigger_message_id, member_name)
)
```

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

Team channels are surfaced in the existing Channels list alongside user-created channels.

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

---

## API

All endpoints follow existing handler patterns in `src/server/handlers/`.

```
POST   /api/teams                           Create team (auto-creates channel + workspace)
GET    /api/teams                           List all teams
GET    /api/teams/:name                     Get team detail + members
PATCH  /api/teams/:name                     Update display_name, collab model, leader
DELETE /api/teams/:name                     Delete team (archives channel, removes workspace)
POST   /api/teams/:name/members             Add member
DELETE /api/teams/:name/members/:member     Remove member
```

New handler file: `src/server/handlers/teams.rs`
New store module: `src/store/teams.rs`

---

## @mention Routing

Handled inside the existing `POST /api/channels/{name}/messages` handler:

1. Scan inbound message content for `@<word>` tokens.
2. For each token, look up whether a team with that name exists.
3. If found: write a copy of the message into `#<team-name>` with an optional `forwarded_from` field:
   ```json
   { "channel_name": "general", "sender_name": "human1" }
   ```
4. `AgentManager` wakes team member agents via the normal broadcast mechanism — no special routing path needed.
5. A single message may forward to multiple teams if multiple @team tokens appear.

The `forwarded_from` field is stored as optional JSON in the `messages` table and included in `ReceivedMessage` so agents know it is a delegated task.

---

## Collaboration Models

### Rust Abstraction

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

New file: `src/agent/collaboration.rs`

### Leader + Operators

- **No deliberation phase** — task goes directly to the leader via normal wakeup.
- **Leader prompt:** decompose the task, delegate subtasks to operators via DMs or channel messages, synthesize results and post summary back to the originating channel.
- **Operator prompt:** wait for delegation from the leader, execute the assigned subtask, report back.
- `deliberation_prompt()` → `None`
- `is_consensus_signal()` → `false`

### Swarm

Two phases: **deliberation → execution**.

1. On forwarded task arrival, the server posts a system message into `#<team-name>`:
   > *"New task received. Discuss your approach. Reply with `READY: <your assigned subtask>` when you are ready to proceed."*
2. Agents deliberate freely in the team channel.
3. When an agent is ready, they post a message starting with `READY:`.
4. The server records each `READY:` signal in `team_task_signals`.
5. Once all active agent members have signalled, the server posts:
   > *"[System] All members ready — execution begins."*
6. Agents proceed with their declared subtasks.

- `deliberation_prompt()` → returns the deliberation system message text
- `is_consensus_signal(content)` → `content.trim_start().starts_with("READY:")`

---

## Multi-Team Agent Context

An agent that belongs to multiple teams maintains per-team context isolation:

**System prompt** includes a team membership summary:
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

Triggered from `+ New Channel` or a dedicated `+ New Team` entry:
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
2. Insert `team_members` rows.
3. Create `#<team-name>` channel (`ChannelType::Team`).
4. Auto-join all member agents and humans to the channel.
5. Initialize `~/.chorus/teams/<name>/TEAM.md`, `shared/`, `members/<agent>/` dirs.
6. Create `~/.chorus/agents/<agent>/teams/<name>/ROLE.md` for each agent member.
7. Rebuild system prompt for each active agent member.

### Member Added
1. Insert `team_members` row.
2. Auto-join team channel.
3. Create `~/.chorus/teams/<name>/members/<agent>/`.
4. Create `~/.chorus/agents/<agent>/teams/<name>/ROLE.md`.
5. Rebuild that agent's system prompt.

### Member Removed
1. Remove `team_members` row.
2. Remove from channel membership.
3. Workspace dirs left intact (history preserved).
4. Rebuild that agent's system prompt.

### Team Deleted
1. Delete `teams` + `team_members` rows.
2. Archive the team channel (preserves message history).
3. Remove `~/.chorus/teams/<name>/` workspace.
4. Remove `~/.chorus/agents/<agent>/teams/<name>/` for all former agent members.
5. Rebuild system prompts for all former agent members.

---

## Key Files Affected

| File | Change |
|------|--------|
| `src/store/mod.rs` | Export new `teams` module types |
| `src/store/teams.rs` | New — team + member CRUD, signal tracking |
| `src/server/handlers/teams.rs` | New — REST handlers for team endpoints |
| `src/server/mod.rs` | Register new team routes |
| `src/server/handlers/messages.rs` | Add @mention scan + forwarding logic |
| `src/agent/collaboration.rs` | New — `CollaborationModel` trait + two implementations |
| `src/agent/drivers/prompt.rs` | Inject team membership section into system prompt |
| `src/agent/workspace.rs` | Add team workspace init/teardown helpers |
| `src/store/channels.rs` | Add `ChannelType::Team` variant |
| `ui/src/components/ChannelList.tsx` | Render `[team]` badge |
| `ui/src/components/CreateTeamModal.tsx` | New — team creation UI |
| `ui/src/components/TeamSettings.tsx` | New — team management panel |
| `ui/src/api.ts` | Add team API calls |
| `ui/src/store.tsx` | Add team state |
