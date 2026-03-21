# Implementation Plan: React + TypeScript Frontend for Chorus

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a React + TypeScript single-page application that provides a Discord-style UI for the Chorus local daemon. The frontend lives in `ui/` and is served as static files by the existing Rust/axum server in production.

**Architecture:** Vite + React 18 + TypeScript. Global state via React Context (no Redux). Polling-based data fetching (no WebSocket needed — the backend already has long-poll on `/receive` but the UI will use short-interval polling for simplicity). In development, Vite proxies API calls to `localhost:3001`. In production, `cargo build` + `npm run build` then `chorus serve` serves everything on one port.

**Tech Stack:** Vite 5, React 18, TypeScript 5, plain CSS (CSS variables, no CSS-in-JS), no UI library (pixel-perfect custom components matching the design references).

---

## File Structure

```
ui/
├── index.html
├── vite.config.ts
├── tsconfig.json
├── package.json
├── src/
│   ├── main.tsx
│   ├── App.tsx
│   ├── App.css
│   ├── api.ts
│   ├── types.ts
│   ├── store.ts
│   ├── hooks/
│   │   ├── useServerInfo.ts
│   │   ├── useHistory.ts
│   │   └── useTasks.ts
│   └── components/
│       ├── Sidebar.tsx
│       ├── Sidebar.css
│       ├── ChatPanel.tsx
│       ├── ChatPanel.css
│       ├── MessageItem.tsx
│       ├── MessageInput.tsx
│       ├── TasksPanel.tsx
│       ├── TasksPanel.css
│       ├── ProfilePanel.tsx
│       ├── ProfilePanel.css
│       ├── TabBar.tsx
│       └── CreateAgentModal.tsx

src/server.rs   (modified — add whoami, static serving, CORS)
Cargo.toml      (modified — add tower-http)
```

---

## Task 1: Scaffold Vite + React + TypeScript project (`ui/`)

**Files:** `ui/package.json`, `ui/vite.config.ts`, `ui/tsconfig.json`, `ui/index.html`, `ui/src/main.tsx`

**Commit message:** `feat(ui): scaffold vite + react + typescript project`

- [ ] **Step 1: Create `ui/package.json`**

```json
{
  "name": "chorus-ui",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc -b && vite build",
    "preview": "vite preview"
  },
  "dependencies": {
    "react": "^18.3.1",
    "react-dom": "^18.3.1"
  },
  "devDependencies": {
    "@types/react": "^18.3.5",
    "@types/react-dom": "^18.3.0",
    "@vitejs/plugin-react": "^4.3.1",
    "typescript": "^5.5.3",
    "vite": "^5.4.2"
  }
}
```

- [ ] **Step 2: Create `ui/vite.config.ts`**

```typescript
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/internal': 'http://localhost:3001',
      '/api': 'http://localhost:3001',
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
})
```

- [ ] **Step 3: Create `ui/tsconfig.json`**

```json
{
  "compilerOptions": {
    "target": "ES2020",
    "useDefineForClassFields": true,
    "lib": ["ES2020", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "skipLibCheck": true,
    "moduleResolution": "bundler",
    "allowImportingTsExtensions": true,
    "isolatedModules": true,
    "moduleDetection": "force",
    "noEmit": true,
    "jsx": "react-jsx",
    "strict": true,
    "noUnusedLocals": true,
    "noUnusedParameters": true,
    "noFallthroughCasesInSwitch": true
  },
  "include": ["src"]
}
```

- [ ] **Step 4: Create `ui/index.html`**

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Chorus</title>
    <link rel="icon" type="image/svg+xml" href="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><text y='.9em' font-size='90'>🎵</text></svg>" />
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

- [ ] **Step 5: Create `ui/src/main.tsx`**

```tsx
import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'
import './App.css'

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
)
```

- [ ] **Step 6: Install dependencies**

```bash
cd /Users/bytedance/slock-daemon/Chorus/ui
npm install
```

---

## Task 2: Types + API client (`types.ts`, `api.ts`)

**Files:** `ui/src/types.ts`, `ui/src/api.ts`

**Commit message:** `feat(ui): add typed api client and typescript interfaces`

- [ ] **Step 1: Create `ui/src/types.ts`**

This file mirrors the Rust models from `src/models.rs` and the API response shapes from `src/server.rs`.

```typescript
// ── Server Info ──

export interface ChannelInfo {
  id?: string
  name: string
  description?: string
  joined: boolean
}

export interface AgentInfo {
  id?: string
  name: string
  display_name?: string
  status: 'active' | 'sleeping' | 'inactive'
  runtime?: string
  model?: string
  description?: string
  session_id?: string
}

export interface HumanInfo {
  name: string
}

export interface ServerInfo {
  channels: ChannelInfo[]
  agents: AgentInfo[]
  humans: HumanInfo[]
}

// ── Messages ──

export interface AttachmentRef {
  id: string
  filename: string
}

export interface HistoryMessage {
  id: string
  seq: number
  content: string
  senderName: string
  senderType: 'human' | 'agent'
  createdAt: string
  thread_parent_id?: string
  attachments?: AttachmentRef[]
}

export interface HistoryResponse {
  messages: HistoryMessage[]
  has_more: boolean
  last_read_seq: number
}

// ── Tasks ──

export type TaskStatus = 'todo' | 'in_progress' | 'in_review' | 'done'

export interface TaskInfo {
  id?: string
  taskNumber: number
  title: string
  status: TaskStatus
  channelId?: string
  claimedByName?: string
  createdByName?: string
  createdAt?: string
}

export interface TasksResponse {
  tasks: TaskInfo[]
}

// ── Upload ──

export interface UploadResponse {
  id: string
  filename: string
  sizeBytes: number
}

// ── Resolve Channel ──

export interface ResolveChannelResponse {
  channelId: string
  channelName?: string
}

// ── Whoami ──

export interface WhoamiResponse {
  username: string
}

// ── App-level target union ──

// A "target" is the encoded channel/DM string passed to send/history
// e.g. "#general" or "dm:@alice"
export type Target = string
```

- [ ] **Step 2: Create `ui/src/api.ts`**

```typescript
import type {
  ServerInfo,
  HistoryResponse,
  TasksResponse,
  TaskStatus,
  UploadResponse,
  ResolveChannelResponse,
  WhoamiResponse,
} from './types'

const BASE = ''  // same origin in prod; Vite proxy in dev

async function json<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText }))
    throw new Error((err as { error?: string }).error ?? res.statusText)
  }
  return res.json() as Promise<T>
}

export async function getWhoami(): Promise<WhoamiResponse> {
  return json(await fetch(`${BASE}/api/whoami`))
}

export async function getServerInfo(username: string): Promise<ServerInfo> {
  return json(await fetch(`${BASE}/internal/agent/${encodeURIComponent(username)}/server`))
}

export async function sendMessage(
  username: string,
  target: string,
  content: string,
  attachmentIds?: string[]
): Promise<{ messageId: string }> {
  return json(
    await fetch(`${BASE}/internal/agent/${encodeURIComponent(username)}/send`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ target, content, attachmentIds: attachmentIds ?? [] }),
    })
  )
}

export async function getHistory(
  username: string,
  channel: string,
  limit = 50,
  before?: number,
  after?: number
): Promise<HistoryResponse> {
  const params = new URLSearchParams({ channel, limit: String(limit) })
  if (before != null) params.set('before', String(before))
  if (after != null) params.set('after', String(after))
  return json(
    await fetch(
      `${BASE}/internal/agent/${encodeURIComponent(username)}/history?${params}`
    )
  )
}

export async function getTasks(
  username: string,
  channel: string,
  status: 'all' | TaskStatus = 'all'
): Promise<TasksResponse> {
  const params = new URLSearchParams({ channel, status })
  return json(
    await fetch(
      `${BASE}/internal/agent/${encodeURIComponent(username)}/tasks?${params}`
    )
  )
}

export async function createTasks(
  username: string,
  channel: string,
  titles: string[]
): Promise<TasksResponse> {
  return json(
    await fetch(`${BASE}/internal/agent/${encodeURIComponent(username)}/tasks`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ channel, tasks: titles.map((title) => ({ title })) }),
    })
  )
}

export async function claimTasks(
  username: string,
  channel: string,
  taskNumbers: number[]
): Promise<{ results: Array<{ taskNumber: number; success: boolean; reason?: string }> }> {
  return json(
    await fetch(`${BASE}/internal/agent/${encodeURIComponent(username)}/tasks/claim`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ channel, task_numbers: taskNumbers }),
    })
  )
}

export async function unclaimTask(
  username: string,
  channel: string,
  taskNumber: number
): Promise<void> {
  await json(
    await fetch(`${BASE}/internal/agent/${encodeURIComponent(username)}/tasks/unclaim`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ channel, task_number: taskNumber }),
    })
  )
}

export async function updateTaskStatus(
  username: string,
  channel: string,
  taskNumber: number,
  status: TaskStatus
): Promise<void> {
  await json(
    await fetch(
      `${BASE}/internal/agent/${encodeURIComponent(username)}/tasks/update-status`,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ channel, task_number: taskNumber, status }),
      }
    )
  )
}

export async function uploadFile(
  username: string,
  file: File
): Promise<UploadResponse> {
  const form = new FormData()
  form.append('file', file)
  return json(
    await fetch(`${BASE}/internal/agent/${encodeURIComponent(username)}/upload`, {
      method: 'POST',
      body: form,
    })
  )
}

export async function resolveChannel(
  username: string,
  target: string
): Promise<ResolveChannelResponse> {
  return json(
    await fetch(
      `${BASE}/internal/agent/${encodeURIComponent(username)}/resolve-channel`,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ target }),
      }
    )
  )
}

export function attachmentUrl(id: string): string {
  return `${BASE}/api/attachments/${id}`
}
```

