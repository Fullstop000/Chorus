# User Settings Panel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a user settings dialog with display name editing, wired to the existing sidebar cog button.

**Architecture:** Schema migration adds `display_name` column to humans table. New PATCH `/api/humans/:name` endpoint updates it. Frontend UserSettings dialog mirrors TeamSettings pattern. Sidebar cog opens the dialog; display names propagate to sidebar footer and humans list.

**Tech Stack:** Rust/Axum (backend), SQLite (store), React/TypeScript (frontend), shadcn/ui Dialog components

---

### File Map

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `src/store/migrations.rs` | Add `display_name` column migration |
| Modify | `src/store/humans.rs` | Add `display_name` to `Human`, add `update_human_display_name()` |
| Modify | `src/server/handlers/dto.rs` | Add `display_name` to `HumanInfo` |
| Modify | `src/server/handlers/mod.rs` | Add `handle_update_human` handler |
| Modify | `src/server/mod.rs` | Register PATCH `/api/humans/:name` route |
| Modify | `ui/src/data/channels.ts` | Add `display_name` to `HumanInfo`, add `updateHuman()` |
| Modify | `ui/src/data/index.ts` | Re-export `updateHuman` |
| Create | `ui/src/components/channels/UserSettings.tsx` | Settings dialog component |
| Create | `ui/src/components/channels/UserSettings.css` | Dialog styles |
| Modify | `ui/src/pages/Sidebar/Sidebar.tsx` | Wire cog button to UserSettings dialog |

---

### Task 1: Schema Migration — add display_name to humans

**Files:**
- Modify: `src/store/migrations.rs`

- [ ] **Step 1: Add the migration function**

Add to `src/store/migrations.rs` at the end of the file, before the closing:

```rust
/// Add display_name column to humans table for user-friendly names.
fn migrate_add_display_name_to_humans(conn: &Connection) -> Result<()> {
    let has_column = conn
        .prepare("PRAGMA table_info(humans)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|col| col == "display_name");
    if !has_column {
        conn.execute_batch("ALTER TABLE humans ADD COLUMN display_name TEXT")?;
        tracing::info!("migration: added display_name column to humans");
    }
    Ok(())
}
```

- [ ] **Step 2: Register migration in run_migrations**

In `src/store/migrations.rs`, add `migrate_add_display_name_to_humans(conn)?;` as the last call in `run_migrations()`, after `migrate_create_trace_events_table(conn)?;`.

- [ ] **Step 3: Run tests to verify migration is compatible**

Run: `cargo test --lib`
Expected: All existing tests pass (migration is idempotent via column check).

- [ ] **Step 4: Commit**

