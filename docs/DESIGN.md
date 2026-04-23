# Chorus Design Language

The visual system of Chorus, written down so the next contributor does not have
to reverse-engineer it from CSS. Every rule here cites its source.

This is a *descriptive* document: it codifies decisions already in the code.
When you change a token or break a rule, update this file in the same PR.

---

## Product context

- **What this is:** AI agent collaboration platform. Agents run as OS
  processes and communicate through a Slack-like chat interface.
- **Who it's for:** Developers and teams running multi-agent workflows.
- **Space:** Developer tools, agent orchestration, team collaboration.
- **Project type:** Web app (Rust/Axum backend, React/TypeScript frontend).

---

## Aesthetic direction

- **Direction:** Industrial / utilitarian. Function-first, data-dense, warm,
  and legible.
- **Decoration level:** Intentional, not expressive. Grid texture, dash-prefix
  kickers, and square avatars are enough.
- **Mood:** Engineering notebook. Warm but serious. Infrastructure you can
  read.
- **Signature:** Zero border radius and flat surfaces. This is one of the
  quickest ways Chorus reads as Chorus.

---

## Philosophy

**Paper and terminal.** A warm cream background with a faint graph-paper grid,
overlaid with a translucent white workspace. All corners are sharp (zero
border-radius). All text surfaces that carry meaning use a monospace font.
Every emphasis comes from weight, letter-spacing, and color inversion, never
from shadows or gradients.

The aesthetic target is a vintage IBM terminal printing onto an accountant's
ledger. Quiet by default, dense by design.

**What we avoid, on purpose:**

- Rounded corners
- Drop shadows and elevation
- Gradients and glows
- Bright saturated colors (the palette is warm, muted, and intentional)
- Emoji as decoration (except the 🎵 favicon)
- Dark mode (single cream theme)
- Material Design ripples, lift, or depth
- Stacked card grids as a layout primitive
- Generic SaaS hero sections

If a proposed change smells like any of those, stop and re-read this file.

---

## Source of truth

| Concern | File | Why |
|---|---|---|
| Tailwind `@theme` tokens | `ui/src/index.css` | Colors, radius, shadow, fonts — consumed by Tailwind utility classes and `var(--color-*)` references |
| CSS variables, global resets, global patterns | `ui/src/App.css` | Body background grid, status colors, kicker pattern, scrollbar, global focus |
| Component styles | `ui/src/components/**/<Component>.css` | Each component keeps its rules in a co-located file |
| Primitive components | `ui/src/components/ui/` | shadcn-adapted primitives (button, select, dialog, etc.) — styled to match the house vocabulary |

When you add a new token, put it in `index.css` if it's a Tailwind theme value,
`App.css` if it's a cross-cutting CSS variable, or a component CSS file if it's
local. Do not invent parallel scales.

---

## Color palette

Defined in `ui/src/index.css:3-33` inside the Tailwind `@theme` block. All
values are warm-leaning cream, near-black, and muted gray.

### Surface tokens

| Token | Value | Usage |
|---|---|---|
| `--color-background` | `#f4f2ed` | App background (under the grid) |
| `--color-foreground` | `#23201a` | Primary text |
| `--color-card` | `rgba(255, 255, 255, 0.82)` | Translucent card surface |
| `--color-card-foreground` | `#23201a` | Text on cards |
| `--color-popover` | `rgba(255, 255, 255, 0.94)` | Slightly more opaque for popovers |
| `--color-popover-foreground` | `#23201a` | Text on popovers |
| `--color-primary` | `#1f1f1c` | Solid fill for primary actions, button hover state |
| `--color-primary-foreground` | `#faf9f6` | Text on primary fills |
| `--color-secondary` | `#efede8` | Subtle fills (badges, pills, inline code) |
| `--color-secondary-foreground` | `#23201a` | Text on secondary |
| `--color-muted` | `#f0eee9` | Quietest fill (button bg, chip bg) |
| `--color-muted-foreground` | `#817b6f` | Quiet text (captions, timestamps, ambient notices, empty states) |
| `--color-accent` | `#f1efe9` | Subtle hover tint |
| `--color-accent-foreground` | `#5d574d` | Medium-emphasis text |
| `--color-destructive` | `#c67a18` | **Amber, not red.** Fatal errors and destructive actions (error-level toast border) |
| `--color-destructive-foreground` | `#faf9f6` | Text on destructive fills |
| `--color-warning` | `#d4a027` | **Golden amber.** Partial success / advisory notices (warning-level toast border). Must be visually distinct from `--color-destructive`. |
| `--color-border` | `rgba(35, 32, 26, 0.14)` | Default 1px borders |
| `--color-input` | `rgba(35, 32, 26, 0.28)` | Input borders, app-shell outline |
| `--color-ring` | `#1f1f1c` | Focus ring color |

