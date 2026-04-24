# Platform Workspaces Design

## Background

Chorus is moving from a purely local collaboration app toward a cloud platform architecture. Workspace should be the platform tenant boundary, not a local-only convenience. Local deployment remains simple by running a local platform instance with implicit local human identity.

The workspace foundation already introduces `workspaces`, `workspace_members`, and `local_workspace_state`, and `chorus setup` creates the first local workspace. The next step is to expose workspace management as first-class CLI behavior and make active workspace resolution an explicit platform control plane.

## Product Model

```text
Human User
  -> Platform Account
      -> Workspace(s)
          -> Channels
          -> Agents
          -> Teams
          -> Tasks
          -> Artifacts
```

Local Chorus maps to the same model:

```text
Local Deployment
  platform = local Chorus instance
  auth = implicit local human
  workspace = explicit platform workspace
  bridge = local-only agent connection layer
```

## CLI UX

`chorus setup` creates the first platform workspace:

```bash
chorus setup
# Human name: zht
# Workspace name: Chorus Local
```

Workspace commands:

```bash
chorus workspace current
chorus workspace list
chorus workspace create "Acme"
chorus workspace switch "Acme"
chorus workspace rename "Acme AI"
```

`chorus workspace create` switches to the new workspace immediately. Creation usually means the user wants to work there now, and `workspace current` makes the state visible.

## Architecture

```text
CLI
  -> resolves data dir
  -> resolves local human identity
  -> resolves active workspace
  -> calls Store workspace APIs

Server
  -> loads active workspace on startup
  -> platform routes operate inside active workspace context

Bridge
  -> stays local
  -> credentials are scoped to agent + workspace
  -> never becomes the human auth token
```

## Data Model

Existing foundation:

```text
workspaces(id, name, slug, mode, created_by_human, created_at)
workspace_members(workspace_id, human_name, role)
local_workspace_state(key='active_workspace_id', workspace_id)
```

Needed store APIs:

- `set_active_workspace(workspace_id)`
- `get_workspace_by_selector(selector)`
- `rename_workspace(workspace_id, new_name, slug_policy)`
- `list_workspaces_for_human(human_name)` for CLI output
- workspace-aware list APIs for core resources (`channels`, `agents`, `teams`)

Workspace identity in CLI should use name or slug, not raw UUID. The database should enforce unique workspace slug values so CLI selection is deterministic. Slugs should be stable by default: renaming a workspace changes the display name only unless the user explicitly asks to regenerate the slug in a later command.

Local human identity must be consistent. `chorus setup` should persist the local human name, defaulting to the OS username and allowing explicit input. Later `chorus workspace create` calls should read that persisted value instead of re-reading the OS username every time.

## Failure Rules

- No active workspace: fail loudly with `run chorus setup or chorus workspace create`.
- Unknown workspace: fail with a short candidate list.
- Ambiguous selector: fail and ask for the slug.
- Invalid rename: fail before mutating state.
- Active workspace pointer references a missing workspace: fail as "no active workspace" and ask the user to switch or create one.
- Legacy unscoped resources exist after upgrade: no migration/backfill. The product is still in rapid development, so scoped workspace results may ignore old unscoped rows.
- Bridge credential with the wrong workspace: reject.

Switching workspace while a server is already running only updates local state in the first slice. The server picks up the active workspace on restart. Live workspace switching can be designed later.

## Implementation Slice

Keep the next PR focused on the workspace control plane:

1. Add `chorus workspace current/list/create/switch/rename`.
2. Make `workspace create` auto-switch to the new workspace.
3. Keep `chorus setup` responsible for first workspace creation.
4. Scope core resource list paths (`channels`, `agents`, `teams`, and `status`) to the active workspace.
5. Add docs and tests for CLI behavior and resource isolation.

Full route-level workspace scoping for every mutation and bridge credential is a later slice. The first slice must still prove that switching the active workspace changes the visible platform surface for core resources; otherwise `workspace switch` is only cosmetic. Compatibility migration for pre-workspace data is intentionally out of scope.
