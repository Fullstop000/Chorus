# User Settings Panel

## Problem

The sidebar footer shows the current user with a settings cog button, but the
button is inert. Users have no way to set a display name or view account info.
The `humans` table stores only `name` (OS username) and `created_at`.

## Approach

Add a `display_name` column to the humans table, a PATCH endpoint to update it,
and a dialog-based settings panel triggered by the existing sidebar cog button.
Follow the TeamSettings dialog pattern and Chorus design language (zero-radius,
monospace, warm/industrial).

## Schema

Add `display_name TEXT` column to the `humans` table via migration:

```sql
ALTER TABLE humans ADD COLUMN display_name TEXT DEFAULT NULL;
```

The Human struct gains `display_name: Option<String>`.

## Backend

### Store layer (`src/store/humans.rs`)

- `update_human_display_name(name: &str, display_name: Option<&str>) -> Result<()>`
  Updates the display_name column for the given human.
- `get_humans()` already returns all columns; add `display_name` to the SELECT.

### DTO (`src/server/handlers/dto.rs`)

`HumanInfo` gains `display_name: Option<String>`.

### Handler (`src/server/handlers/mod.rs`)

New handler `handle_update_human`:
- Route: `PATCH /api/humans/:name`
- Body: `{ "display_name": "..." }` (nullable)
- Validates: name matches an existing human row.
- Returns: updated `HumanInfo`.

### Route registration (`src/server/mod.rs`)

Add `.route("/humans/:name", patch(handle_update_human))` alongside existing
`/humans` GET.

## Frontend

### Data layer (`ui/src/data/channels.ts`)

- Update `HumanInfo` interface to include `display_name?: string`.
- Add `updateHuman(name: string, body: { display_name?: string }): Promise<HumanInfo>`.

### Component (`ui/src/components/channels/UserSettings.tsx`)

Dialog-based panel matching TeamSettings pattern:

- **Username** — read-only label showing OS username.
- **Display Name** — editable input, saved via PATCH.
- **Member Since** — read-only, shows `created_at` formatted.
- **Actions** — Save + Close buttons, matching TeamSettings layout.

### Sidebar wiring (`ui/src/pages/Sidebar/Sidebar.tsx`)

Wire the existing cog button to open the UserSettings dialog.

### Display name usage

Update the sidebar footer and the Humans list to show `display_name ?? name`
where appropriate.

## Data flow

1. User clicks cog → dialog opens with current user info.
2. User edits display name → clicks Save → PATCH /api/humans/:name.
3. On success → invalidate humans query → sidebar/footer reflect new name.
4. Close button dismisses dialog without saving.

## Error handling

- PATCH with unknown username → 404 with `"human not found: {name}"`.
- Empty display_name string → normalize to NULL (use OS username as fallback).
- Network/store errors surface via FormError in the dialog, matching TeamSettings.

## Testing

- **Backend:** Unit test for `update_human_display_name` roundtrip (store test).
- **Backend:** Existing `cargo test` ensures migration + schema compatibility.
- **Frontend:** Vitest for the new `updateHuman` data function.
- **Typecheck:** `tsc --noEmit` covers component typing.

## Out of scope

- Avatar customization (deterministic by design).
- Notification preferences (no notification system exists).
- Theme selection (single cream theme per DESIGN.md).
- Dark mode (explicitly excluded in DESIGN.md).