### Status tokens

Defined in `ui/src/App.css:5-7` outside the Tailwind theme (no shadcn equivalent):

| Token | Value | Usage |
|---|---|---|
| `--status-online` | `#1f9d4d` | Agent online, status dot, presence |
| `--status-sleeping` | `#c67a18` | Agent thinking or working (same amber as destructive — reuse is intentional, both convey "active attention") |
| `--status-inactive` | `#b8b1a6` | Agent offline / asleep |
| `--status-failed` | `#c94040` | Agent crashed or in a failed state — rose/red to distinguish from the gray of inactive |

### Color rules

- **Never introduce a new color without adding a token.** Inline hex values are
  allowed only in decorative pseudo-elements for translucent overlays (e.g.
  `rgba(255, 255, 255, 0.14)` on nested button borders).
- **Destructive is amber, not red.** If you feel the urge to add red, you
  probably want destructive. If the warmth feels wrong, discuss before
  introducing a new token.
- **Text color defaults to `--color-foreground`.** Step down to
  `--color-accent-foreground` for medium emphasis, `--color-muted-foreground`
  for quiet/ambient text.

---

## Shape and elevation

Defined in `ui/src/index.css:24-32`:

```css
--radius: 0px;
--radius-lg: 0px;
--radius-md: 0px;
--radius-sm: 0px;
--shadow: none;
```

**Rules:**

- **Every component has `border-radius: 0`.** The only exception is
  `.status-dot` (`border-radius: 50%`) because a square presence indicator
  reads as broken.
- **No `box-shadow`, anywhere.** Multiple components set `box-shadow: none`
  explicitly to override browser defaults or shadcn defaults. Do not
  reintroduce elevation.
- **1px borders do the work shadows would do.** `var(--color-border)` for
  subtle separation, `var(--color-input)` for visible outlines, nested
  `::before { inset: 3px; border: 1px solid rgba(35,32,26,0.08) }` for
  decorative embossed buttons (see "Nested border buttons" below).

---

## Typography

Defined in `ui/src/index.css:29-30` and reinforced in `ui/src/App.css:1-12`.

```css
--font-sans: "Inter", system-ui, sans-serif;
--font-mono: "IBM Plex Mono", monospace;
--font-display: 'Inter', sans-serif;  /* alias of sans, for now */
```

Fonts are loaded from Google Fonts at the top of `App.css`. Weights in use:
Inter 400 / 500 / 600 / 700, IBM Plex Mono 400 / 500 / 600.

### When to use which

| Surface | Font | Notes |
|---|---|---|
| Page titles, chat header names, large display text | Inter 600, 24px, `letter-spacing: -0.01em` | The ONE place where tracking is negative, for display tightness |
| Body UI (buttons, form labels, headings inside panels) | Inter, 13-14px | Default sans for anything that's not "chat content" |
| **Chat message content** | **IBM Plex Mono 400, 14px, line-height 1.55** | This is the single most recognizable choice in the app. Chat body is mono. Do not change it without serious reason. |
| Captions, timestamps, descriptions, empty states | Mono, 11-13px, `--color-muted-foreground` | Captions are mono by default, not sans |
| Badges, kicker labels, button copy on buttons | Mono, 10-11px, uppercase, `letter-spacing: 0.04-0.07em` | See "Badge and label pattern" below |
| Code blocks and inline code | Mono, 12px, `var(--color-muted)` background | Already mono since the body is mono — code blocks just add a background |

