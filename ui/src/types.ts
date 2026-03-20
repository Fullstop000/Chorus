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
