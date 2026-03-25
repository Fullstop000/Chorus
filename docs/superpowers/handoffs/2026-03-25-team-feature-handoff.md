# Team Feature — Implementation Handoff

**Date:** 2026-03-25
**Branch:** `claude/team-concept`
**Spec:** `docs/superpowers/specs/2026-03-25-team-design.md`
**Plan:** `docs/superpowers/plans/2026-03-25-team-feature.md`
**QA Cases:** `qa/cases/teams.md` (TMT-001 through TMT-008)

---

## What Was Built

A **Team** feature for Chorus — a named group of agents + optional humans that collaborates on tasks via a shared channel, shared workspace on disk, and a pluggable collaboration model (Leader+Operators or Swarm).

Key design: Server owns structure (team registry, channel, workspace, @mention forwarding). Agents drive collaboration via prompts.

---

## Current Branch State

All work is on `claude/team-concept`. Recent commits (newest first):

```
0d19d97 feat(prompt): inject team membership into agent system prompt
a515f21 docs(agent): add doc comments to LeaderOperators and Swarm structs
370acfe feat(agent): add CollaborationModel trait with LeaderOperators and Swarm
7fc2005 feat(workspace): add TeamWorkspace and agent team memory helpers
57abe35 fix(store): guard record_swarm_signal against non-quorum agents
7f438f8 feat(store): add teams module with CRUD and swarm signal tracking
eec5f8a fix(store): add teams module stub
26747ab fix(store): select and parse forwarded_from in get_messages_for_agent
71d2a4e feat(store): add team tables, ChannelType::Team, ForwardedFrom
a900111 qa(team): add TMT-001–008 case module and register in catalog
3b9f73c docs(team): add implementation plan
2a462f0 docs(team): fix spec issues from review
2fd19d1 docs(team): add team feature design spec
```

All 25 Rust tests pass (`cargo test`). `cargo build` is clean.

---

## Tasks Completed (Tasks 1–5)

### Task 1 ✅ — Schema: new tables + ChannelType::Team + ForwardedFrom

**Files changed:**
- `src/store/mod.rs` — added 4 new tables to `init_schema`, `forwarded_from` migration guard, `pub mod teams;`, `pub use teams::...`, `teams_dir()`, `conn_for_test()`
- `src/store/channels.rs` — added `ChannelType::Team` variant; updated `create_channel`, `list_channels` (includes team channels), `archive_channel`, `update_channel`, `delete_channel` guards
- `src/store/messages.rs` — added `ForwardedFrom { channel_name, sender_name }` struct; added `forwarded_from: Option<ForwardedFrom>` to `Message` and `ReceivedMessage`; `get_messages_for_agent` now selects and parses `m.forwarded_from`
- `src/store/teams.rs` — created (was stub, filled in Task 2)
- `tests/store_tests.rs` — added `test_team_tables_exist`

**Key detail:** `forwarded_from` JSON is parsed with `serde_json::from_str` in `get_messages_for_agent`; parse errors produce `None` (silently dropped — acceptable since the codebase always writes valid JSON here).

### Task 2 ✅ — Store: teams module CRUD + signal/quorum

**File changed:** `src/store/teams.rs` (full implementation)

**Methods added to `Store`:**
- `create_team`, `get_team`, `get_team_by_id`, `list_teams`, `update_team`, `delete_team`
- `add_team_member`, `remove_team_member`, `get_team_members`, `list_teams_for_agent`
- `leave_channel(channel_name, member_name)` — removes from `channel_members`
- `snapshot_swarm_quorum(team_id, trigger_message_id)` — idempotent INSERT into `team_task_quorum`
- `record_swarm_signal(team_id, member_name, signal) -> Result<bool>` — **guarded**: uses `EXISTS` subquery to verify `member_name` is in quorum before inserting; non-quorum agents return `Ok(false)` immediately; returns `Ok(true)` when quorum complete + sets `resolved_at`

**Tests added:** `test_create_and_get_team`, `test_add_and_list_team_members`, `test_list_teams_for_agent`, `test_delete_team_cascades`, `test_record_swarm_signal_ignores_non_quorum_agent`

### Task 3 ✅ — Workspace: TeamWorkspace + AgentWorkspace helpers

**File changed:** `src/agent/workspace.rs`

**Added `TeamWorkspace` struct** (owns `PathBuf teams_dir`):
- `new(teams_dir: PathBuf) -> Self`
- `team_path(team_name) -> PathBuf`
- `member_path(team_name, agent_name) -> PathBuf`
- `init_team(team_name, members: &[&str])` — creates `shared/`, per-member dirs, idempotent TEAM.md stub
- `init_member(team_name, agent_name)` — creates member dir
- `delete_team(team_name)` — removes dir tree if exists