### Typography rules

- **Never use sans for chat-adjacent surfaces.** Anything inside the message
  list, inbox, or thread panel is mono. The one exception is the display-size
  chat header name.
- **Captions are always muted AND mono.** Both at once. Pick one and you get a
  mismatch with the rest of the app.
- **Labels use uppercase mono with letter-spacing.** Badges, kickers, button
  copy on primary-action buttons, section headers inside panels — all follow
  this recipe.
- **Display text uses negative tracking.** Only at 24px+. Below that, use
  default or positive tracking.
- **Ambient system markers break the case and sans rules on purpose.**
  Task-event cards in the chat feed (`ui/src/components/chat/TaskEventMessage.*`)
  use *lowercase* mono for the task-number and status badges (letter-spacing
  0.07em) and *sans* for the 14px title. The card is a quiet state indicator,
  not a content label or chat message — shouting "IN PROGRESS" next to a
  pressable card reads like a warning. Mono + muted still applies to the
  meta row and the inline timeline. If you add a new ambient marker (not a
  content message, not a badge on a control), follow this pattern.

---

## Layout tokens

Defined in `ui/src/App.css:14-16`:

```css
--sidebar-width: 312px;
```

Other spacing is consistent but not tokenized yet:

| Measurement | Common values | Where |
|---|---|---|
| Horizontal list gutter | `18px` | `.message-list`, `.message-input-area` |
| Small gap | `6px` | Flex gaps between inline elements |
| Medium gap | `10px` | Flex gaps in dividers, avatar-to-body gap |
| Component padding (tight) | `8px 10px` | `.message-item`, `.chat-header` inner |
| Component padding (relaxed) | `12px 14px` | `.chat-header` outer, panel headers |
| Empty state padding | `40px` or `56px 20px` | `.empty-state`, `.message-list-empty`, `.chat-messages-empty` |

When you need a new spacing value, prefer one of the above before inventing a
new number.

### Panel widths and breakpoints

The layout is grid-disciplined: one fixed primary sidebar, fixed secondary
panels where needed, and a small set of repeated collapse points.

| Surface | Value | Source |
|---|---|---|
| Sidebar | `312px` | `ui/src/App.css` |
| Thread panel | `360px` | `ui/src/components/chat/ThreadPanel.css`, `ui/src/components/chat/ThreadsTab.css` |
| Members panel | `min(320px, calc(100% - 36px))` | `ui/src/components/channels/ChannelMembersPanel.css` |
| Workspace sidebar | `minmax(260px, 28%)` | `ui/src/components/agents/WorkspacePanel.css` |
| Compact breakpoint | `920px` | `ChatPanel.css`, `ChannelMembersPanel.css` |
| Medium breakpoint | `1100px` | `WorkspacePanel.css`, `ChannelMembersPanel.css` |
| Full threads breakpoint | `1120px` | `ThreadsTab.css` |

---

## Global patterns

These are not components but reusable visual motifs applied across many
components. Each has a single implementation to copy from.

### Graph-paper background

```css
/* ui/src/App.css:31-36 */
body {
  background-image:
    linear-gradient(rgba(49, 45, 37, 0.09) 1px, transparent 1px),
    linear-gradient(90deg, rgba(49, 45, 37, 0.09) 1px, transparent 1px);
  background-size: 240px 160px;
}
```

A faint 240×160 grid at 9% opacity on the body. The `.app-shell` sits on top
with `background: rgba(250, 249, 246, 0.9)` and a 6px margin, so the grid
peeks around the edges. This is the visual anchor of the whole paper aesthetic.

### Translucent app shell

```css
/* ui/src/App.css:38-47 */
.app-shell {
  margin: 6px;
  border: 1px solid var(--color-input);
  border-radius: 0;
  background: rgba(250, 249, 246, 0.9);
  box-shadow: none;
}
```

Do not replace this with opaque white. The translucency is what lets the grid
show through and gives the app its texture.

### Kicker labels (marker dash before label)

```css
/* ui/src/App.css:131-167 */
.*-kicker::before {
  content: '';
  width: 8px;
  height: 1px;
  background: currentColor;
  opacity: 0.55;
}
```