---

## Task 3: App state / context (`store.ts`, `App.tsx`, `App.css`)

**Files:** `ui/src/store.ts`, `ui/src/App.tsx`, `ui/src/App.css`

**Commit message:** `feat(ui): add global app context and root layout`

- [ ] **Step 1: Create `ui/src/store.ts`**

```typescript
import React, { createContext, useContext, useState, useEffect, useCallback } from 'react'
import type { ServerInfo, AgentInfo } from './types'
import { getWhoami, getServerInfo } from './api'

export type ActiveTab = 'chat' | 'tasks' | 'workspace' | 'activity' | 'profile'

export interface AppState {
  currentUser: string                  // OS username from /api/whoami
  serverInfo: ServerInfo | null
  serverInfoLoading: boolean
  selectedChannel: string | null       // e.g. "#general"
  selectedAgent: AgentInfo | null      // non-null when viewing a DM with an agent
  activeTab: ActiveTab
  setSelectedChannel: (ch: string | null) => void
  setSelectedAgent: (agent: AgentInfo | null) => void
  setActiveTab: (tab: ActiveTab) => void
  refreshServerInfo: () => void
}

const AppContext = createContext<AppState | null>(null)

export function AppProvider({ children }: { children: React.ReactNode }) {
  const [currentUser, setCurrentUser] = useState('')
  const [serverInfo, setServerInfo] = useState<ServerInfo | null>(null)
  const [serverInfoLoading, setServerInfoLoading] = useState(true)
  const [selectedChannel, setSelectedChannel] = useState<string | null>(null)
  const [selectedAgent, setSelectedAgent] = useState<AgentInfo | null>(null)
  const [activeTab, setActiveTab] = useState<ActiveTab>('chat')

  // Fetch current user once on mount
  useEffect(() => {
    getWhoami()
      .then((r) => setCurrentUser(r.username))
      .catch(() => setCurrentUser('user'))
  }, [])

  const refreshServerInfo = useCallback(() => {
    if (!currentUser) return
    setServerInfoLoading(true)
    getServerInfo(currentUser)
      .then((info) => {
        setServerInfo(info)
        // Auto-select first joined channel if nothing selected
        setSelectedChannel((prev) => {
          if (prev) return prev
          const first = info.channels.find((c) => c.joined)
          return first ? `#${first.name}` : null
        })
      })
      .catch(console.error)
      .finally(() => setServerInfoLoading(false))
  }, [currentUser])

  // Poll server info every 10s
  useEffect(() => {
    if (!currentUser) return
    refreshServerInfo()
    const id = setInterval(refreshServerInfo, 10_000)
    return () => clearInterval(id)
  }, [currentUser, refreshServerInfo])

  // When selecting an agent, switch to chat tab
  const handleSetSelectedAgent = useCallback((agent: AgentInfo | null) => {
    setSelectedAgent(agent)
    if (agent) {
      setSelectedChannel(null)
      setActiveTab('chat')
    }
  }, [])

  const handleSetSelectedChannel = useCallback((ch: string | null) => {
    setSelectedChannel(ch)
    if (ch) {
      setSelectedAgent(null)
      setActiveTab('chat')
    }
  }, [])

  return (
    <AppContext.Provider
      value={{
        currentUser,
        serverInfo,
        serverInfoLoading,
        selectedChannel,
        selectedAgent,
        activeTab,
        setSelectedChannel: handleSetSelectedChannel,
        setSelectedAgent: handleSetSelectedAgent,
        setActiveTab,
        refreshServerInfo,
      }}
    >
      {children}
    </AppContext.Provider>
  )
}

export function useApp(): AppState {
  const ctx = useContext(AppContext)
  if (!ctx) throw new Error('useApp must be used inside AppProvider')
  return ctx
}

// Derive the active "target" string for API calls
export function useTarget(): string | null {
  const { selectedChannel, selectedAgent } = useApp()
  if (selectedChannel) return selectedChannel
  if (selectedAgent) return `dm:@${selectedAgent.name}`
  return null
}
```

- [ ] **Step 2: Create `ui/src/App.tsx`**

```tsx
import { AppProvider } from './store'
import { Sidebar } from './components/Sidebar'
import { MainPanel } from './components/MainPanel'

export default function App() {
  return (
    <AppProvider>
      <div className="app-shell">
        <Sidebar />
        <MainPanel />
      </div>
    </AppProvider>
  )
}
```

Note: `MainPanel` is a thin wrapper that will be introduced in Task 9. For now a placeholder can be used.

- [ ] **Step 3: Create `ui/src/App.css`**

```css
/* ── Design tokens ── */
:root {
  --sidebar-bg: #FFD700;
  --sidebar-hover: #FFC800;
  --sidebar-active: #1a1a1a;
  --sidebar-active-text: #FFD700;
  --header-bg: #1a1a1a;
  --header-text: #ffffff;
  --content-bg: #FFFFF0;
  --border: #e0e0e0;
  --accent: #FF2D55;
  --badge-claude: #2563EB;
  --badge-sonnet: #7C3AED;
  --status-online: #22C55E;
  --status-sleeping: #F59E0B;
  --status-inactive: #9CA3AF;
  --text-primary: #1a1a1a;
  --text-muted: #666;
  --font: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  --font-mono: "SF Mono", "Fira Code", monospace;
  --sidebar-width: 240px;
}

*, *::before, *::after {
  box-sizing: border-box;
  margin: 0;
  padding: 0;
}

html, body, #root {
  height: 100%;
  font-family: var(--font);
  font-size: 14px;
  color: var(--text-primary);
  background: var(--content-bg);
}

.app-shell {
  display: flex;
  height: 100vh;
  overflow: hidden;
}

button {
  cursor: pointer;
  border: none;
  background: none;
  font-family: inherit;
}

input, textarea, select {
  font-family: inherit;
  font-size: inherit;
}
```

---

## Task 4: Sidebar component

**Files:** `ui/src/components/Sidebar.tsx`, `ui/src/components/Sidebar.css`

**Commit message:** `feat(ui): add sidebar with channels, agents, and humans sections`

- [ ] **Step 1: Create `ui/src/components/Sidebar.css`**

```css
.sidebar {
  width: var(--sidebar-width);
  background: var(--sidebar-bg);
  display: flex;
  flex-direction: column;
  overflow: hidden;
  flex-shrink: 0;
  border-right: 2px solid #e6c200;
}

.sidebar-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 12px 10px;
  border-bottom: 1px solid rgba(0,0,0,0.1);
}

.sidebar-server-name {
  font-weight: 700;
  font-size: 15px;
  color: var(--text-primary);
  display: flex;
  align-items: center;
  gap: 4px;
}

.sidebar-server-name button {
  font-size: 11px;
  opacity: 0.6;
}

.sidebar-icon-btn {
  width: 28px;
  height: 28px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 16px;
  transition: background 0.15s;
}

.sidebar-icon-btn:hover {
  background: rgba(0,0,0,0.1);
}

.sidebar-body {
  flex: 1;
  overflow-y: auto;
  padding: 8px 0;
}

.sidebar-section {
  margin-bottom: 8px;
}

.sidebar-section-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 4px 12px;
}

.sidebar-section-label {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  color: rgba(0,0,0,0.55);
}

.sidebar-add-btn {
  font-size: 18px;
  line-height: 1;
  color: rgba(0,0,0,0.45);
  padding: 0 2px;
  border-radius: 4px;
  transition: background 0.15s;
}

.sidebar-add-btn:hover {
  background: rgba(0,0,0,0.1);
  color: var(--text-primary);
}

.sidebar-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 5px 12px;
  cursor: pointer;
  border-radius: 0;
  transition: background 0.1s;
  user-select: none;
}

.sidebar-item:hover {
  background: var(--sidebar-hover);
}

.sidebar-item.active {
  background: var(--sidebar-active);
  color: var(--sidebar-active-text);
}

.sidebar-item.active .sidebar-item-text {
  color: var(--sidebar-active-text);
}

.sidebar-item.active .sidebar-item-hash {
  color: rgba(255,215,0,0.6);
}

.sidebar-item-hash {
  font-size: 16px;
  color: rgba(0,0,0,0.4);
  flex-shrink: 0;
}

