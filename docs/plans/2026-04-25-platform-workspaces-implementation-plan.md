# Platform Workspaces Implementation Plan

## Scope

Add first-class CLI commands for platform workspaces on top of the existing workspace foundation. This plan also scopes the core read surfaces to the active workspace so switching workspaces has visible platform behavior. It does not fully migrate every mutation route or bridge credential to workspace-scoped behavior.

## Steps

1. Store APIs
   - Add lookup by slug or name.
   - Add active workspace setter.
   - Add active workspace resolver that validates the stored ID still exists.
   - Add rename helper that updates the display name while keeping the slug stable.
   - Enforce unique workspace slug values in schema and migration.
   - Add deterministic slug collision handling for workspace creation (`acme`, `acme-1`, `acme-2`).
   - Add workspace-aware list helpers for channels, agents, and teams.
   - Persist local human identity in local state or config so workspace ownership does not depend on future OS username changes.

2. CLI command shape
   - Add `WorkspaceCommands` under `chorus workspace`.
   - Implement `current`, `list`, `create`, `switch`, and `rename`.
   - Resolve data dir using the same default as setup/check.

3. CLI behavior
   - `current` prints the active workspace name, slug, mode, and id.
   - `list` marks the active workspace.
   - `create <name>` creates a local platform workspace for the local human and switches to it.
   - `switch <workspace>` accepts slug or exact name.
   - `rename <new-name>` renames the active workspace unless a selector is added later.
   - `switch` and `rename` print a restart hint when they change local state while a server may already be running.

4. Core resource isolation
   - Resolve the active workspace at server startup.
   - Filter core resource list routes by active workspace: channels, agents, teams, and status/server-info surfaces.
   - Keep unscoped legacy rows hidden from scoped list results.
   - Do not add legacy migration or backfill; Chorus is still in rapid development and compatibility is not a goal for this slice.
   - Do not claim complete workspace isolation until mutation routes and bridge credentials are scoped in later slices.

5. Error handling
   - Missing active workspace points users to `chorus setup` or `chorus workspace create`.
   - Unknown selector includes a short list of available workspaces.
   - Duplicate slug fails loudly.
   - Invalid name fails before mutation.
   - Stale active workspace IDs are treated as no active workspace, with a clear recovery command.

6. Tests
   - Store tests for lookup, switch, duplicate slug, and rename.
   - Store tests proving channels, agents, and teams can be listed by workspace.
   - CLI unit tests for output formatting where practical.
   - CLI integration tests for create/list/current/switch/rename using temp data dirs.
   - Integration test proving switching workspace changes visible core resources.

7. Docs
   - Update `docs/CLI.md` with the new workspace commands.
   - Keep the architecture note in this plan doc until the larger cloud architecture doc exists.

## Non-Goals

- Cloud account sign-in/sign-up.
- Workspace invitations or role management beyond owner membership.
- Live server workspace switching without restart.
- Full route-level workspace scoping for every object and mutation.
- Bridge credential workspace enforcement.