**Added to existing `AgentWorkspace` impl:**
- `team_memory_path(agent_name, team_name) -> PathBuf` → `agents_dir/agent/teams/team`
- `init_team_memory(agent_name, team_name, role)` — creates dir + idempotent ROLE.md stub
- `delete_team_memory(agent_name, team_name)` — removes dir tree if exists

### Task 4 ✅ — Collaboration models trait

**File created:** `src/agent/collaboration.rs`

```rust
pub trait CollaborationModel: Send + Sync {
    fn member_role_prompt(&self, role: &str) -> String;
    fn deliberation_prompt(&self) -> Option<String>;
    fn is_consensus_signal(&self, content: &str) -> bool;
}
pub fn make_collaboration_model(model: &str) -> Box<dyn CollaborationModel>
// "swarm" → Swarm, anything else → LeaderOperators
```

- `LeaderOperators` — `deliberation_prompt` → `None`, `is_consensus_signal` → `false`
- `Swarm` — `deliberation_prompt` → `Some(...)`, `is_consensus_signal` → `content.trim_start().starts_with("READY:")`
- `build_teams_prompt_section(memberships: &[TeamMembership]) -> String` — renders `## Your Teams\n- #name — role: role\n...` or `""` if empty

**File modified:** `src/agent/mod.rs` — added `pub mod collaboration;`

### Task 5 ✅ — System prompt: team membership injection

**Files changed:**
- `src/agent/drivers/prompt.rs` — added `teams: Vec<TeamMembership>` to `PromptOptions`; `build_base_system_prompt` now appends `build_teams_prompt_section(&opts.teams)` at the end of the prompt
- `src/agent/manager.rs` — at agent spawn, `AgentConfig` is built with `teams: self.store.list_teams_for_agent(agent_name).unwrap_or_default()`
- `src/agent/drivers/claude.rs` + `codex.rs` — pass `config.teams.clone()` into `PromptOptions`
- `src/store/agents.rs` — added `teams: Vec<TeamMembership>` field to `AgentConfig`
- `tests/driver_tests.rs` — added `teams: vec![]` to `AgentConfig` test literals

---

## Tasks Remaining (Tasks 6–12)

### Task 6 — Team REST handlers

**Files to create/modify:** `src/server/handlers/teams.rs` (new), `src/server/handlers/mod.rs`, `src/server/mod.rs`, `tests/server_tests.rs`

**Endpoints to implement:**
```
POST   /api/teams                        → handle_create_team
GET    /api/teams                        → handle_list_teams
GET    /api/teams/:name                  → handle_get_team
PATCH  /api/teams/:name                  → handle_update_team
DELETE /api/teams/:name                  → handle_delete_team
POST   /api/teams/:name/members          → handle_add_team_member
DELETE /api/teams/:name/members/:member  → handle_remove_team_member
```

The full handler code for all 7 endpoints is in the plan at `docs/superpowers/plans/2026-03-25-team-feature.md` lines 976–1241.

**`handle_create_team` lifecycle:**
1. Validate `collaboration_model` is `"leader_operators"` or `"swarm"`
2. `store.create_team(...)` → team_id
3. `store.create_channel(name, None, ChannelType::Team)` — creates the team channel
4. For each initial member: `store.add_team_member(...)` + `store.join_channel(...)` + stop/restart agent
5. `TeamWorkspace::init_team(...)` + `AgentWorkspace::init_team_memory(...)` per agent member

**`handle_delete_team` lifecycle:**
1. Collect agent members before deletion
2. `store.delete_team(team_id)` — cascades to team_members/signals/quorum
3. `store.archive_channel(ch.id)` — preserve message history
4. `TeamWorkspace::delete_team(...)` + `AgentWorkspace::delete_team_memory(...)` per agent
5. Stop + restart each former agent member

**`handle_add_team_member`:** add to store + join channel + init workspace + stop/restart agent
**`handle_remove_team_member`:** remove from store + `store.leave_channel(...)` + stop/restart agent

**DTOs in the plan:**
- `CreateTeamRequest { name, display_name, collaboration_model, leader_agent_name?, members: Vec<CreateTeamMemberRequest> }`
- `UpdateTeamRequest { display_name?, collaboration_model?, leader_agent_name?: Option<String> }` (all optional; use `Option<Option<String>>` for leader to distinguish "not provided" vs "set to null")
- `AddMemberRequest { member_name, member_type, member_id, role }`
- `TeamResponse { team: Team, members: Vec<TeamMember> }`

