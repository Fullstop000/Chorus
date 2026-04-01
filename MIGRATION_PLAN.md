# shadcn/ui Full Migration Plan

## Overview

Migrate all UI components from custom CSS classes to shadcn/ui components with brutalist theme.

**Estimated Time:** 90-120 minutes  
**Risk Level:** Medium (affects all UI components)  
**Rollback Strategy:** Git worktree can be discarded

---

## PHASE 0: Setup & Safety (5 min)

### 0.1 Verify Current State
```bash
cd /Users/bytedance/slock-daemon/Chorus/.worktrees/ui-shadcn-adaptor-frame/ui
npm run build
```
**Success Criteria:** Build passes with no errors

### 0.2 Add Required shadcn Components
```bash
npx shadcn@latest add label textarea -y
```

### 0.3 Create Migration Helpers
Create `src/components/ui/form.tsx` for consistent form layouts:
- FormLabel
- FormField
- FormDescription
- FormMessage (for errors)

---

## PHASE 1: Migrate Buttons (15 min)

### 1.1 Audit Current Button Usage
Files with `btn-brutal` or similar:
- `CreateAgentModal.tsx` (2 buttons)
- `EditChannelModal.tsx` (5 buttons)
- `AgentConfigForm.tsx` (1 button)
- `ChatPanel.tsx` (multiple)
- `MessageInput.tsx` (1 button)
- `Sidebar.tsx` (multiple)
- `ProfilePanel.tsx` (multiple)
- `TasksPanel.tsx` (multiple)
- `TeamSettings.tsx` (multiple)
- `WorkspacePanel.tsx` (multiple)
- `ChannelMembersPanel.tsx` (multiple)

### 1.2 Migration Pattern

**Before:**
```tsx
<button className="btn-brutal" onClick={onClose}>Cancel</button>
<button className="btn-brutal btn-cyan" onClick={handleCreate} disabled={creating}>
  {creating ? 'Creating...' : 'Create'}
</button>
<button className="btn-brutal btn-orange" onClick={handleDelete}>
  Delete
</button>
```

**After:**
```tsx
import { Button } from '@/components/ui/button'

<Button variant="outline" onClick={onClose}>Cancel</Button>
<Button onClick={handleCreate} disabled={creating}>
  {creating ? 'Creating...' : 'Create'}
</Button>
<Button variant="destructive" onClick={handleDelete}>
  Delete
</Button>
```

### 1.3 Button Variant Mapping
| Custom Class | shadcn Variant |
|--------------|----------------|
| `btn-brutal` | default |
| `btn-brutal btn-cyan` | default (primary action) |
| `btn-brutal btn-pink` | default (accent) |
| `btn-brutal btn-yellow` | secondary |
| `btn-brutal btn-orange` | destructive |
| `btn-brutal btn-lime` | secondary |
| `btn-brutal-sm` | size="sm" |

---

## PHASE 2: Migrate Inputs (15 min)

### 2.1 Audit Current Input Usage
Files with `form-input`, `form-textarea`:
- `EditChannelModal.tsx` (input + textarea)
- `CreateAgentModal.tsx` (via AgentConfigForm)
- `AgentConfigForm.tsx` (multiple inputs)
- `TeamSettings.tsx` (inputs)
- `ProfilePanel.tsx` (inputs)

### 2.2 Migration Pattern

**Before:**
```tsx
<div className="form-group">
  <label className="form-label" htmlFor="name">Name</label>
  <input
    id="name"
    className="form-input"
    value={name}
    onChange={(e) => setName(e.target.value)}
  />
</div>
```

**After:**
```tsx
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'

<div className="space-y-2">
  <Label htmlFor="name">Name</Label>
  <Input
    id="name"
    value={name}
    onChange={(e) => setName(e.target.value)}
  />
</div>
```

### 2.3 Textarea Migration
Same pattern, use `Textarea` component from shadcn.

---

## PHASE 3: Migrate Modals to Dialog (20 min)

### 3.1 Audit Current Modal Usage
Files with `modal-overlay`, `modal-box`:
- `CreateAgentModal.tsx` - Custom div-based modal
- `CreateChannelModal.tsx` - Likely custom
- `ProfilePanel.tsx` - May have modals

### 3.2 Migration Pattern

**Before:**
```tsx
<div className="modal-overlay" onClick={(e) => e.target === e.currentTarget && onClose()}>
  <div className="modal-box modal-box-agent">
    <div className="modal-header">
      <div className="modal-title-block">
        <span className="modal-title">Create Agent</span>
        <span className="modal-subtitle">[agent::new]</span>
      </div>
      <button className="modal-close" onClick={onClose}>×</button>
    </div>
    ...
    <div className="modal-footer">
      <button className="btn-brutal" onClick={onClose}>Cancel</button>
      <button className="btn-brutal btn-cyan" onClick={handleCreate}>Create</button>
    </div>
  </div>
</div>
```

**After:**
```tsx
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'

<Dialog open={open} onOpenChange={onOpenChange}>
  <DialogContent className="sm:max-w-[720px]">
    <DialogHeader>
      <DialogTitle>Create Agent</DialogTitle>
      <DialogDescription>[agent::new]</DialogDescription>
    </DialogHeader>
    ...
    <DialogFooter>
      <Button variant="outline" onClick={onClose}>Cancel</Button>
      <Button onClick={handleCreate}>Create</Button>
    </DialogFooter>
  </DialogContent>
</Dialog>
```

