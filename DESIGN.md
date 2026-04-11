# Design System — Chorus

## Product Context
- **What this is:** AI agent collaboration platform. Agents run as OS processes and communicate through a Slack-like chat interface.
- **Who it's for:** Developers and teams running multi-agent workflows
- **Space/industry:** Dev tools, agent orchestration, team collaboration
- **Project type:** Web app (Rust/Axum backend, React/TypeScript frontend)

## Aesthetic Direction
- **Direction:** Industrial/Utilitarian — function-first, data-dense, monospace accents, warm tones
- **Decoration level:** Intentional — grid-line body texture, dash-prefix kickers, colored letter avatars. Not minimal, not expressive.
- **Mood:** Engineering notebook. Warm but serious. Infrastructure you can read.
- **Signature:** Zero border radius. Everything is flat. This makes Chorus recognizable.

## Typography
- **Display/Hero:** Inter 700 — clean geometric sans for headings. Not flashy, just clear.
- **Body:** Inter 400/500 — readable at 14px, good for long message threads
- **UI/Labels:** IBM Plex Mono 500 — kickers, section labels, badges. The voice of the infrastructure.
- **Data/Tables:** IBM Plex Mono 400 (tabular-nums) — UUIDs, timestamps, trace data
- **Code:** IBM Plex Mono 400
- **Loading:** Google Fonts `Inter:wght@400;500;600;700` + `IBM+Plex+Mono:wght@400;500;600`
- **Scale:**