**`MockLifecycle` in `tests/server_tests.rs`:** verify it implements `stop_agent` and `start_agent`; add stubs if missing.

**Route registration in `src/server/mod.rs`:**
```rust
.route("/api/teams", get(handle_list_teams).post(handle_create_team))
.route("/api/teams/:name", get(handle_get_team).patch(handle_update_team).delete(handle_delete_team))
.route("/api/teams/:name/members", post(handle_add_team_member))
.route("/api/teams/:name/members/:member", axum::routing::delete(handle_remove_team_member))
```

**Tests to add:** `test_create_team_endpoint`, `test_list_teams_endpoint` (full bodies in plan lines 925–968)

**Important:** Check whether `AppState`, `api_err`, `internal_err`, `ApiResult` are the correct names in `src/server/handlers/mod.rs` — look at existing handlers (e.g., `channels.rs`) for the exact import path.

---

### Task 7 — @mention routing + Swarm consensus detection

**File modified:** `src/server/handlers/messages.rs`

**Also need new store helpers in `src/store/messages.rs`:**
- `post_message_with_forwarded_from(channel_id, sender_name, sender_type, content, attachments, forwarded_from: Option<ForwardedFrom>) -> Result<String>` — inserts message with `forwarded_from` JSON; returns message id
- `post_system_message(channel_id, content) -> Result<String>` — inserts a system-sender message

**@mention routing logic** (in send message handler, after message is saved):
```rust
let mention_re = regex::Regex::new(r"@([A-Za-z0-9_-]+)").unwrap();
for cap in mention_re.captures_iter(&req.content) {
    let mention = &cap[1];
    if let Ok(Some(team)) = state.store.get_team(mention) {
        if let Ok(Some(team_ch)) = state.store.find_channel_by_name(&team.name) {
            let forwarded_from = ForwardedFrom { channel_name, sender_name };
            if let Ok(fwd_msg_id) = state.store.post_message_with_forwarded_from(...) {
                let collab = make_collaboration_model(&team.collaboration_model);
                if let Some(prompt) = collab.deliberation_prompt() {
                    state.store.snapshot_swarm_quorum(&team.id, &fwd_msg_id)?;
                    state.store.post_system_message(&team_ch.id, &prompt)?;
                }
                // Wake team agent members
                for m in members.filter(|m| m.member_type == "agent") {
                    state.lifecycle.notify_agent(&m.member_name).await;
                }
            }
        }
    }
}
```

**Swarm consensus detection** (in send handler, for agent messages in team channels):
```rust
if sender_type == SenderType::Agent && ch.channel_type == ChannelType::Team {
    if let Ok(Some(team)) = state.store.get_team(&channel_name) {
        let collab = make_collaboration_model(&team.collaboration_model);
        if collab.is_consensus_signal(&req.content) {
            match state.store.record_swarm_signal(&team.id, &sender_name, &req.content) {
                Ok(true) => state.store.post_system_message(&ch.id, "[System] All members ready — execution begins."),
                Ok(false) => {},
                Err(e) => tracing::warn!("swarm signal error: {e}"),
            }
        }
    }
}
```

**Add to `Cargo.toml`:** `regex = "1"` if not already present.

**Test:** `test_at_mention_forwards_to_team_channel` (full body in plan lines 1299–1330)

---

### Task 8 — UI: types, API, store state

**Files:** `ui/src/types.ts`, `ui/src/api.ts`, `ui/src/store.tsx`

Add `Team`, `TeamMember`, `TeamResponse` interfaces to `types.ts`. Ensure `ChannelInfo.channel_type` includes `'team'`.

Add API functions: `listTeams`, `createTeam`, `updateTeam`, `deleteTeam`, `addTeamMember`, `removeTeamMember` (full signatures in plan lines 1469–1519).

Add `teams: Team[]` and `refreshTeams()` to zustand store. Call `refreshTeams()` inside `refreshServerInfo`.

**Verify with:** `cd ui && npm run build`

---

### Task 9 — UI: Sidebar badge + Create modal toggle

**Files:** `ui/src/components/Sidebar.tsx`, `ui/src/components/CreateChannelModal.tsx`

**Sidebar:** Render `[team]` badge for channels with `channel_type === 'team'` (same visual pattern as the `[sys]` badge on system channels). Add a `+ New Team` shortcut next to `+ New Channel` that opens the create modal with Team tab pre-selected.

