# Workspace Architecture

Workspaces are the root collaboration boundary for Chorus. They exist because
the product is moving from a single local chat graph toward a cloud platform
where channels, agents, teams, tasks, and humans need an explicit owner and
isolation boundary.

Local Chorus still stays simple: setup creates one local workspace, and users
can create more when they need separate projects. Cloud identity, billing, and
organization policy are future layers on top of the same workspace model, not a
separate local-only concept.

---

## Background

Before workspaces, Chorus treated the local SQLite store as one implicit
collaboration space. That made the local experience fast, but it left no clean
place to attach platform concerns:

- human identity and membership
- cloud sync and sharing
- project-level channels, agents, teams, and tasks
- future permissions and audit trails

The workspace model makes that boundary explicit without requiring cloud auth
for local deployment.

```
Cloud platform, later
    -> user identity / auth
        -> workspace
            -> humans
            -> channels
            -> agents
            -> teams
            -> tasks

Local deployment, now
    -> local human from config / OS username
        -> local workspace
            -> channels
            -> agents
            -> teams
            -> tasks
```

---

## User Model

Setup creates an explicit workspace and activates it.

```
chorus setup
  -> choose workspace name, default "Chorus Local"
  -> create local human
  -> create workspace
  -> add local human as owner
  -> set active workspace
```

Creating another workspace does not switch to it. Switching is an explicit
action because create is often used to prepare a project, while switch changes
where subsequent channel, agent, and team commands operate.

```
chorus workspace create "Client A"   # creates only
chorus workspace switch client-a     # changes active workspace
chorus channel create planning       # created inside Client A
```

The CLI calls the server API. It does not open SQLite directly, because the
running server owns active workspace state, realtime updates, and future auth
checks.

---

## Architecture

Workspace state crosses three layers:

```
Frontend settings panel
    |
    v
HTTP API: /api/workspaces
    |
    v
Store: workspaces.rs
    |
    v
SQLite tables and indexes
```

The active workspace is local server state backed by SQLite. It answers "where
should commands and UI actions go right now?" It is not the same thing as
workspace membership or cloud authentication.

```
workspaces                 durable platform boundary
workspace_members          humans allowed in that workspace
local_workspace_state      local active workspace pointer
channels.workspace_id      channel belongs to workspace
agents.workspace_id        agent belongs to workspace
teams.workspace_id         team belongs to workspace
```

The shared MCP bridge remains local. It is the local runtime connection point
for agents and tools; it should not become the cloud credential authority.

---

## Data Model

### Core Tables

| Table | Purpose |
| --- | --- |
| `workspaces` | One row per collaboration boundary. Holds `id`, display `name`, stable unique `slug`, `mode`, creator, and timestamp. |
| `workspace_members` | Joins humans to workspaces. The first local member is inserted as `owner`. |
| `local_workspace_state` | Stores local process state such as `active_workspace_id`. |
| `humans` | Local human identities. Today this is config or OS username backed, not cloud auth. |

### Scoped Resources

| Resource | Workspace relationship |
| --- | --- |
| `channels.workspace_id` | Required direct workspace ownership. Messages, inbox state, tasks, and attachments are reached through channels. |
| `agents.workspace_id` | Required direct workspace ownership. Agent names are still globally unique while `agent_env_vars` and runtime lifecycle remain name-keyed. |
| `teams.workspace_id` | Required direct workspace ownership. Team backing channels are created in the same workspace transaction. |
| `tasks` | Indirect ownership through `tasks.channel_id`. Task sub-channels inherit workspace through their channel row. |
| `messages` | Indirect ownership through `messages.channel_id`. |
| `attachments` | Indirect ownership through message attachment links. |

### Uniqueness

Workspace slugs are globally unique. Channel and team names are unique inside a
workspace. Directly scoped resources cannot be written without a `workspace_id`.

Agent names are currently still globally unique because `agent_env_vars` uses
`agent_name` as its foreign key and some runtime/session paths are name-keyed.
The workspace-scoped agent index exists as a step toward full scoping, but
duplicate agent names across workspaces are intentionally not enabled yet.

### Creation Invariants

Workspace creation is transactional:

1. ensure the local human exists
2. insert the workspace
3. insert the owner membership
4. provision the workspace-scoped writable `#all` system channel
5. optionally activate it
6. return the inserted workspace

Channel, agent, and team creation paths require an explicit workspace id or
resolve the active local workspace before writing rows.

Team creation with its backing team channel is also transactional so a failed
channel insert cannot leave a team without its channel.

---

## API and CLI Behavior

| Operation | Behavior |
| --- | --- |
| `GET /api/workspaces` | Lists workspaces with active marker and channel, agent, human counts. |
| `GET /api/workspaces/current` | Returns the active workspace or fails loudly if none is active. |
| `POST /api/workspaces` | Creates a workspace without activating it. |
| `POST /api/workspaces/switch` | Sets the active workspace by slug or exact name. |
| `PATCH /api/workspaces/current` | Renames the active workspace; slug remains stable. |
| `DELETE /api/workspaces/{selector}` | Hard-deletes a workspace and its scoped database data. If it was active, the active workspace becomes unset until the user switches explicitly. |

`chorus workspace` is a thin client over those endpoints. This keeps CLI,
frontend, and future cloud auth on the same behavior path.

---

## Restrictions

These are current product and implementation boundaries, not long-term product
goals.

- Workspaces are local-only today. `WorkspaceMode::Cloud` is reserved for the
  platform path.
- There is no sign-in or sign-up yet. The local human comes from `config.toml`
  or the OS username.
- Active workspace is a single pointer for the local running server. It is not
  per browser tab or per CLI shell.
- Creating a workspace does not switch to it.
- New local workspaces auto-provision a writable `#all` system channel scoped
  to that workspace, and add the workspace owner as its first member.
- Deleting a workspace is destructive: workspace-scoped channels, messages,
  tasks, agents, teams, memberships, trace events, attachment links, orphaned
  attachment rows, and orphaned attachment files are removed.
- Deleting a workspace does not yet remove per-agent or per-team runtime
  workspace directories. Team filesystem paths are still keyed by team name, so
  wiping them during workspace deletion would be unsafe until those paths become
  workspace-scoped.
- The CLI requires a running Chorus server and must call the API instead of
  opening SQLite directly.
- Agent names are not fully workspace-scoped yet.
- The local bridge is not a cloud credential store. Agent bridge credentials and
  human auth tokens are separate concerns.

---

## Design Direction

The next platform step is to keep the local bridge local while moving identity,
membership, and workspace metadata into cloud APIs.

The important split is:

| Concern | Owner |
| --- | --- |
| Human authentication token | Cloud platform; proves which human is using Chorus. |
| Workspace membership | Cloud platform; controls which workspaces the human can access. |
| Active workspace | Local app state; selects the current operating context. |
| Agent bridge credentials | Local bridge; lets a local agent runtime connect to the local Chorus bridge. |
| Runtime provider credentials | Local machine or provider auth; not stored as human workspace auth. |

Keep that split intact. It lets local Chorus remain lightweight while leaving a
clean path to shared cloud workspaces.