| Token | Size | Role |
|-------|------|------|
| `--text-2xs` | 10px | Badges, timestamps, micro labels |
| `--text-xs` | 11px | Kickers, section labels |
| `--text-sm` | 12px | Telescope traces, monospace body, pills |
| `--text-base` | 13px | Empty states, activity log |
| `--text-md` | 14px | Message body, sidebar items, default body |
| `--text-lg` | 18px | Section titles |
| `--text-xl` | 20px | Panel titles |
| `--text-2xl` | 24px | Main headers (#channel) |

- **Line heights:** 1.0 (headings), 1.4 (lists), 1.5 (mono body), 1.55 (body text)
- **Letter spacing:** -0.01em (headings), 0.02em (body mono), 0.05em (uppercase badges), 0.07em (kickers)

## Color
- **Approach:** Restrained — color is rare and meaningful. The parchment warmth is the palette.

### Core
| Token | Hex | Usage |
|-------|-----|-------|
| `--color-background` | `#f4f2ed` | Page background |
| `--color-foreground` | `#23201a` | Primary text |
| `--color-card` | `rgba(255,255,255,0.82)` | Card/panel surfaces |
| `--color-popover` | `rgba(255,255,255,0.94)` | Popover/dropdown surfaces |
| `--color-primary` | `#1f1f1c` | CTA buttons, active states |
| `--color-primary-foreground` | `#faf9f6` | Text on primary |
| `--color-secondary` | `#efede8` | Active sidebar item, pills |
| `--color-muted` | `#f0eee9` | Badge backgrounds, disabled |
| `--color-muted-foreground` | `#817b6f` | Secondary text, metadata |
| `--color-accent` | `#f1efe9` | Hover backgrounds |
| `--color-accent-foreground` | `#5d574d` | Descriptions, tertiary text |
| `--color-destructive` | `#c67a18` | Amber — warnings, destructive actions |
| `--color-border` | `rgba(35,32,26,0.14)` | Borders |
| `--color-input` | `rgba(35,32,26,0.28)` | Active input borders, app shell border |
| `--color-ring` | `#1f1f1c` | Focus ring |

### Status (must be CSS vars, not hardcoded)
| Token | Hex | Usage |
|-------|-----|-------|
| `--status-online` | `#1f9d4d` | Agent online, task done |
| `--status-thinking` | `#c67a18` | Agent thinking/working |
| `--status-inactive` | `#b8b1a6` | Agent offline |
| `--status-sent` | `#355d8a` | Message sent, in progress |
| `--status-error` | `#a84738` | Error states |
| `--status-unread` | `#8a3d0c` | Unread badge |

### Semantic (activity log, darker variants)
| Token | Hex | Usage |
|-------|-----|-------|
| `--activity-received` | `#2f7b44` | Message received |
| `--activity-thinking` | `#9f6a2b` | Thinking indicator |
| `--activity-raw` | `#6d5f48` | Raw output |

### Avatar
| Token | Hex | Usage |
|-------|-----|-------|
| `--avatar-human` | `#7c684f` | Human user avatar bg |
| `--avatar-agent` | `#1f1f1c` | Default agent avatar bg |

- **Dark mode:** Invert surfaces (bg → `#1a1816`), reduce saturation 10-20%, shift grays warm. Keep status colors. Grid texture opacity drops to 0.04.

## Spacing
- **Base unit:** 4px
- **Density:** Comfortable — not cramped, not spacious

| Token | Value | Usage |
|-------|-------|-------|
| `--space-2xs` | 2px | Tight internal gaps |
| `--space-xs` | 4px | Badge padding, inline gaps |
| `--space-sm` | 8px | List item padding, small gaps |
| `--space-md` | 12px | Panel padding, section gaps |
| `--space-lg` | 16px | Major padding, header spacing |
| `--space-xl` | 24px | Section separation |
| `--space-2xl` | 32px | Large panel padding |
| `--space-3xl` | 48px | Page-level separation |
| `--space-4xl` | 64px | Maximum breathing room |

## Layout
- **Approach:** Grid-disciplined — fixed sidebar, flex panels, predictable structure
- **Sidebar:** 312px fixed
- **Thread panel:** 360px fixed
- **Members panel:** min(320px, calc(100% - 36px))
- **Workspace sidebar:** minmax(260px, 28%)
- **Breakpoints:** 920px (compact), 1100px (medium), 1120px (full)
- **Border radius:** 0px everywhere. Exception: status dots (50% — they're circles).
- **Shadows:** None. Exception: sidebar channel context menu (`0 12px 28px rgba(24,20,12,0.16)`).
- **Body texture:** Grid lines at 240×160px, rgba(49,45,37,0.09)

## Motion
- **Approach:** Minimal-functional — motion serves comprehension, not decoration
- **Easing:** enter(ease-out), exit(ease-in), move(ease-in-out), spinner(linear)
- **Duration:**

| Token | Value | Usage |
|-------|-------|-------|
| `--duration-micro` | 80ms | Instant feedback (active states) |
| `--duration-short` | 150ms | Hover, focus, button transitions |
| `--duration-medium` | 300ms | Panel expansion, state changes |
| `--duration-long` | 500ms | Content reveal |

- **Keyframes:**
  - `pulse-dot` — 1s ease-in-out infinite (status dots)
  - `spin` — 1s linear infinite (spinners)

## Component Patterns

### Kicker (section label)
```css
font-family: var(--font-mono);
font-size: var(--text-xs);  /* 11px */
font-weight: 500;
letter-spacing: 0.07em;
text-transform: uppercase;
color: var(--color-muted-foreground);
```
With `::before` dash decorator: `width: 8px; height: 1px; opacity: 0.55`

### Badge
```css
min-height: 20px;
padding: 0 8px;               /* was 7px, standardized */
border: 1px solid var(--color-border);
background: var(--color-muted);
font-family: var(--font-mono);
font-size: var(--text-2xs);   /* 10px */
letter-spacing: 0.05em;
text-transform: uppercase;
```

### Button (default)
```css
min-height: 28px;
padding: 0 var(--space-md);   /* 12px */
border: 1px solid var(--color-border);
font-family: var(--font-mono);
font-size: var(--text-sm);    /* 12px */
```
Hover: `background: var(--primary); color: var(--primary-foreground);`

### Empty State
```css
font-family: var(--font-mono);
font-size: var(--text-base);  /* 13px */
color: var(--color-muted-foreground);
padding: var(--space-3xl);    /* 48px */
text-align: center;
```

## Decisions Log
| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-04-11 | Initial design system created | Codified existing visual language from CSS audit. 18 CSS files analyzed. |
| 2026-04-11 | Keep Inter + IBM Plex Mono | Strong pairing already in use. No reason to change. |
| 2026-04-11 | Keep 0px border radius | Signature element. Makes Chorus visually distinct. |
| 2026-04-11 | Consolidate type scale 12→8 | Removed 15px, 28px. Every size now has a named token and clear purpose. |
| 2026-04-11 | Standardize spacing to 4px multiples | Replace 7px→8px, 14px→12px/16px. Minor corrections. |
| 2026-04-11 | Tokenize status/semantic colors | 8+ hardcoded hex values moved to CSS vars. Enables dark mode, prevents drift. |
| 2026-04-11 | Standardize animation durations | 0.9s/1.2s→1s. Add micro/short/medium/long scale. |