### 3.3 Modal Size Mapping
| Custom Class | Dialog Class |
|--------------|--------------|
| `modal-box` | default |
| `modal-box-agent` | `sm:max-w-[720px]` |

---

## PHASE 4: Migrate Selects to AsyncSelect (15 min)

### 4.1 Audit Current Select Usage
Files using Select from `@/components/ui/select`:
- `CreateAgentModal.tsx` - Machine select
- `AgentConfigForm.tsx` - Runtime, Model, Reasoning selects

### 4.2 Migration Pattern

**Before:**
```tsx
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'

<Select value={runtime} onValueChange={setRuntime}>
  <SelectTrigger className="form-select" aria-label="Runtime">
    <SelectValue />
  </SelectTrigger>
  <SelectContent>
    <SelectItem value="claude">{runtimeOptionLabel('claude', runtimeStatuses)}</SelectItem>
    ...
  </SelectContent>
</Select>
{runtimeStatusError && <div className="modal-field-hint">{runtimeStatusError}</div>}
```

**After:**
```tsx
import { RuntimeSelect } from '@/components/ui/async-select'

<RuntimeSelect
  value={runtime}
  onValueChange={(rt) => setRuntime(rt)}
  runtimes={runtimeStatuses}
  isLoading={isLoadingRuntimes}
  error={runtimeStatusError}
/>
```

### 4.3 For Non-Runtime Selects
Use `AsyncSelect` with `options` prop.

---

## PHASE 5: Add Remaining shadcn Components (10 min)

### 5.1 Add Checkbox (for settings/toggles)
```bash
npx shadcn@latest add checkbox
```

### 5.2 Add Badge (for status indicators)
```bash
npx shadcn@latest add badge
```

### 5.3 Add Separator (for dividers)
```bash
npx shadcn@latest add separator
```

### 5.4 Update Components Using Custom Badges
Replace custom `.badge` classes with shadcn `Badge` component.

---

## PHASE 6: Cleanup & Verify (20 min)

### 6.1 Remove Unused CSS
From `App.css`, remove classes that are no longer used:
- `.btn-brutal` (and variants)
- `.btn-brutal-sm`
- `.form-input`, `.form-textarea`, `.form-select`
- `.form-label`
- `.form-group`
- `.modal-overlay`, `.modal-box`, `.modal-card`
- `.modal-header`, `.modal-footer`
- `.modal-title`, `.modal-subtitle`
- `.modal-close`
- `.error-banner` (may keep but restyle)

**KEEP these (component-specific styles):**
- `.mention-pill`, `.channel-pill`
- `.status-dot`
- `.activity-*` classes
- `.message-*` classes
- `.sidebar-*` classes (except button-related)
- `.chat-*` classes
- `.workspace-*` classes
- `.tasks-*` classes

### 6.2 Build Verification
```bash
npm run build
```
Fix any TypeScript errors.

### 6.3 Visual Regression Check
Key flows to verify:
1. Create Agent modal (runtime select, buttons)
2. Edit Channel modal (inputs, buttons, dialog)
3. Send message (input, button)
4. Profile panel (forms)
5. Settings panels

---

## Component Migration Order

**Priority 1 (Core Modals):**
1. `CreateAgentModal.tsx`
2. `EditChannelModal.tsx`
3. `CreateChannelModal.tsx`

**Priority 2 (Forms):**
4. `AgentConfigForm.tsx`
5. `TeamSettings.tsx`
6. `ProfilePanel.tsx`

**Priority 3 (Chat UI):**
7. `MessageInput.tsx`
8. `ChatPanel.tsx`
9. `MentionTextarea.tsx`

**Priority 4 (Sidebar & Panels):**
10. `Sidebar.tsx`
11. `TasksPanel.tsx`
12. `WorkspacePanel.tsx`
13. `ChannelMembersPanel.tsx`

**Priority 5 (Remaining):**
14. Other panels and prototypes

---

## Testing Checklist

- [ ] All buttons have proper focus states
- [ ] All inputs have proper focus states
- [ ] Modals close on backdrop click
- [ ] Modals close on Escape key
- [ ] Form validation errors display correctly
- [ ] Loading states work (buttons disabled, spinners)
- [ ] Select dropdowns open/close correctly
- [ ] No visual regressions in layout
- [ ] Dark mode still works (if applicable)
- [ ] Build passes

---

## Rollback Plan

If issues arise:
1. Stop migration
2. Restore from git: `git checkout -- ui/src/components/`
3. Re-apply only shadcn components: `npx shadcn@latest add [components]`
4. Rebuild

---

## Post-Migration

### Benefits Achieved
- Consistent component API across codebase
- Full accessibility (keyboard nav, ARIA, focus management)
- Loading states built-in
- Type-safe props
- Easy to add new shadcn components

### Next Steps
- Add Storybook for component documentation
- Consider adding more shadcn components (tooltip, dropdown-menu, etc.)
- Standardize on form validation library (react-hook-form + zod)
