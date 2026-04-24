# Platform Workspaces Implementation Plan

## Scope

Add first-class CLI commands for platform workspaces on top of the existing workspace foundation. This plan does not fully migrate server routes or bridge credentials to workspace-scoped behavior.

## Steps

1. Store APIs
   - Add lookup by slug or name.
   - Add active workspace setter.
   - Add rename helper that updates name and slug together.
   - Enforce unique workspace slug values in schema and migration.

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

4. Error handling
   - Missing active workspace points users to `chorus setup` or `chorus workspace create`.
   - Unknown selector includes a short list of available workspaces.
   - Duplicate slug fails loudly.
   - Invalid name fails before mutation.

5. Tests
   - Store tests for lookup, switch, duplicate slug, and rename.
   - CLI unit tests for output formatting where practical.
   - CLI integration tests for create/list/current/switch/rename using temp data dirs.

6. Docs
   - Update `docs/CLI.md` with the new workspace commands.
   - Keep the architecture note in this plan doc until the larger cloud architecture doc exists.

## Non-Goals

- Cloud account sign-in/sign-up.
- Workspace invitations or role management beyond owner membership.
- Live server workspace switching without restart.
- Full route-level workspace scoping for every object.
- Bridge credential workspace enforcement.
