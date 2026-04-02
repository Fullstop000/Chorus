// ── Agents, runtimes, activity, workspace ──

export interface AgentInfo {
  id?: string
  name: string
  display_name?: string
  status: 'active' | 'sleeping' | 'inactive'
  runtime?: string
  model?: string
  reasoningEffort?: string
  description?: string
  session_id?: string
  /** Live activity state: online | thinking | working | offline */
  activity?: string
  activity_detail?: string
}

export interface AgentEnvVar {
  key: string
  value: string
}

export type RuntimeAuthStatus = 'authed' | 'unauthed'

export interface RuntimeStatusInfo {
  runtime: 'claude' | 'codex' | 'kimi' | 'opencode' | string
  installed: boolean
  authStatus?: RuntimeAuthStatus
}

export interface AgentDetailResponse {
  agent: AgentInfo
  envVars: AgentEnvVar[]
}

export interface ActivityMessage {
  id: string
  seq: number
  content: string
  channelName: string
  createdAt: string
}

export interface ActivityResponse {
  messages: ActivityMessage[]
}

export type ActivityEntryKind =
  | 'thinking'
  | 'tool_start'
  | 'text'
  | 'raw_output'
  | 'message_received'
  | 'message_sent'
  | 'status'

export interface ActivityEntry {
  kind: ActivityEntryKind
  // thinking / text
  text?: string
  content?: string
  // tool_start
  tool_name?: string
  tool_input?: string
  // message_received
  channel_label?: string
  sender_name?: string
  // message_sent
  target?: string
  // status
  activity?: string
  detail?: string
}

export interface ActivityLogEntry {
  seq: number
  timestamp_ms: number
  entry: ActivityEntry
}

export interface ActivityLogResponse {
  entries: ActivityLogEntry[]
  agent_activity: string
  agent_detail: string
}

export interface WorkspaceResponse {
  path: string
  files: string[]
}

export interface WorkspaceFileResponse {
  path: string
  content: string
  truncated: boolean
  sizeBytes: number
  modifiedMs?: number
}