```bash
git add src/store/migrations.rs
git commit -m "feat(store): add display_name column migration for humans table

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 2: Store Layer — update Human struct and add update method

**Files:**
- Modify: `src/store/humans.rs`

- [ ] **Step 1: Add display_name field to Human struct**

In `src/store/humans.rs`, update the `Human` struct to add the `display_name` field:

```rust
/// Registered human user (can post and own channels).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Human {
    /// Username (typically OS login) used as sender id.
    pub name: String,
    /// Optional user-chosen display name.
    pub display_name: Option<String>,
    /// When the human row was inserted.
    pub created_at: DateTime<Utc>,
}
```

- [ ] **Step 2: Update get_humans to select display_name**

Update the `get_humans` method to include `display_name`:

```rust
    pub fn get_humans(&self) -> Result<Vec<Human>> {
        let conn = self.conn.lock().unwrap();
        let rows = conn
            .prepare("SELECT name, display_name, created_at FROM humans ORDER BY name")?
            .query_map([], |row| {
                Ok(Human {
                    name: row.get(0)?,
                    display_name: row.get(1)?,
                    created_at: parse_datetime(&row.get::<_, String>(2)?),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }
```

- [ ] **Step 3: Add update_human_display_name method**

Add after `get_humans`:

```rust
    /// Update the display name for a human user. Pass `None` to clear.
    pub fn update_human_display_name(
        &self,
        name: &str,
        display_name: Option<&str>,
    ) -> Result<Human> {
        let conn = self.conn.lock().unwrap();
        let updated = conn.execute(
            "UPDATE humans SET display_name = ?2 WHERE name = ?1",
            params![name, display_name],
        )?;
        if updated == 0 {
            anyhow::bail!("human not found: {name}");
        }
        let human = conn.query_row(
            "SELECT name, display_name, created_at FROM humans WHERE name = ?1",
            params![name],
            |row| {
                Ok(Human {
                    name: row.get(0)?,
                    display_name: row.get(1)?,
                    created_at: parse_datetime(&row.get::<_, String>(2)?),
                })
            },
        )?;
        Ok(human)
    }
```

- [ ] **Step 4: Run tests**

Run: `cargo test --lib`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/store/humans.rs
git commit -m "feat(store): add display_name to Human struct and update method

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 3: DTO and Handler — HumanInfo display_name + PATCH endpoint

**Files:**
- Modify: `src/server/handlers/dto.rs`
- Modify: `src/server/handlers/mod.rs`
- Modify: `src/server/mod.rs`

- [ ] **Step 1: Add display_name to HumanInfo DTO**

In `src/server/handlers/dto.rs`, update `HumanInfo`:

```rust
/// Human user row for agent workspace snapshots and the UI shell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HumanInfo {
    /// OS / login username used as human id.
    pub name: String,
    /// Optional user-chosen display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}
```

- [ ] **Step 2: Update From<Human> for HumanInfo**

In `src/server/handlers/dto.rs`, update the `From` impl:

```rust
impl From<Human> for HumanInfo {
    fn from(human: Human) -> Self {
        Self {
            name: human.name,
            display_name: human.display_name,
        }
    }
}
```

- [ ] **Step 3: Add update handler in mod.rs**

In `src/server/handlers/mod.rs`, add request type and handler after `handle_list_humans`:

```rust
#[derive(Debug, Deserialize)]
pub struct UpdateHumanRequest {
    pub display_name: Option<String>,
}

pub async fn handle_update_human(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<UpdateHumanRequest>,
) -> ApiResult<dto::HumanInfo> {
    // Normalize empty strings to None
    let display_name = body
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let human = state
        .store
        .update_human_display_name(&name, display_name)
        .map_err(|e| app_err!(StatusCode::NOT_FOUND, e.to_string()))?;
    Ok(Json(dto::HumanInfo::from(human)))
}
```

Add `Deserialize` to the existing serde import at the top of `src/server/handlers/mod.rs` if not already there (it is already imported via `use serde::{Deserialize, Serialize}` from dto.rs, but check the handler file's own imports).

- [ ] **Step 4: Register the route**

In `src/server/mod.rs`, add the route inside the `api_router`. After the `.route("/humans", get(handle_list_humans))` line, add:

```rust
        .route("/humans/{name}", patch(handle_update_human))
```

- [ ] **Step 5: Run tests and lint**

Run: `cargo test --lib && cargo clippy --all-targets -- -D warnings`
Expected: All pass.

- [ ] **Step 6: Commit**

```bash
git add src/server/handlers/dto.rs src/server/handlers/mod.rs src/server/mod.rs
git commit -m "feat(api): add PATCH /api/humans/:name endpoint for display_name

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 4: Frontend Data Layer — update HumanInfo type + add updateHuman

**Files:**
- Modify: `ui/src/data/channels.ts`
- Modify: `ui/src/data/index.ts`

- [ ] **Step 1: Add display_name to HumanInfo interface**

In `ui/src/data/channels.ts`, update the `HumanInfo` interface (line 22-24):

```typescript
export interface HumanInfo {
  name: string
  display_name?: string
}
```

- [ ] **Step 2: Add updateHuman API function**

In `ui/src/data/channels.ts`, add after the `listHumans` function (after line 119):

```typescript
export function updateHuman(
  name: string,
  body: { display_name?: string | null },
): Promise<HumanInfo> {
  return patch(`/api/humans/${encodeURIComponent(name)}`, body)
}
```

- [ ] **Step 3: Re-export updateHuman from index.ts**

In `ui/src/data/index.ts`, add `updateHuman` to the channels re-export block. Find the line with `listHumans,` and add `updateHuman,` after it.

- [ ] **Step 4: Run typecheck**

Run: `cd ui && npx tsc --noEmit`
Expected: No errors.

- [ ] **Step 5: Commit**

```bash
git add ui/src/data/channels.ts ui/src/data/index.ts
git commit -m "feat(ui): add display_name to HumanInfo and updateHuman API function

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 5: UserSettings Component

**Files:**
- Create: `ui/src/components/channels/UserSettings.css`
- Create: `ui/src/components/channels/UserSettings.tsx`

- [ ] **Step 1: Create UserSettings.css**

Create `ui/src/components/channels/UserSettings.css`:

```css
.user-settings-card {
  width: min(480px, 96vw);
}

.user-settings-info {
  display: grid;
  gap: 4px;
}

.user-settings-info-label {
  font-family: var(--font-mono);
  font-size: 11px;
  letter-spacing: 0.04em;
  color: var(--color-muted-foreground);
  text-transform: uppercase;
}

.user-settings-info-value {
  font-family: var(--font-mono);
  font-size: 13px;
}

.user-settings-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 20px;
}
```

- [ ] **Step 2: Create UserSettings.tsx**

Create `ui/src/components/channels/UserSettings.tsx`:

```tsx
import { useEffect, useState } from "react";
import { updateHuman } from "../../data";
import { useQueryClient } from "@tanstack/react-query";
import { channelQueryKeys } from "../../data";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogClose,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { FormField, FormError } from "@/components/ui/form";
import { Label } from "@/components/ui/label";
import "./UserSettings.css";

interface Props {
  username: string;
  displayName?: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function UserSettings({
  username,
  displayName,
  open,
  onOpenChange,
}: Props) {
  const queryClient = useQueryClient();
  const [editDisplayName, setEditDisplayName] = useState(displayName ?? "");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setEditDisplayName(displayName ?? "");
  }, [displayName, open]);

  async function handleSave() {
    setSaving(true);
    setError(null);
    try {
      const trimmed = editDisplayName.trim();
      await updateHuman(username, {
        display_name: trimmed || null,
      });
      await queryClient.invalidateQueries({
        queryKey: channelQueryKeys.humans,
      });
      onOpenChange(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="user-settings-card">
        <DialogHeader>
          <div className="flex flex-col gap-1">
            <DialogTitle>User Settings</DialogTitle>
            <DialogDescription>{username}</DialogDescription>
          </div>
          <DialogClose className="h-8 w-8 grid place-items-center text-muted-foreground hover:bg-secondary hover:text-foreground">
            ×
          </DialogClose>
        </DialogHeader>

        {error && <FormError>{error}</FormError>}

        <FormField>
          <Label>Display Name</Label>
          <Input
            value={editDisplayName}
            onChange={(event) => setEditDisplayName(event.target.value)}
            placeholder={username}
            disabled={saving}
          />
        </FormField>

        <div className="user-settings-info">
          <div className="user-settings-info-label">Username</div>
          <div className="user-settings-info-value">{username}</div>
        </div>

        <div className="user-settings-actions">
          <Button
            variant="outline"
            type="button"
            onClick={() => onOpenChange(false)}
            disabled={saving}
          >
            Close
          </Button>
          <Button type="button" onClick={handleSave} disabled={saving}>
            Save
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
```

- [ ] **Step 3: Run typecheck**

Run: `cd ui && npx tsc --noEmit`
Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add ui/src/components/channels/UserSettings.tsx ui/src/components/channels/UserSettings.css
git commit -m "feat(ui): add UserSettings dialog component

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 6: Wire Sidebar Cog + Display Name Usage

**Files:**
- Modify: `ui/src/pages/Sidebar/Sidebar.tsx`

- [ ] **Step 1: Import UserSettings and add state**

In `ui/src/pages/Sidebar/Sidebar.tsx`, add the import near the top with other imports:

```typescript
import { UserSettings } from '../../components/channels/UserSettings'
```

- [ ] **Step 2: Add state and lookup current user's display name**

Inside the `Sidebar` component, add state for the settings dialog. Find the line `const [humansCollapsed, setHumansCollapsed] = useState(false)` and add after it:

```typescript
  const [showUserSettings, setShowUserSettings] = useState(false)
```

Also, derive the current user's display name from the humans list. After the `const channels = loadedChannels.filter(isVisibleSidebarChannel)` line, add:

```typescript
  const currentHuman = humans.find((h) => h.name === currentUser)
  const currentDisplayName = currentHuman?.display_name
```

- [ ] **Step 3: Wire the cog button onClick**

Find the settings cog button (the `<button className="sidebar-footer-cog"` element near line 367). Replace it with:

```tsx
          <button
            className="sidebar-footer-cog"
            type="button"
            aria-label="Open settings"
            onClick={() => setShowUserSettings(true)}
          >
            <Settings2 size={15} />
          </button>
```

- [ ] **Step 4: Update footer to show display name**

Find the `<span className="sidebar-footer-name">{currentUser}</span>` line and update to:

```tsx
            <span className="sidebar-footer-name">{currentDisplayName || currentUser}</span>
```

- [ ] **Step 5: Update humans list to show display names**

Find the humans list item that renders `<span className="sidebar-item-text">{h.name}</span>` (around line 335). Update to:

```tsx
                    <span className="sidebar-item-text">{h.display_name || h.name}</span>
```

- [ ] **Step 6: Add UserSettings dialog render**

Inside the `<>` fragment, after the `DeleteChannelModal` conditional render (after the closing `)}` around line 424), add:

```tsx
      <UserSettings
        username={currentUser}
        displayName={currentDisplayName}
        open={showUserSettings}
        onOpenChange={setShowUserSettings}
      />
```

- [ ] **Step 7: Run typecheck and frontend tests**

Run: `cd ui && npx tsc --noEmit && npm run test`
Expected: No errors, all tests pass.

- [ ] **Step 8: Commit**

```bash
git add ui/src/pages/Sidebar/Sidebar.tsx
git commit -m "feat(ui): wire sidebar cog to UserSettings dialog, show display names

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 7: Backend Tests

**Files:**
- Modify: `src/store/humans.rs`

- [ ] **Step 1: Add store tests for human display name**

Add a `#[cfg(test)]` module at the bottom of `src/store/humans.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> crate::store::Store {
        crate::store::Store::open(":memory:").unwrap()
    }

    #[test]
    fn create_human_has_no_display_name() {
        let store = test_store();
        store.create_human("alice").unwrap();
        let humans = store.get_humans().unwrap();
        assert_eq!(humans.len(), 1);
        assert_eq!(humans[0].name, "alice");
        assert!(humans[0].display_name.is_none());
    }

    #[test]
    fn update_display_name_roundtrip() {
        let store = test_store();
        store.create_human("bob").unwrap();

        let updated = store
            .update_human_display_name("bob", Some("Bob Builder"))
            .unwrap();
        assert_eq!(updated.display_name.as_deref(), Some("Bob Builder"));

        let humans = store.get_humans().unwrap();
        assert_eq!(humans[0].display_name.as_deref(), Some("Bob Builder"));
    }

    #[test]
    fn update_display_name_to_none_clears_it() {
        let store = test_store();
        store.create_human("carol").unwrap();
        store
            .update_human_display_name("carol", Some("Carol"))
            .unwrap();

        let cleared = store
            .update_human_display_name("carol", None)
            .unwrap();
        assert!(cleared.display_name.is_none());
    }

    #[test]
    fn update_unknown_human_returns_error() {
        let store = test_store();
        let result = store.update_human_display_name("ghost", Some("Boo"));
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("human not found"),
            "should mention 'human not found'"
        );
    }
}
```

- [ ] **Step 2: Run the new tests**

Run: `cargo test --lib humans`
Expected: All 4 tests pass.

- [ ] **Step 3: Run full test suite**

Run: `cargo test --lib`
Expected: All tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/store/humans.rs
git commit -m "test(store): add unit tests for human display_name operations

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

---

### Task 8: Final Validation

- [ ] **Step 1: Run full backend checks**

Run: `cargo clippy --all-targets -- -D warnings && cargo test --lib`
Expected: No warnings, all tests pass.

- [ ] **Step 2: Run full frontend checks**

Run: `cd ui && npx tsc --noEmit && npm run test`
Expected: No type errors, all tests pass.

- [ ] **Step 3: Verify schema file is still in sync**

The `schema.sql` file is the canonical schema for new databases. The migration handles existing databases. Optionally update `schema.sql` to include `display_name` in the humans table definition. In `src/store/schema.sql`, update lines 94-97:

```sql
CREATE TABLE IF NOT EXISTS humans (
    name TEXT PRIMARY KEY, -- Unique username
    display_name TEXT, -- Optional user-chosen display name
    created_at TEXT NOT NULL DEFAULT (datetime('now')) -- When the user was created
);
```

- [ ] **Step 4: Run tests one final time**

Run: `cargo test --lib && cd ui && npx tsc --noEmit && npm run test`
Expected: Everything green.

- [ ] **Step 5: Commit schema update**

```bash
git add src/store/schema.sql
git commit -m "chore: add display_name to humans schema definition

Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```