A huge number of small section headers use this pattern: an 8px × 1px
horizontal line before the label, same color as the text, 55% opacity. Creates
a typewritten "> " prompt feel without using a glyph.

Used by: `.sidebar-server-label`, `.sidebar-footer-meta`, `.chat-header-kicker`,
`.thread-kicker`, `.tasks-panel-kicker`, `.tasks-panel-channel`,
`.workspace-toolbar-kicker`, `.workspace-sidebar-kicker`,
`.workspace-preview-kicker`, `.profile-kicker`, `.profile-section-label`,
`.agent-config-section-kicker`, `.activity-title`. Follow the same recipe when
you add a new one.

### Badge and label pattern

```css
/* ui/src/App.css:74-86 */
.badge {
  display: inline-flex;
  align-items: center;
  min-height: 20px;
  padding: 0 7px;
  border-radius: 0;
  border: 1px solid var(--color-border);
  background: var(--color-muted);
  font-family: var(--font-mono);
  font-size: 10px;
  letter-spacing: 0.05em;
  text-transform: uppercase;
}
```

Variants live in `.agent-badge`, `.you-inline-badge`, `.deleted-inline-badge`
(all `ChatPanel.css:255-279`) at font-size 10-11px, same letter-spacing. Use
this recipe for any new inline badge.

### Horizontal-rule divider

```css
/* ui/src/components/chat/MessageList.css:17-36 */
.new-message-divider {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 2px;
}
.new-message-divider__line {
  flex: 1;
  height: 1px;
  background: var(--color-border);
}
.new-message-divider__label {
  font-size: 13px;
  font-family: var(--font-mono);
  color: var(--color-foreground);
  white-space: nowrap;
  letter-spacing: 0.02em;
}
```

The canonical divider for marking a boundary in a list. Centered mono label
between two flanking 1px rules. The quieter sibling `.system-message-divider`
(same file) uses the same structure with `--color-muted-foreground` and
rule opacity 0.6 so it reads as ambient. **Adopt this pattern for any new
in-list marker.**

### Nested border buttons (embossed)

```css
/* ui/src/components/chat/ChatPanel.css:81-103 */
.chat-header-btn {
  width: 30px;
  height: 30px;
  display: grid;
  place-items: center;
  border-radius: 0;
  background: var(--color-muted);
  border: 1px solid var(--color-border);
  color: var(--color-accent-foreground);
  position: relative;
}
.chat-header-btn::before {
  content: "";
  position: absolute;
  inset: 3px;
  border: 1px solid rgba(35, 32, 26, 0.08);
  pointer-events: none;
}
```

The outer 1px border plus a second inner 1px border at `inset: 3px` with 8%
opacity creates a subtle embossed "typewriter key" effect. Also used by
`.chat-header-member-btn`, `.message-input-send`, and others. Use this when
you want a button to feel *pressable* without a shadow.

### Color-hashed avatars

Message avatars are 40×40 squares with the background color derived from a
hash of the sender name (see `senderColor()` in `MessageItem.tsx:115-129`).
Seven preset colors rotate deterministically. Text is white, font-size 12px,
bold. **Square, not round, not rounded.**

### Empty state

```css
/* ui/src/App.css:169-190 */
.empty-state {
  color: var(--color-muted-foreground);
  font-family: var(--font-mono);
  font-size: 13px;
  padding: 40px;
  text-align: center;
}
```

Always mono, always muted, always centered. Repeated as `.message-list-empty`
and `.chat-messages-empty` with the same values.

### Scrollbar

```css
/* ui/src/App.css:192-198 */
::-webkit-scrollbar { width: 8px; height: 8px; }
::-webkit-scrollbar-thumb {
  background: rgba(35, 32, 26, 0.16);
  border-radius: 0;
}
```

8px, zero radius, translucent dark thumb. Do not override per-component.

---

## Interaction states

### Hover: color inversion

The house hover recipe is *invert to primary*:

```css
background: var(--color-primary);   /* #1f1f1c */
border-color: var(--color-primary);
color: #f8f6f1;
```