**CreateChannelModal:** Add a Channel / Team toggle at the top. When Team is selected, show team-specific fields: collaboration model selector, initial members with roles, leader selector (Leader+Operators only). The toggle switches between two form modes — do **not** redeclare `const [mode, setMode]` in both branches; declare once at the top of the component. The modal must accept a `defaultMode?: 'channel' | 'team'` prop so the `+ New Team` shortcut can pre-select the Team tab.

On Team form submit: call `createTeam(...)` from the API, then `refreshTeams()` and `refreshChannels()`.

---

### Task 10 — UI: Team settings panel

**File created:** `ui/src/components/TeamSettings.tsx`

Show in channel header when channel has `channel_type === 'team'`. Fields:
- Display name (editable)
- Collaboration model selector (`Leader+Operators` / `Swarm`)
- Leader selector (only visible for Leader+Operators)
- Member list with roles; add/remove controls
- Delete team button (with confirmation)

Wire to API: `updateTeam`, `addTeamMember`, `removeTeamMember`, `deleteTeam`.

After delete: navigate away to a default channel (e.g., `#general`).

---

### Task 11 — UI: @mention autocomplete for teams

**File modified:** `ui/src/components/MentionTextarea.tsx`

Include teams in the existing `@mention` autocomplete picker. Read `store.teams` and merge them into the suggestion list with a group icon to distinguish from agent mentions.

---

### Task 12 — End-to-end verification

Run the full test suite and a manual browser pass covering the QA plan at `qa/runs/2026-03-25T000000/plan.md`. The QA plan calls for Core Regression mode with the `mixed-runtime-trio` preset (bot-a: claude/sonnet, bot-b: claude/opus, bot-c: codex/gpt-5.4-mini).

Minimum automated checks:
```bash
cargo test
cd ui && npm run build
```

Then run the browser QA per the plan (or document why it can't be run).

---

## Architecture Notes for Remaining Tasks

### AppState / handler imports
Look at `src/server/handlers/channels.rs` for the exact import pattern. Typical:
```rust
use super::{api_err, internal_err, ApiResult, AppState};
```

### `AgentLifecycle` trait methods
The `stop_agent(name)` and `start_agent(name, session_id)` methods are on `AgentLifecycle` in `src/server/mod.rs`. `MockLifecycle` in `tests/server_tests.rs` must implement all trait methods — add stubs as needed when adding new trait methods.

### `notify_agent`
Used to wake an agent after a message arrives in their channel. Check `src/server/mod.rs` for the current `AgentLifecycle` trait definition; `notify_agent` may already exist or may need adding.

### `find_channel_by_name`
Exists in `src/store/channels.rs`. Returns `Result<Option<Channel>>`.

### `join_channel` / `leave_channel`
- `join_channel(channel_name, member_name, sender_type)` — in `src/store/channels.rs`
- `leave_channel(channel_name, member_name)` — added in Task 2 in `src/store/teams.rs`

### `agents_dir()`
If this method doesn't exist on `Store`, look at how `AgentWorkspace::new` is called elsewhere in `manager.rs` to get the agents dir path. May need to add `pub fn agents_dir(&self) -> PathBuf` to Store similar to `teams_dir()`.

### Regex crate
If `regex` is not in `Cargo.toml`, add: `regex = "1"` under `[dependencies]`.

---

## Key Design Decisions (Don't Re-debate)

1. **Team channels appear in the main Channels sidebar with a `[team]` badge** — no separate sidebar section
2. **Both `+ New Channel → Team tab` and `+ New Team` shortcut** use the same modal with a `defaultMode` prop
3. **System prompt rebuilt by stop+restart** — no hot-reload; uses existing `AgentLifecycle` stop/start
4. **Swarm quorum is snapshotted at task arrival** — agents joining after the snapshot are not counted for that task
5. **Non-quorum signals are silently discarded** — `record_swarm_signal` returns `Ok(false)` for non-quorum agents
6. **Team deletion archives the channel** (not hard-deletes) — preserves message history
7. **`forwarded_from` parse errors produce `None`** — acceptable since the codebase is the only writer

---

## How to Continue

```bash
git checkout claude/team-concept
cargo test   # should show 25 passed
```

Then implement Tasks 6–12 in order. Each task is self-contained. Read the full plan at `docs/superpowers/plans/2026-03-25-team-feature.md` for the complete step-by-step instructions, including exact code for each step.

After all tasks complete, run:
```bash
cargo test
cd ui && npm run build
```

Then open the QA plan at `qa/runs/2026-03-25T000000/plan.md` and execute the browser QA pass before merging to `main`.
