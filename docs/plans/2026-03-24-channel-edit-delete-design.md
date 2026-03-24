# Channel Edit And Delete Design

**Date:** 2026-03-24

## Goal

Add Slock-style channel edit and delete controls to Chorus so users can rename channels, update descriptions, and choose between archiving a channel or deleting it permanently.

## Product Direction

The feature should follow the existing Slock-inspired visual language rather than introducing a generic settings surface. User channels in the sidebar should expose:

- an inline edit pencil on hover and on the active row
- a row-level delete action

System channels and DMs remain read-only.

## User Experience

### Edit flow

- Clicking the channel pencil opens an `Edit Channel` modal.
- The modal uses the same brutalist style as the rest of the workspace.
- Fields:
  - `name`
  - `description`
- Save is allowed when the normalized name is non-empty and does not collide with another channel.

### Delete flow

- Choosing `Delete Channel` opens a confirmation modal.
- The modal presents two explicit outcomes:
  - `Archive channel`
  - `Delete permanently`
  - `Cancel`

This keeps destructive behavior intentional while still letting the user decide retention policy at deletion time.

## Data And Persistence

### Rename

- Channel rename must preserve `channel.id`.
- Messages, tasks, memberships, unread state, and thread relationships stay attached to the same channel row.
- Name normalization should match channel create:
  - trim whitespace
  - strip leading `#`
  - lowercase

### Archive

- Archive is a soft delete.
- Archived channels disappear from the normal sidebar and `server-info` response.
- Archived data remains in SQLite for possible future recovery or audit.
- No archived-channel management UI is required in this iteration.

### Permanent delete

- Permanent delete removes:
  - the channel row
  - channel memberships
  - channel tasks
  - channel messages
  - thread replies within that channel
  - orphaned attachment records and files tied only to deleted messages

## API Design

Use stable `channel_id`-based mutation routes so rename does not break targeting:

- `PATCH /api/channels/:channel_id`
- `POST /api/channels/:channel_id/archive`
- `DELETE /api/channels/:channel_id`

These routes should reject mutations against system channels and DMs.

## Client State Rules

- If the active channel is renamed, keep the user on that channel and rewrite `selectedChannel` to the new `#name`.
- If the active channel is archived or deleted:
  - close any open thread panel
  - fall back to the first visible joined non-system channel
  - if none remain, clear the selection and show the empty state

## Error Handling

- Duplicate logical names are rejected after normalization.
- Invalid or empty names are rejected before or by the API.
- On failure, the modal stays open and shows an inline error.
- The client should prefer server-confirmed refresh over optimistic mutation in v1.

## QA Coverage Changes

The feature must ship with catalog updates.

- Tighten `CHN-002` to cover edit normalization and duplicate rejection.
- Convert `CHN-004` from blocked to executable once delete is shipped.
- Add `CHN-005` if needed for edit-specific identity preservation and active-selection behavior.
- Update `qa/QA_CASES.md` so the index matches `qa/cases/channels.md`.

Because this touches channel management and core visible workflow, implementation should plan for the QA SOP’s Core Regression path after code changes are ready.

## Scope Decisions

- Include edit, archive, and permanent delete in the same feature.
- Do not add archived-channel browsing or restore in this iteration.
- Do not broaden the scope into channel membership management or channel settings pages.