Used by: `.chat-header-btn`, `.chat-header-member-btn`, `.mention-pill-clickable`,
`.message-input-btn`, `.new-message-badge`, `.message-action-btn`. When you add a
new interactive element, this is the default hover treatment.

### Large-surface hover: accent tint, not primary-invert

Large clickable surfaces — whole card rows, feed items — use a quieter hover
(`background: var(--color-accent)`) instead of the full primary-invert.
Color-inversion at card scale is visually loud next to the feed's natural
hover rhythm and competes with the content. Reserve primary-invert for
buttons, pills, and small controls.

Used by: `.message-item` (via translucent white, historical), `.task-event-card`,
`.task-event-done-row`. When you add a new card-scale click target, follow
this pattern, not the primary-invert above.

### Active / selected

Same as hover — primary fill, white text. There is no separate "active" style.

### Hover transition

```css
transition:
  background 0.15s ease,
  color 0.15s ease,
  border-color 0.15s ease;
```

120-150ms ease on the three color properties. Never on `transform`, `scale`,
or `box-shadow`. Hover changes color, nothing else.

### Focus

```css
/* ui/src/App.css:65-72 */
button:focus-visible,
input:focus-visible,
textarea:focus-visible,
select:focus-visible {
  outline: 2px solid var(--color-ring);
  outline-offset: 2px;
}
```

A single 2px solid outline at `--color-ring` (the primary near-black) with 2px
offset. Applied globally via `focus-visible` so it only appears for keyboard
users. Do not override per-component.

### Disabled

```css
opacity: 0.45;
cursor: default;
```

See `.message-input-send:disabled` (`ChatPanel.css:566`). Keep the same values
for consistency.

---

## Motion

Chorus has **two** animation keyframes. That is intentional. Do not add a third
without a real reason.

### `pulse-dot`

```css
/* ui/src/App.css:114-117 */
@keyframes pulse-dot {
  0%, 100% { opacity: 1; transform: scale(1); }
  50% { opacity: 0.5; transform: scale(0.82); }
}
```

1s ease-in-out infinite. Applied to `.status-dot.thinking` and
`.status-dot.working` to signal an agent is actively processing.

### `form-control-spin` / `message-status-spin`

```css
@keyframes form-control-spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}
```

0.9s linear infinite. Applied to loading spinners. Two near-identical copies
exist (`App.css:220-223` and `ChatPanel.css:245-253`). They should be
consolidated but have not been.

### Motion rules

- **Color transitions only *on hover*.** 120-150ms ease on background, color,
  border. Never on `transform`, `scale`, or `box-shadow`.
- **Never scale on hover.** Never translate. Never rotate on hover.
- **Loaders spin linearly, statuses pulse in ease-in-out.** Keep it that way.
- **No spring physics, no overshoot, no stagger.**
- **`prefers-reduced-motion` is honored.** Any transition added must drop to
  `transition: none !important` inside `@media (prefers-reduced-motion: reduce)`.

### State-machine transitions

The "color transitions only" rule above governs **hover**. State-machine
components — where a data attribute flips an element between distinct,
named states — may also transition the properties listed below. These are
*not* hover animations; they express a persistent data change.

Allowed properties:

- `max-height` + `opacity` for reveal / collapse of stacked views (e.g. the
  `.task-event-card-view` ↔ `.task-event-pill-view` swap when a task reaches
  `done`). 200-260ms on `cubic-bezier(0.2, 0.8, 0.2, 1)`.
- `transform: translateX` (small, under 8px) + `opacity` for list-item entry
  into a live feed (`.task-event-ev.is-show`). 180ms.
- `border-left-color` + `color` for status-pill color changes
  (`.task-event-status[data-status=...]`). 220ms on the same ease curve.

See `ui/src/components/chat/TaskEventMessage.css` for the canonical recipe.
Reach for this *only* when the element is driven by a data-attribute state
flip. If the trigger is the user's pointer, re-read the motion rules above.

---

## Component families

High-level summary of how the current components group together. Each family
shares a visual vocabulary; new additions should match.