.sidebar-item-text {
  font-size: 14px;
  font-weight: 500;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* Agent / Human rows */
.agent-avatar {
  width: 24px;
  height: 24px;
  border-radius: 4px;
  flex-shrink: 0;
  image-rendering: pixelated;
  position: relative;
}

.agent-avatar-img {
  width: 100%;
  height: 100%;
  border-radius: 4px;
}

.status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  border: 2px solid var(--sidebar-bg);
  position: absolute;
  bottom: -2px;
  right: -2px;
}

.status-dot.online  { background: var(--status-online); }
.status-dot.sleeping { background: var(--status-sleeping); }
.status-dot.inactive { background: var(--status-inactive); }

.sidebar-item.active .status-dot {
  border-color: var(--sidebar-active);
}

.you-badge {
  font-size: 10px;
  font-weight: 700;
  background: rgba(0,0,0,0.15);
  border-radius: 3px;
  padding: 1px 4px;
  margin-left: auto;
  flex-shrink: 0;
}

/* Bottom user bar */
.sidebar-footer {
  border-top: 1px solid rgba(0,0,0,0.12);
  padding: 8px 12px;
  display: flex;
  align-items: center;
  gap: 8px;
}

.sidebar-footer-name {
  flex: 1;
  font-weight: 600;
  font-size: 13px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.sidebar-footer-cog {
  font-size: 18px;
  color: rgba(0,0,0,0.45);
  border-radius: 50%;
  width: 28px;
  height: 28px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.sidebar-footer-cog:hover {
  background: rgba(0,0,0,0.1);
}
```

- [ ] **Step 2: Create `ui/src/components/Sidebar.tsx`**

```tsx
import { useState } from 'react'
import { useApp } from '../store'
import type { AgentInfo } from '../types'
import { CreateAgentModal } from './CreateAgentModal'
import './Sidebar.css'

function agentColor(name: string): string {
  const colors = ['#FF6B6B','#4ECDC4','#45B7D1','#96CEB4','#FFEAA7','#DDA0DD','#98D8C8']
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff
  return colors[Math.abs(h) % colors.length]
}

function AgentAvatar({ name, status }: { name: string; status: string }) {
  const color = agentColor(name)
  const initial = name[0]?.toUpperCase() ?? '?'
  const dotClass =
    status === 'active' ? 'online' : status === 'sleeping' ? 'sleeping' : 'inactive'
  return (
    <div className="agent-avatar" style={{ position: 'relative' }}>
      <div
        className="agent-avatar-img"
        style={{
          background: color,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          fontSize: 12,
          fontWeight: 700,
          color: '#fff',
          fontFamily: 'var(--font-mono)',
        }}
      >
        {initial}
      </div>
      <span className={`status-dot ${dotClass}`} />
    </div>
  )
}

export function Sidebar() {
  const {
    currentUser,
    serverInfo,
    selectedChannel,
    selectedAgent,
    setSelectedChannel,
    setSelectedAgent,
    refreshServerInfo,
  } = useApp()
  const [showCreateAgent, setShowCreateAgent] = useState(false)

  const channels = serverInfo?.channels.filter((c) => c.joined) ?? []
  const agents = serverInfo?.agents ?? []
  const humans = serverInfo?.humans ?? []

  return (
    <>
      <nav className="sidebar">
        {/* Header */}
        <div className="sidebar-header">
          <span className="sidebar-server-name">
            Squad-Alpha <button>▾</button>
          </span>
          <div style={{ display: 'flex', gap: 4 }}>
            <button className="sidebar-icon-btn">✦</button>
            <button className="sidebar-icon-btn">⊕</button>
          </div>
        </div>

        <div className="sidebar-body">
          {/* Channels */}
          <div className="sidebar-section">
            <div className="sidebar-section-header">
              <span className="sidebar-section-label">Channels</span>
              <button className="sidebar-add-btn" title="Add channel">+</button>
            </div>
            {channels.map((ch) => {
              const target = `#${ch.name}`
              return (
                <div
                  key={ch.name}
                  className={`sidebar-item${selectedChannel === target ? ' active' : ''}`}
                  onClick={() => setSelectedChannel(target)}
                >
                  <span className="sidebar-item-hash">#</span>
                  <span className="sidebar-item-text">{ch.name}</span>
                </div>
              )
            })}
          </div>

          {/* Agents */}
          <div className="sidebar-section">
            <div className="sidebar-section-header">
              <span className="sidebar-section-label">Agents</span>
              <button
                className="sidebar-add-btn"
                title="Create agent"
                onClick={() => setShowCreateAgent(true)}
              >
                +
              </button>
            </div>
            {agents.map((agent) => (
              <div
                key={agent.name}
                className={`sidebar-item${
                  selectedAgent?.name === agent.name ? ' active' : ''
                }`}
                onClick={() => setSelectedAgent(agent as AgentInfo)}
              >
                <AgentAvatar name={agent.name} status={agent.status} />
                <span className="sidebar-item-text">{agent.display_name ?? agent.name}</span>
              </div>
            ))}
          </div>

          {/* Humans */}
          <div className="sidebar-section">
            <div className="sidebar-section-header">
              <span className="sidebar-section-label">Humans</span>
            </div>
            {humans.map((h) => (
              <div key={h.name} className="sidebar-item">
                <div
                  className="agent-avatar"
                  style={{
                    background: agentColor(h.name),
                    borderRadius: 4,
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    fontSize: 12,
                    fontWeight: 700,
                    color: '#fff',
                  }}
                >
                  {h.name[0]?.toUpperCase()}
                </div>
                <span className="sidebar-item-text">{h.name}</span>
                {h.name === currentUser && <span className="you-badge">you</span>}
              </div>
            ))}
          </div>
        </div>

        {/* Footer */}
        <div className="sidebar-footer">
          <div
            style={{
              width: 32,
              height: 32,
              borderRadius: 6,
              background: agentColor(currentUser),
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              fontSize: 14,
              fontWeight: 700,
              color: '#fff',
              flexShrink: 0,
            }}
          >
            {currentUser[0]?.toUpperCase() ?? '?'}
          </div>
          <span className="sidebar-footer-name">{currentUser}</span>
          <button className="sidebar-footer-cog">⚙</button>
        </div>
      </nav>

      {showCreateAgent && (
        <CreateAgentModal
          onClose={() => setShowCreateAgent(false)}
          onCreated={() => {
            setShowCreateAgent(false)
            refreshServerInfo()
          }}
        />
      )}
    </>
  )
}
```

---

## Task 5: Chat panel + MessageItem + polling hook

**Files:** `ui/src/hooks/useHistory.ts`, `ui/src/hooks/useServerInfo.ts`, `ui/src/components/ChatPanel.tsx`, `ui/src/components/ChatPanel.css`, `ui/src/components/MessageItem.tsx`

**Commit message:** `feat(ui): add chat panel with message history polling`

- [ ] **Step 1: Create `ui/src/hooks/useServerInfo.ts`**

The server info polling is already embedded in `store.ts`. This hook simply re-exports it for components that want direct access without importing the full context.

```typescript
// Re-export convenience — full polling logic lives in store.ts AppProvider
export { useApp as useServerInfo } from '../store'
```

- [ ] **Step 2: Create `ui/src/hooks/useHistory.ts`**

```typescript
import { useState, useEffect, useRef, useCallback } from 'react'
import { getHistory } from '../api'
import type { HistoryMessage } from '../types'

export function useHistory(username: string, target: string | null) {
  const [messages, setMessages] = useState<HistoryMessage[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const lastSeqRef = useRef<number>(0)

  const fetchHistory = useCallback(async () => {
    if (!username || !target) return
    try {
      const res = await getHistory(username, target, 50)
      setMessages(res.messages)
      if (res.messages.length > 0) {
        lastSeqRef.current = res.messages[res.messages.length - 1].seq
      }
      setError(null)
    } catch (e) {
      setError(String(e))
    } finally {
      setLoading(false)
    }
  }, [username, target])

  useEffect(() => {
    if (!target) {
      setMessages([])
      return
    }
    setLoading(true)
    setMessages([])
    lastSeqRef.current = 0
    fetchHistory()
    const id = setInterval(fetchHistory, 2_000)
    return () => clearInterval(id)
  }, [target, fetchHistory])

  return { messages, loading, error, refresh: fetchHistory }
}
```

- [ ] **Step 3: Create `ui/src/components/MessageItem.tsx`**

```tsx
import type { HistoryMessage } from '../types'
import { attachmentUrl } from '../api'

// Parse @mentions and render as colored inline pills
function renderContent(content: string) {
  const parts = content.split(/(@\w+)/g)
  return parts.map((part, i) =>
    part.startsWith('@') ? (
      <span key={i} className="mention-pill">
        {part}
      </span>
    ) : (
      <span key={i}>{part}</span>
    )
  )
}

function formatTime(iso: string): string {
  try {
    return new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  } catch {
    return iso
  }
}

function formatDate(iso: string): string {
  try {
    return new Date(iso).toLocaleDateString([], {
      month: 'short',
      day: 'numeric',
      year: 'numeric',
    })
  } catch {
    return iso
  }
}

function senderColor(name: string): string {
  const colors = [
    '#C0392B','#2980B9','#27AE60','#8E44AD','#D35400','#16A085','#2C3E50',
  ]
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff
  return colors[Math.abs(h) % colors.length]
}

interface MessageItemProps {
  message: HistoryMessage
  currentUser: string
  prevMessage?: HistoryMessage
}

export function MessageItem({ message, currentUser, prevMessage }: MessageItemProps) {
  const isMe = message.senderName === currentUser
  const initial = message.senderName[0]?.toUpperCase() ?? '?'
  const color = senderColor(message.senderName)

  // Group messages from the same sender within 5 minutes
  const isGrouped =
    prevMessage?.senderName === message.senderName &&
    Math.abs(
      new Date(message.createdAt).getTime() - new Date(prevMessage.createdAt).getTime()
    ) < 5 * 60 * 1000

  return (
    <div className={`message-item${isGrouped ? ' grouped' : ''}`}>
      {!isGrouped && (
        <div
          className="message-avatar"
          style={{
            background: color,
          }}
        >
          {message.senderType === 'agent' ? (
            <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, fontWeight: 700 }}>
              {initial}
            </span>
          ) : (
            <span style={{ fontSize: 12, fontWeight: 700 }}>{initial}</span>
          )}
        </div>
      )}
      {isGrouped && <div className="message-avatar-spacer" />}
      <div className="message-body">
        {!isGrouped && (
          <div className="message-header">
            <span className="message-sender" style={{ color }}>
              {message.senderName}
              {message.senderType === 'agent' && (
                <span className="agent-badge">BOT</span>
              )}
              {isMe && <span className="you-inline-badge">you</span>}
            </span>
            <span className="message-time">
              {formatDate(message.createdAt)} {formatTime(message.createdAt)}
            </span>
          </div>
        )}
        <div className="message-content">{renderContent(message.content)}</div>
        {message.attachments && message.attachments.length > 0 && (
          <div className="message-attachments">
            {message.attachments.map((att) => (
              <a
                key={att.id}
                href={attachmentUrl(att.id)}
                target="_blank"
                rel="noreferrer"
                className="attachment-link"
              >
                📎 {att.filename}
              </a>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
```

- [ ] **Step 4: Create `ui/src/components/ChatPanel.css`**

```css
.chat-panel {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  background: var(--content-bg);
}

.chat-header {
  background: var(--header-bg);
  color: var(--header-text);
  padding: 0 16px;
  height: 48px;
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
  border-bottom: 2px solid #333;
}

.chat-header-icon {
  font-size: 18px;
  opacity: 0.7;
}

.chat-header-name {
  font-weight: 700;
  font-size: 15px;
}

.chat-header-desc {
  font-size: 12px;
  color: rgba(255,255,255,0.5);
  margin-left: 8px;
}

.chat-header-actions {
  margin-left: auto;
  display: flex;
  gap: 4px;
}

.chat-header-btn {
  color: rgba(255,255,255,0.6);
  font-size: 18px;
  padding: 4px;
  border-radius: 4px;
}

.chat-header-btn:hover {
  background: rgba(255,255,255,0.1);
  color: #fff;
}

.chat-messages {
  flex: 1;
  overflow-y: auto;
  padding: 16px 0 8px;
}

.chat-messages-empty {
  padding: 40px 20px;
  text-align: center;
  color: var(--text-muted);
  font-size: 13px;
}

.message-item {
  display: flex;
  padding: 2px 16px;
  gap: 12px;
  transition: background 0.1s;
}

.message-item:hover {
  background: rgba(0,0,0,0.03);
}

.message-item.grouped {
  padding-top: 1px;
  padding-bottom: 1px;
}

.message-avatar {
  width: 36px;
  height: 36px;
  border-radius: 6px;
  flex-shrink: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  color: #fff;
  align-self: flex-start;
  margin-top: 2px;
}

.message-avatar-spacer {
  width: 36px;
  flex-shrink: 0;
}

.message-body {
  flex: 1;
  min-width: 0;
}

.message-header {
  display: flex;
  align-items: baseline;
  gap: 8px;
  margin-bottom: 2px;
}

.message-sender {
  font-weight: 700;
  font-size: 14px;
}

.agent-badge {
  font-size: 9px;
  font-weight: 700;
  background: var(--badge-claude);
  color: #fff;
  border-radius: 3px;
  padding: 1px 4px;
  margin-left: 4px;
  vertical-align: middle;
}

.you-inline-badge {
  font-size: 9px;
  font-weight: 700;
  background: rgba(0,0,0,0.12);
  color: #666;
  border-radius: 3px;
  padding: 1px 4px;
  margin-left: 4px;
  vertical-align: middle;
}

.message-time {
  font-size: 11px;
  color: var(--text-muted);
}

.message-content {
  font-size: 14px;
  line-height: 1.5;
  word-break: break-word;
  white-space: pre-wrap;
}

.mention-pill {
  background: rgba(37, 99, 235, 0.12);
  color: var(--badge-claude);
  border-radius: 3px;
  padding: 0 3px;
  font-weight: 600;
}

.message-attachments {
  margin-top: 4px;
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}

.attachment-link {
  font-size: 12px;
  color: var(--badge-claude);
  text-decoration: none;
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 2px 8px;
  background: rgba(37,99,235,0.04);
}

.attachment-link:hover {
  background: rgba(37,99,235,0.1);
}
```

- [ ] **Step 5: Create `ui/src/components/ChatPanel.tsx`**

```tsx
import { useEffect, useRef } from 'react'
import { useApp, useTarget } from '../store'
import { useHistory } from '../hooks/useHistory'
import { MessageItem } from './MessageItem'
import './ChatPanel.css'

export function ChatPanel() {
  const { currentUser, selectedChannel, selectedAgent, serverInfo } = useApp()
  const target = useTarget()
  const { messages, loading } = useHistory(currentUser, target)
  const bottomRef = useRef<HTMLDivElement>(null)

  // Scroll to bottom when messages change
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [messages])

  const channelInfo = selectedChannel
    ? serverInfo?.channels.find((c) => `#${c.name}` === selectedChannel)
    : null

  const headerName = selectedChannel
    ? selectedChannel
    : selectedAgent
    ? `@${selectedAgent.display_name ?? selectedAgent.name}`
    : 'Select a channel'

  const headerDesc = channelInfo?.description ?? selectedAgent?.description ?? ''
  const headerIcon = selectedChannel ? '#' : selectedAgent ? '@' : '?'

  return (
    <div className="chat-panel">
      <div className="chat-header">
        <span className="chat-header-icon">{headerIcon}</span>
        <span className="chat-header-name">{headerName}</span>
        {headerDesc && <span className="chat-header-desc">{headerDesc}</span>}
        <div className="chat-header-actions">
          <button className="chat-header-btn">🔍</button>
          <button className="chat-header-btn">⋯</button>
        </div>
      </div>

      <div className="chat-messages">
        {loading && messages.length === 0 && (
          <div className="chat-messages-empty">Loading messages...</div>
        )}
        {!loading && messages.length === 0 && target && (
          <div className="chat-messages-empty">
            No messages yet. Be the first to say something!
          </div>
        )}
        {!target && (
          <div className="chat-messages-empty">Select a channel or agent to start chatting.</div>
        )}
        {messages.map((msg, i) => (
          <MessageItem
            key={msg.id}
            message={msg}
            currentUser={currentUser}
            prevMessage={messages[i - 1]}
          />
        ))}
        <div ref={bottomRef} />
      </div>
    </div>
  )
}
```

---

## Task 6: MessageInput component

**Files:** `ui/src/components/MessageInput.tsx`

**Commit message:** `feat(ui): add message input bar with file upload and task checkbox`

- [ ] **Step 1: Create `ui/src/components/MessageInput.tsx`**

```tsx
import { useState, useRef, type KeyboardEvent } from 'react'
import { useApp, useTarget } from '../store'
import { sendMessage, createTasks, uploadFile } from '../api'

interface Props {
  onMessageSent?: () => void
}

export function MessageInput({ onMessageSent }: Props) {
  const { currentUser, selectedChannel } = useApp()
  const target = useTarget()
  const [content, setContent] = useState('')
  const [alsoTask, setAlsoTask] = useState(false)
  const [sending, setSending] = useState(false)
  const [pendingFiles, setPendingFiles] = useState<File[]>([])
  const fileInputRef = useRef<HTMLInputElement>(null)

  const placeholder = target
    ? `Message ${target}`
    : 'Select a channel to message'

  async function handleSend() {
    if (!target || !currentUser || (!content.trim() && pendingFiles.length === 0)) return
    setSending(true)
    try {
      // Upload files first
      const attachmentIds: string[] = []
      for (const file of pendingFiles) {
        const res = await uploadFile(currentUser, file)
        attachmentIds.push(res.id)
      }

      await sendMessage(currentUser, target, content.trim(), attachmentIds)

      if (alsoTask && selectedChannel && content.trim()) {
        await createTasks(currentUser, selectedChannel, [content.trim()])
      }

      setContent('')
      setPendingFiles([])
      setAlsoTask(false)
      onMessageSent?.()
    } catch (e) {
      console.error('Send failed:', e)
    } finally {
      setSending(false)
    }
  }

  function handleKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      handleSend()
    }
  }

  function handleFileChange(e: React.ChangeEvent<HTMLInputElement>) {
    const files = Array.from(e.target.files ?? [])
    setPendingFiles((prev) => [...prev, ...files])
    if (fileInputRef.current) fileInputRef.current.value = ''
  }

  return (
    <div className="message-input-area">
      {pendingFiles.length > 0 && (
        <div className="message-input-files">
          {pendingFiles.map((f, i) => (
            <span key={i} className="file-chip">
              📎 {f.name}
              <button
                onClick={() => setPendingFiles((prev) => prev.filter((_, j) => j !== i))}
              >
                ×
              </button>
            </span>
          ))}
        </div>
      )}
      <div className="message-input-row">
        <button
          className="message-input-btn attach-btn"
          onClick={() => fileInputRef.current?.click()}
          disabled={!target}
          title="Attach file"
        >
          ⊕
        </button>
        <input
          ref={fileInputRef}
          type="file"
          multiple
          style={{ display: 'none' }}
          onChange={handleFileChange}
        />
        <textarea
          className="message-input-textarea"
          placeholder={placeholder}
          value={content}
          onChange={(e) => setContent(e.target.value)}
          onKeyDown={handleKeyDown}
          disabled={!target || sending}
          rows={1}
        />
        <button
          className="message-input-send"
          onClick={handleSend}
          disabled={!target || sending || (!content.trim() && pendingFiles.length === 0)}
        >
          {sending ? '...' : 'Send'}
        </button>
      </div>
      {selectedChannel && (
        <div className="message-input-footer">
          <label className="task-checkbox-label">
            <input
              type="checkbox"
              checked={alsoTask}
              onChange={(e) => setAlsoTask(e.target.checked)}
            />
            Also create as a task
          </label>
        </div>
      )}
    </div>
  )
}
```

Add these styles to `ChatPanel.css` (append):

```css
/* ── MessageInput styles (appended to ChatPanel.css) ── */

.message-input-area {
  border-top: 1px solid var(--border);
  padding: 10px 16px 12px;
  background: var(--content-bg);
  flex-shrink: 0;
}

.message-input-files {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  margin-bottom: 6px;
}

.file-chip {
  font-size: 12px;
  background: rgba(37,99,235,0.08);
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 2px 6px;
  display: flex;
  align-items: center;
  gap: 4px;
}

.file-chip button {
  font-size: 14px;
  color: var(--text-muted);
  line-height: 1;
}

.message-input-row {
  display: flex;
  align-items: flex-end;
  gap: 8px;
  background: #fff;
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 6px 8px;
}

.message-input-btn {
  font-size: 20px;
  color: var(--text-muted);
  flex-shrink: 0;
  padding: 2px;
  border-radius: 4px;
}

.message-input-btn:hover:not(:disabled) {
  background: rgba(0,0,0,0.06);
  color: var(--text-primary);
}

.message-input-textarea {
  flex: 1;
  border: none;
  outline: none;
  resize: none;
  font-size: 14px;
  line-height: 1.5;
  max-height: 120px;
  overflow-y: auto;
  background: transparent;
}

.message-input-send {
  background: var(--accent);
  color: #fff;
  font-weight: 700;
  font-size: 13px;
  padding: 6px 14px;
  border-radius: 6px;
  flex-shrink: 0;
  transition: opacity 0.15s;
}

.message-input-send:hover:not(:disabled) {
  opacity: 0.88;
}

.message-input-send:disabled {
  opacity: 0.45;
  cursor: default;
}

.message-input-footer {
  margin-top: 6px;
}

.task-checkbox-label {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-muted);
  cursor: pointer;
  user-select: none;
}
```

---

## Task 7: Tasks panel + polling hook

**Files:** `ui/src/hooks/useTasks.ts`, `ui/src/components/TasksPanel.tsx`, `ui/src/components/TasksPanel.css`

**Commit message:** `feat(ui): add tasks panel with kanban-style task board`

- [ ] **Step 1: Create `ui/src/hooks/useTasks.ts`**

```typescript
import { useState, useEffect, useCallback } from 'react'
import { getTasks } from '../api'
import type { TaskInfo } from '../types'

export function useTasks(username: string, channel: string | null) {
  const [tasks, setTasks] = useState<TaskInfo[]>([])
  const [loading, setLoading] = useState(false)

  const fetchTasks = useCallback(async () => {
    if (!username || !channel) return
    try {
      const res = await getTasks(username, channel, 'all')
      setTasks(res.tasks)
    } catch (e) {
      console.error('fetchTasks error', e)
    } finally {
      setLoading(false)
    }
  }, [username, channel])

  useEffect(() => {
    if (!channel) {
      setTasks([])
      return
    }
    setLoading(true)
    fetchTasks()
    const id = setInterval(fetchTasks, 5_000)
    return () => clearInterval(id)
  }, [channel, fetchTasks])

  return { tasks, loading, refresh: fetchTasks }
}
```

- [ ] **Step 2: Create `ui/src/components/TasksPanel.css`**

```css
.tasks-panel {
  flex: 1;
  display: flex;
  flex-direction: column;
  overflow: hidden;
  background: var(--content-bg);
}

.tasks-panel-header {
  background: var(--header-bg);
  color: var(--header-text);
  padding: 0 16px;
  height: 48px;
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
}

.tasks-panel-title {
  font-weight: 700;
  font-size: 15px;
}

.tasks-add-btn {
  margin-left: auto;
  background: var(--accent);
  color: #fff;
  font-size: 12px;
  font-weight: 700;
  padding: 5px 12px;
  border-radius: 6px;
}

.tasks-add-btn:hover {
  opacity: 0.88;
}

.tasks-board {
  flex: 1;
  overflow-x: auto;
  display: flex;
  gap: 16px;
  padding: 16px;
  align-items: flex-start;
}

.task-column {
  background: rgba(0,0,0,0.04);
  border-radius: 8px;
  padding: 10px;
  min-width: 220px;
  max-width: 280px;
  flex-shrink: 0;
}

.task-column-header {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  color: var(--text-muted);
  margin-bottom: 10px;
  display: flex;
  align-items: center;
  gap: 6px;
}

.task-count-badge {
  background: rgba(0,0,0,0.12);
  border-radius: 10px;
  padding: 1px 6px;
  font-size: 10px;
}

.task-card {
  background: #fff;
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 10px 12px;
  margin-bottom: 6px;
  cursor: pointer;
  transition: box-shadow 0.15s;
}

.task-card:hover {
  box-shadow: 0 2px 8px rgba(0,0,0,0.1);
}

.task-card-number {
  font-size: 11px;
  color: var(--text-muted);
  font-family: var(--font-mono);
}

.task-card-title {
  font-size: 13px;
  font-weight: 500;
  margin-top: 2px;
  line-height: 1.4;
}

.task-card-meta {
  margin-top: 6px;
  font-size: 11px;
  color: var(--text-muted);
}

.task-card-claimed {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  background: rgba(34,197,94,0.1);
  color: #15803d;
  border-radius: 10px;
  padding: 1px 6px;
}

.tasks-empty {
  padding: 40px 20px;
  text-align: center;
  color: var(--text-muted);
  font-size: 13px;
}

/* Task status colors */
.task-column[data-status="todo"] .task-column-header { color: #6B7280; }
.task-column[data-status="in_progress"] .task-column-header { color: #2563EB; }
.task-column[data-status="in_review"] .task-column-header { color: #D97706; }
.task-column[data-status="done"] .task-column-header { color: #22C55E; }

/* New task input */
.new-task-row {
  display: flex;
  gap: 6px;
  margin-top: 8px;
}

.new-task-input {
  flex: 1;
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 5px 8px;
  font-size: 13px;
  outline: none;
}

.new-task-input:focus {
  border-color: var(--accent);
}

.new-task-submit {
  background: var(--accent);
  color: #fff;
  font-size: 12px;
  font-weight: 700;
  padding: 5px 10px;
  border-radius: 4px;
}
```

- [ ] **Step 3: Create `ui/src/components/TasksPanel.tsx`**

```tsx
import { useState } from 'react'
import { useApp, useTarget } from '../store'
import { useTasks } from '../hooks/useTasks'
import { createTasks, updateTaskStatus } from '../api'
import type { TaskInfo, TaskStatus } from '../types'
import './TasksPanel.css'

const COLUMNS: { status: TaskStatus; label: string }[] = [
  { status: 'todo', label: 'To Do' },
  { status: 'in_progress', label: 'In Progress' },
  { status: 'in_review', label: 'In Review' },
  { status: 'done', label: 'Done' },
]

function TaskCard({
  task,
  currentUser,
  channel,
  onRefresh,
}: {
  task: TaskInfo
  currentUser: string
  channel: string
  onRefresh: () => void
}) {
  const nextStatus: Record<TaskStatus, TaskStatus | null> = {
    todo: 'in_progress',
    in_progress: 'in_review',
    in_review: 'done',
    done: null,
  }
  const next = nextStatus[task.status]

  async function advance() {
    if (!next) return
    try {
      await updateTaskStatus(currentUser, channel, task.taskNumber, next)
      onRefresh()
    } catch (e) {
      console.error(e)
    }
  }

  return (
    <div className="task-card" onClick={advance} title={next ? `Advance to ${next}` : 'Done'}>
      <div className="task-card-number">#{task.taskNumber}</div>
      <div className="task-card-title">{task.title}</div>
      <div className="task-card-meta">
        {task.claimedByName && (
          <span className="task-card-claimed">⚡ {task.claimedByName}</span>
        )}
        {!task.claimedByName && task.createdByName && (
          <span>by {task.createdByName}</span>
        )}
      </div>
    </div>
  )
}

export function TasksPanel() {
  const { currentUser, selectedChannel } = useApp()
  const { tasks, loading, refresh } = useTasks(currentUser, selectedChannel)
  const [newTaskTitle, setNewTaskTitle] = useState('')
  const [creating, setCreating] = useState(false)

  async function handleCreate() {
    if (!selectedChannel || !newTaskTitle.trim()) return
    setCreating(true)
    try {
      await createTasks(currentUser, selectedChannel, [newTaskTitle.trim()])
      setNewTaskTitle('')
      refresh()
    } catch (e) {
      console.error(e)
    } finally {
      setCreating(false)
    }
  }

  if (!selectedChannel) {
    return (
      <div className="tasks-panel">
        <div className="tasks-empty">Select a channel to view tasks.</div>
      </div>
    )
  }

  return (
    <div className="tasks-panel">
      <div className="tasks-panel-header">
        <span className="tasks-panel-title">Tasks — {selectedChannel}</span>
      </div>

      {loading && tasks.length === 0 ? (
        <div className="tasks-empty">Loading tasks...</div>
      ) : (
        <div className="tasks-board">
          {COLUMNS.map(({ status, label }) => {
            const col = tasks.filter((t) => t.status === status)
            return (
              <div key={status} className="task-column" data-status={status}>
                <div className="task-column-header">
                  {label}
                  <span className="task-count-badge">{col.length}</span>
                </div>
                {col.map((task) => (
                  <TaskCard
                    key={task.taskNumber}
                    task={task}
                    currentUser={currentUser}
                    channel={selectedChannel}
                    onRefresh={refresh}
                  />
                ))}
                {status === 'todo' && (
                  <div className="new-task-row">
                    <input
                      className="new-task-input"
                      placeholder="New task title..."
                      value={newTaskTitle}
                      onChange={(e) => setNewTaskTitle(e.target.value)}
                      onKeyDown={(e) => e.key === 'Enter' && handleCreate()}
                    />
                    <button
                      className="new-task-submit"
                      onClick={handleCreate}
                      disabled={creating || !newTaskTitle.trim()}
                    >
                      +
                    </button>
                  </div>
                )}
              </div>
            )
          })}
        </div>
      )}
    </div>
  )
}
```

---

## Task 8: Profile panel + CreateAgentModal

**Files:** `ui/src/components/ProfilePanel.tsx`, `ui/src/components/ProfilePanel.css`, `ui/src/components/CreateAgentModal.tsx`

**Commit message:** `feat(ui): add agent profile panel and create agent modal`

- [ ] **Step 1: Create `ui/src/components/ProfilePanel.css`**

```css
.profile-panel {
  flex: 1;
  overflow-y: auto;
  padding: 24px;
  background: var(--content-bg);
}

.profile-avatar-section {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  padding-bottom: 24px;
  border-bottom: 1px solid var(--border);
  margin-bottom: 24px;
}

.profile-avatar-large {
  width: 80px;
  height: 80px;
  border-radius: 12px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 32px;
  font-weight: 700;
  color: #fff;
  font-family: var(--font-mono);
}

.profile-name {
  font-size: 22px;
  font-weight: 700;
}

.profile-handle {
  font-size: 14px;
  color: var(--text-muted);
  font-family: var(--font-mono);
}

.profile-section {
  margin-bottom: 20px;
}

.profile-section-label {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  color: var(--text-muted);
  margin-bottom: 8px;
  display: flex;
  align-items: center;
  gap: 6px;
}

.profile-section-label button {
  font-size: 14px;
  color: var(--text-muted);
}

.profile-section-label button:hover {
  color: var(--text-primary);
}

.profile-role-text {
  font-size: 14px;
  line-height: 1.5;
  color: var(--text-primary);
}

.profile-config-grid {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 6px 12px;
  font-size: 13px;
}

.profile-config-key {
  color: var(--text-muted);
  font-weight: 500;
}

.badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 8px;
  border-radius: 4px;
  font-size: 12px;
  font-weight: 600;
  color: #fff;
}

.badge-claude { background: var(--badge-claude); }
.badge-sonnet { background: var(--badge-sonnet); }
.badge-connected { background: var(--status-online); }

.env-var-row {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  font-family: var(--font-mono);
  padding: 4px 0;
  border-bottom: 1px solid var(--border);
}

.env-var-key {
  font-weight: 600;
  min-width: 120px;
}

.env-var-val {
  color: var(--text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* Modal overlay */
.modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0,0,0,0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
}

.modal-box {
  background: #fff;
  border-radius: 10px;
  width: 460px;
  max-width: 95vw;
  padding: 28px 28px 24px;
  box-shadow: 0 8px 40px rgba(0,0,0,0.18);
}

.modal-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 20px;
}

.modal-title {
  font-size: 16px;
  font-weight: 800;
  letter-spacing: 0.04em;
  text-transform: uppercase;
}

.modal-close {
  font-size: 20px;
  color: var(--text-muted);
  border-radius: 50%;
  width: 32px;
  height: 32px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.modal-close:hover {
  background: rgba(0,0,0,0.06);
}

.modal-field {
  margin-bottom: 14px;
}

.modal-field label {
  display: block;
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.06em;
  text-transform: uppercase;
  color: var(--text-muted);
  margin-bottom: 5px;
}

.modal-field input,
.modal-field textarea,
.modal-field select {
  width: 100%;
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 8px 10px;
  font-size: 14px;
  outline: none;
  transition: border-color 0.15s;
}

.modal-field input:focus,
.modal-field textarea:focus,
.modal-field select:focus {
  border-color: var(--accent);
}

.modal-field textarea {
  resize: vertical;
  min-height: 70px;
}

.modal-accordion-trigger {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-muted);
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 8px 0;
  width: 100%;
  text-align: left;
  border-top: 1px solid var(--border);
  margin-top: 4px;
}

.modal-accordion-trigger:hover {
  color: var(--text-primary);
}

.env-var-editor {
  margin-top: 8px;
}

.env-var-editor-row {
  display: flex;
  gap: 6px;
  margin-bottom: 6px;
}

.env-var-editor-row input {
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 6px 8px;
  font-size: 13px;
  font-family: var(--font-mono);
  outline: none;
}

.env-var-editor-row input:first-child { width: 140px; }
.env-var-editor-row input:last-child { flex: 1; }

.env-add-btn {
  font-size: 12px;
  color: var(--badge-claude);
  font-weight: 600;
  padding: 4px 0;
}

.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 20px;
}

.btn-secondary {
  padding: 8px 16px;
  border-radius: 6px;
  font-size: 13px;
  font-weight: 600;
  border: 1px solid var(--border);
  color: var(--text-primary);
  background: #fff;
}

.btn-secondary:hover {
  background: rgba(0,0,0,0.04);
}

.btn-primary {
  padding: 8px 18px;
  border-radius: 6px;
  font-size: 13px;
  font-weight: 700;
  background: var(--accent);
  color: #fff;
}

.btn-primary:hover:not(:disabled) {
  opacity: 0.88;
}

.btn-primary:disabled {
  opacity: 0.45;
  cursor: default;
}
```

- [ ] **Step 2: Create `ui/src/components/ProfilePanel.tsx`**

```tsx
import { useApp } from '../store'
import './ProfilePanel.css'

function agentColor(name: string): string {
  const colors = ['#FF6B6B','#4ECDC4','#45B7D1','#96CEB4','#FFEAA7','#DDA0DD','#98D8C8']
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff
  return colors[Math.abs(h) % colors.length]
}

export function ProfilePanel() {
  const { selectedAgent } = useApp()

  if (!selectedAgent) {
    return (
      <div className="profile-panel" style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', color: 'var(--text-muted)' }}>
        Select an agent to view profile.
      </div>
    )
  }

  const color = agentColor(selectedAgent.name)
  const initial = selectedAgent.name[0]?.toUpperCase() ?? '?'

  return (
    <div className="profile-panel">
      <div className="profile-avatar-section">
        <div className="profile-avatar-large" style={{ background: color }}>
          {initial}
        </div>
        <div className="profile-name">{selectedAgent.display_name ?? selectedAgent.name}</div>
        <div className="profile-handle">@{selectedAgent.name}</div>
      </div>

      {selectedAgent.description && (
        <div className="profile-section">
          <div className="profile-section-label">
            Role <button title="Edit role">✎</button>
          </div>
          <div className="profile-role-text">{selectedAgent.description}</div>
        </div>
      )}

      <div className="profile-section">
        <div className="profile-section-label">Configuration</div>
        <div className="profile-config-grid">
          <span className="profile-config-key">Runtime</span>
          <span>
            <span className="badge badge-claude">
              {selectedAgent.runtime ?? 'Claude Code'}
            </span>
          </span>
          <span className="profile-config-key">Model</span>
          <span>
            <span className="badge badge-sonnet">
              {selectedAgent.model ?? 'Sonnet'}
            </span>
          </span>
          <span className="profile-config-key">Status</span>
          <span>
            <span
              className="badge"
              style={{
                background:
                  selectedAgent.status === 'active'
                    ? 'var(--status-online)'
                    : selectedAgent.status === 'sleeping'
                    ? 'var(--status-sleeping)'
                    : 'var(--status-inactive)',
              }}
            >
              {selectedAgent.status}
            </span>
          </span>
        </div>
      </div>
    </div>
  )
}
```

- [ ] **Step 3: Create `ui/src/components/CreateAgentModal.tsx`**

```tsx
import { useState } from 'react'
import { useApp } from '../store'
import './ProfilePanel.css'  // reuses modal styles

interface Props {
  onClose: () => void
  onCreated: () => void
}

interface EnvVar {
  key: string
  value: string
}

export function CreateAgentModal({ onClose, onCreated }: Props) {
  const { currentUser } = useApp()
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [runtime, setRuntime] = useState('claude')
  const [model, setModel] = useState('sonnet')
  const [showAdvanced, setShowAdvanced] = useState(false)
  const [envVars, setEnvVars] = useState<EnvVar[]>([])
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)

  function addEnvVar() {
    setEnvVars((prev) => [...prev, { key: '', value: '' }])
  }

  function updateEnvVar(i: number, field: 'key' | 'value', val: string) {
    setEnvVars((prev) => prev.map((e, j) => (j === i ? { ...e, [field]: val } : e)))
  }

  async function handleCreate() {
    if (!name.trim()) {
      setError('Name is required')
      return
    }
    setCreating(true)
    setError(null)
    try {
      // The backend CLI command is `chorus agent create <name>` but there's no REST endpoint yet.
      // For now we call the Rust CLI via a hypothetical /api/agents POST (to be added in Task 10 as an optional enhancement).
      // Minimal implementation: call the existing store directly would require a backend endpoint.
      // Since no POST /api/agents endpoint exists yet, we display an instructional message.
      // When Task 10 adds the endpoint, replace this with a real fetch call.
      alert(
        `To create agent, run:\nchorus agent create ${name.trim()} --runtime ${runtime} --model ${model}`
      )
      onCreated()
    } catch (e) {
      setError(String(e))
    } finally {
      setCreating(false)
    }
  }

  return (
    <div className="modal-overlay" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal-box">
        <div className="modal-header">
          <span className="modal-title">Create Agent</span>
          <button className="modal-close" onClick={onClose}>×</button>
        </div>

        <div className="modal-field">
          <label>Machine</label>
          <select disabled value="local">
            <option value="local">local</option>
          </select>
        </div>

        <div className="modal-field">
          <label>Name</label>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="e.g. my-agent"
            autoFocus
          />
        </div>

        <div className="modal-field">
          <label>Description</label>
          <textarea
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="What does this agent do?"
          />
        </div>

        <div className="modal-field">
          <label>Runtime</label>
          <select value={runtime} onChange={(e) => setRuntime(e.target.value)}>
            <option value="claude">Claude Code</option>
            <option value="codex">Codex CLI</option>
          </select>
        </div>

        <div className="modal-field">
          <label>Model</label>
          <select value={model} onChange={(e) => setModel(e.target.value)}>
            <option value="sonnet">Sonnet</option>
            <option value="opus">Opus</option>
            <option value="haiku">Haiku</option>
          </select>
        </div>

        <button
          className="modal-accordion-trigger"
          onClick={() => setShowAdvanced((v) => !v)}
        >
          {showAdvanced ? '▾' : '▸'} Advanced
        </button>

        {showAdvanced && (
          <div className="env-var-editor">
            {envVars.map((ev, i) => (
              <div key={i} className="env-var-editor-row">
                <input
                  placeholder="KEY"
                  value={ev.key}
                  onChange={(e) => updateEnvVar(i, 'key', e.target.value)}
                />
                <input
                  placeholder="value"
                  value={ev.value}
                  onChange={(e) => updateEnvVar(i, 'value', e.target.value)}
                />
              </div>
            ))}
            <button className="env-add-btn" onClick={addEnvVar}>
              + Add Variable
            </button>
          </div>
        )}

        {error && (
          <div style={{ color: 'var(--accent)', fontSize: 13, marginTop: 8 }}>{error}</div>
        )}

        <div className="modal-footer">
          <button className="btn-secondary" onClick={onClose}>Cancel</button>
          <button
            className="btn-primary"
            onClick={handleCreate}
            disabled={creating || !name.trim()}
          >
            {creating ? 'Creating...' : 'Create Agent'}
          </button>
        </div>
      </div>
    </div>
  )
}
```

---

## Task 9: TabBar + App layout wiring (MainPanel)

**Files:** `ui/src/components/TabBar.tsx`, `ui/src/components/MainPanel.tsx`

**Commit message:** `feat(ui): wire up tabbar and main panel layout`

- [ ] **Step 1: Create `ui/src/components/TabBar.tsx`**

```tsx
import { useApp } from '../store'
import type { ActiveTab } from '../store'

const CHANNEL_TABS: { id: ActiveTab; label: string }[] = [
  { id: 'chat', label: 'Chat' },
  { id: 'tasks', label: 'Tasks' },
]

const AGENT_TABS: { id: ActiveTab; label: string }[] = [
  { id: 'chat', label: 'Chat' },
  { id: 'tasks', label: 'Tasks' },
  { id: 'workspace', label: 'Workspace' },
  { id: 'activity', label: 'Activity' },
  { id: 'profile', label: 'Profile' },
]

export function TabBar() {
  const { selectedChannel, selectedAgent, activeTab, setActiveTab } = useApp()
  const tabs = selectedAgent ? AGENT_TABS : CHANNEL_TABS

  return (
    <div
      style={{
        display: 'flex',
        borderBottom: '2px solid var(--border)',
        background: 'var(--content-bg)',
        paddingLeft: 16,
        gap: 0,
        flexShrink: 0,
      }}
    >
      {tabs.map((tab) => (
        <button
          key={tab.id}
          onClick={() => setActiveTab(tab.id)}
          style={{
            padding: '10px 16px',
            fontSize: 12,
            fontWeight: 700,
            letterSpacing: '0.06em',
            textTransform: 'uppercase',
            borderBottom: activeTab === tab.id ? '2px solid var(--accent)' : '2px solid transparent',
            marginBottom: -2,
            color: activeTab === tab.id ? 'var(--accent)' : 'var(--text-muted)',
            background: 'none',
            cursor: 'pointer',
            transition: 'color 0.15s',
          }}
        >
          {tab.label}
        </button>
      ))}
    </div>
  )
}
```

- [ ] **Step 2: Create `ui/src/components/MainPanel.tsx`**

```tsx
import { useApp } from '../store'
import { TabBar } from './TabBar'
import { ChatPanel } from './ChatPanel'
import { TasksPanel } from './TasksPanel'
import { ProfilePanel } from './ProfilePanel'
import { MessageInput } from './MessageInput'
import { useHistory } from '../hooks/useHistory'
import { useTarget } from '../store'

export function MainPanel() {
  const { activeTab, currentUser, selectedChannel, selectedAgent } = useApp()
  const target = useTarget()
  const { refresh: refreshHistory } = useHistory(currentUser, target)

  const showHeader = selectedChannel || selectedAgent

  return (
    <div
      style={{
        flex: 1,
        display: 'flex',
        flexDirection: 'column',
        overflow: 'hidden',
        background: 'var(--content-bg)',
      }}
    >
      {showHeader && <TabBar />}

      <div style={{ flex: 1, display: 'flex', flexDirection: 'column', overflow: 'hidden' }}>
        {activeTab === 'chat' && (
          <>
            <ChatPanel />
            <MessageInput onMessageSent={refreshHistory} />
          </>
        )}
        {activeTab === 'tasks' && <TasksPanel />}
        {activeTab === 'profile' && <ProfilePanel />}
        {(activeTab === 'workspace' || activeTab === 'activity') && (
          <div
            style={{
              flex: 1,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              color: 'var(--text-muted)',
              fontSize: 14,
            }}
          >
            {activeTab.charAt(0).toUpperCase() + activeTab.slice(1)} — coming soon
          </div>
        )}
        {!showHeader && (
          <div
            style={{
              flex: 1,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              color: 'var(--text-muted)',
              flexDirection: 'column',
              gap: 8,
            }}
          >
            <span style={{ fontSize: 32 }}>🎵</span>
            <span>Select a channel or agent to get started</span>
          </div>
        )}
      </div>
    </div>
  )
}
```

- [ ] **Step 3: Update `ui/src/App.tsx` to import MainPanel properly**

The stub from Task 3 already imports `MainPanel` from `./components/MainPanel`. No additional changes needed.

---

## Task 10: Backend — add `/api/whoami`, static file serving, CORS

**Files:** `Cargo.toml`, `src/server.rs`

**Commit message:** `feat(server): add whoami endpoint, cors middleware, and static file serving for ui`

This task modifies the Rust backend. All changes must keep `cargo test` green.

- [ ] **Step 1: Update `Cargo.toml`**

Add `tower-http` with the `fs` and `cors` features, and `tower` for middleware composition:

```toml
tower-http = { version = "0.6", features = ["fs", "cors"] }
```

The `tower` dev-dependency is already present. No version bump needed.

- [ ] **Step 2: Update `src/server.rs` — add imports**

At the top of `src/server.rs`, add:

```rust
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
```

- [ ] **Step 3: Update `src/server.rs` — add `handle_whoami`**

Insert this handler function anywhere above `build_router`:

```rust
async fn handle_whoami() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "username": whoami::username() }))
}
```

Note: `whoami` is already a dependency in `Cargo.toml` and is used in `main.rs`, so no new crate is needed.

- [ ] **Step 4: Update `src/server.rs` — update `build_router`**

Replace the current `build_router` function body. The new version adds the CORS layer, the whoami route, and the static file fallback. The critical constraint is that all existing `/internal/` and `/api/` routes must continue to match before the static fallback.

```rust
pub fn build_router(store: Arc<Store>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // ── Existing API routes (unchanged) ──
        .route("/internal/agent/{agent_id}/send", post(handle_send))
        .route("/internal/agent/{agent_id}/receive", get(handle_receive))
        .route("/internal/agent/{agent_id}/history", get(handle_history))
        .route("/internal/agent/{agent_id}/server", get(handle_server_info))
        .route(
            "/internal/agent/{agent_id}/resolve-channel",
            post(handle_resolve_channel),
        )
        .route(
            "/internal/agent/{agent_id}/tasks",
            get(handle_list_tasks).post(handle_create_tasks),
        )
        .route(
            "/internal/agent/{agent_id}/tasks/claim",
            post(handle_claim_tasks),
        )
        .route(
            "/internal/agent/{agent_id}/tasks/unclaim",
            post(handle_unclaim_task),
        )
        .route(
            "/internal/agent/{agent_id}/tasks/update-status",
            post(handle_update_task_status),
        )
        .route("/internal/agent/{agent_id}/upload", post(handle_upload))
        .route(
            "/api/attachments/{attachment_id}",
            get(handle_get_attachment),
        )
        // ── New: whoami ──
        .route("/api/whoami", get(handle_whoami))
        // ── CORS middleware ──
        .layer(cors)
        // ── Static file serving (must be last — fallback for all non-API paths) ──
        .fallback_service(
            ServeDir::new("ui/dist")
                .fallback(ServeFile::new("ui/dist/index.html")),
        )
        .with_state(store)
}
```

Important: `.with_state(store)` must be the last call. The `fallback_service` does not need the store state so this ordering is correct. The CORS `.layer(cors)` applies to all routes including the static fallback, which is desirable for the dev proxy case.

- [ ] **Step 5: Verify `cargo test` still passes**

```bash
cd /Users/bytedance/slock-daemon/Chorus
cargo test
```

The existing test suite in `tests/store_tests.rs`, `tests/server_tests.rs`, and `tests/e2e_tests.rs` should not be affected because:
- All existing routes are unchanged.
- `handle_whoami` is a pure function with no state dependency.
- `ServeDir` falls back gracefully when `ui/dist` doesn't exist (returns 404 for static files, which does not affect API tests).

---

## Task 11: Build integration — `npm run build` → Rust serves

**Files:** No new files. Documents the dev and production workflows.

**Commit message:** `chore(ui): document build integration and add npm scripts`

- [ ] **Step 1: Development workflow**

```bash
# Terminal 1 — start Rust backend
cd /Users/bytedance/slock-daemon/Chorus
cargo run -- serve --port 3001

# Terminal 2 — start Vite dev server with proxy
cd /Users/bytedance/slock-daemon/Chorus/ui
npm run dev
# Opens http://localhost:5173
# API calls to /internal/* and /api/* are proxied to :3001
```

- [ ] **Step 2: Production build**

```bash
cd /Users/bytedance/slock-daemon/Chorus/ui
npm run build
# Outputs to ui/dist/

cd /Users/bytedance/slock-daemon/Chorus
cargo build --release

# Now serve everything on one port:
./target/release/chorus serve --port 3001
# http://localhost:3001 serves both the API and the React app
```

- [ ] **Step 3: Add a combined build script to `Cargo.toml` (optional)**

Optionally add a `build.rs` or `Makefile` target:

```makefile
# Makefile (optional)
.PHONY: build-ui build release

build-ui:
	cd ui && npm install && npm run build

build: build-ui
	cargo build --release

release: build
	@echo "Build complete. Run: ./target/release/chorus serve"
```

- [ ] **Step 4: Add `ui/dist` to `.gitignore`**

The `ui/dist/` directory should not be committed. Verify the project's `.gitignore` includes:

```
ui/dist/
ui/node_modules/
```

---

## Dependency / Sequencing Notes

The tasks have these dependencies:

```
Task 1 (scaffold)
  → Task 2 (types/api) — depends on scaffolding
    → Task 3 (store/context) — depends on types
      → Task 4 (sidebar) — depends on store
      → Task 5 (chat panel) — depends on store + api
        → Task 6 (message input) — depends on chat panel CSS
      → Task 7 (tasks panel) — depends on store + api
      → Task 8 (profile + modal) — depends on store
      → Task 9 (tabbar + wiring) — depends on all components
Task 10 (backend changes) — independent of Tasks 1-9, can be done in parallel
Task 11 (build integration) — depends on Tasks 1-10 all being complete
```

Tasks 4, 5, 7, 8 can be committed in any order once Tasks 1-3 are done. Task 9 should be last among the UI tasks. Task 10 is safe to commit at any point and does not block frontend development.

---

## Key Implementation Notes

**Polling strategy:** The UI polls rather than uses long-poll/WebSocket for simplicity. `/history` is polled every 2 seconds for the active channel. `/server` (sidebar data) is polled every 10 seconds. `/tasks` is polled every 5 seconds when the Tasks tab is active. This is efficient for a local daemon (no network latency).

**`AgentInfo` extension:** The `GET /server` endpoint currently returns `AgentInfo { name, status }` (from `models.rs`). The frontend types define optional fields `display_name`, `runtime`, `model`, `description`, `session_id`. The backend's `get_server_info` in `store.rs` should be checked — if it currently only returns name/status, it may need to be extended to return the full agent record fields. This is a backend store query change, not a server route change. The `AgentInfo` struct in `models.rs` should be enriched to include those fields.

**CreateAgentModal fallback:** The modal currently calls a CLI command because there is no `POST /api/agents` REST endpoint. As a future enhancement, `src/server.rs` can add `POST /api/agents` which creates the agent via the store and starts it via the AgentManager. The modal's `handleCreate` function is designed to be easily updated to replace the `alert()` call with a real `fetch`.

**CSS approach:** Plain CSS with CSS custom properties. No CSS modules or CSS-in-JS. Each component has a co-located `.css` file imported directly. The global tokens in `App.css` are available everywhere via `:root` variables.

**TypeScript strictness:** `tsconfig.json` uses `strict: true` including `noUnusedLocals` and `noUnusedParameters`. All props and return types must be explicit.

---

### Critical Files for Implementation

- `/Users/bytedance/slock-daemon/Chorus/src/server.rs` - Core file to modify for `/api/whoami`, CORS, and static serving
- `/Users/bytedance/slock-daemon/Chorus/Cargo.toml` - Add `tower-http` dependency with `fs` and `cors` features
- `/Users/bytedance/slock-daemon/Chorus/src/models.rs` - Check `AgentInfo` struct: may need `display_name`, `runtime`, `model`, `description` fields added so sidebar and profile panel receive full agent data
- `/Users/bytedance/slock-daemon/Chorus/ui/src/store.ts` - Global React context: the hub that all components wire into; implement this carefully before any component work
- `/Users/bytedance/slock-daemon/Chorus/ui/src/api.ts` - All HTTP calls: every component depends on this being correct and typed