| Family | Examples | Shared vocabulary |
|---|---|---|
| **Chrome** | `.app-shell`, `.chat-header`, side panels | Translucent white background, 1px border, zero radius, no shadow |
| **Lists** | `.message-list`, inbox, threads tab | Mono content, 18px horizontal gutter, hover row highlight |
| **Inputs** | `.message-input-row`, `.chat-header-btn`, form controls | Nested border embossed, mono caption, hover inverts to primary |
| **Badges and pills** | `.badge`, `.agent-badge`, `.you-inline-badge`, `.mention-pill`, `.channel-pill` | Mono 10-12px, uppercase where a label, zero radius, secondary or muted background |
| **Dividers** | `.new-message-divider`, `.system-message-divider` | Flex + flanking 1px rules + centered mono label |
| **Kickers** | `.*-kicker`, `.*-section-label` | Inline-flex with 8×1px `::before` dash marker |
| **Empty states** | `.empty-state`, `.message-list-empty`, `.chat-messages-empty` | Mono 13px muted, 40-56px padding, centered |
| **Status indicators** | `.status-dot` | The ONE place with `border-radius: 50%` |

---

## Accessibility baseline

- **Focus ring is always visible for keyboard users** (via `:focus-visible`).
  Do not add `outline: none` without a replacement.
- **Interactive elements are buttons or links.** Do not put click handlers on
  plain divs. Cursor-interactive divs without a role are already a problem
  flagged in several `snapshot -i` outputs.
- **System-authored notices use `role="status"` + `aria-live="polite"`.** See
  `SystemMessageItem` in `ui/src/components/chat/MessageItem.tsx` for the
  reference implementation.
- **Color is never the sole signal.** Status dots are accompanied by text
  labels ("online", "thinking"). Destructive actions include text, not just
  an amber color.

---

## Decision log

This section preserves the design decisions from the former top-level
`DESIGN.md`. When a historical note and the live CSS disagree, the CSS-backed
sections above are authoritative.

| Date | Decision | Status | Rationale |
|---|---|---|---|
| 2026-04-11 | Initial design system created | Landed | Codified the existing visual language from a CSS audit instead of inventing a new aesthetic. |
| 2026-04-11 | Keep Inter + IBM Plex Mono | Landed | Strong pairing already in use. Sans for framing, mono for chat and system surfaces. |
| 2026-04-11 | Keep 0px border radius | Landed | Signature element. Makes Chorus visually distinct immediately. |
| 2026-04-11 | Consolidate the type scale into a small set of recurring sizes | Directional | Reduce one-off font sizes and keep hierarchy legible. |
| 2026-04-11 | Prefer 4px-derived spacing values | Directional | Normalize 7px/14px drift over time rather than inventing a parallel spacing system. |
| 2026-04-11 | Tokenize reusable status and semantic colors | Partial | Shared colors belong in theme variables; some legacy usages still need cleanup. |
| 2026-04-11 | Reduce animation duration drift | Partial | `pulse-dot` is standardized at 1s, but spinner timings are not fully consolidated yet. |

---

## When you change the design

1. **Update this file in the same PR.** If you add a token, document it here.
  If you break a rule, explain why here. A stale `docs/DESIGN.md` is worse
  than none.
2. **Add a token before a second use.** The first use can be inline; the
   second use means it should live in `index.css` or `App.css`.
3. **Match an existing component family.** New components should look like
   they belong. Reach for the badge recipe, the divider recipe, the kicker
   recipe before inventing.
4. **Run `/gstack-plan-design-review` on non-trivial visual changes.** It
   reads this file as the source of truth for design-system alignment
   (pass 5).

---

## Code organization rules (UI)

These are enforcement rules about where things go, not about how they look.

- Component styles in co-located `.css` files (not a shared stylesheet)
- Design tokens live in `ui/src/index.css` (`@theme`) and `ui/src/App.css` (cross-cutting vars)
- Icons: `lucide-react` (13px inline, 16px panel)
- No global state mutations outside `ui/src/store/`
- API calls through `ui/src/api/`
- Do not introduce a second visual style for shared dialogs, forms, or selects
- Do not separate labels from their focusable controls
- Do not use the browser viewport for read visibility
