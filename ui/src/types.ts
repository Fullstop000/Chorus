// ── Server Info ──

export interface ChannelInfo {
  id?: string
  name: string
  description?: string
  joined: boolean
  channel_type?: 'channel' | 'dm' | 'system' | 'team'
  read_only?: boolean
}

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

export interface AgentDetailResponse {
  agent: AgentInfo
  envVars: AgentEnvVar[]
}

export interface HumanInfo {
  name: string
}

export interface ChannelMemberInfo {
  memberName: string
  memberType: 'human' | 'agent'
  displayName?: string
}

export interface ChannelMembersResponse {
  channelId: string
  memberCount: number
  members: ChannelMemberInfo[]
}

export interface ServerInfo {
  /** System-managed channels (e.g. #all, #shared-memory). Shown separately. */
  system_channels: ChannelInfo[]
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
  senderDeleted: boolean
  createdAt: string
  thread_parent_id?: string
  attachments?: AttachmentRef[]
  replyCount?: number
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

// ── Agent Activity (legacy message history) ──

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

// ── Agent Activity Log (living log) ──

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

// ── Agent Workspace ──

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

// ── Teams ──

export interface Team {
  id: string
  name: string
  display_name: string
  channel_id?: string | null
  collaboration_model: 'leader_operators' | 'swarm'
  leader_agent_name?: string | null
  created_at: string
}

export interface TeamMember {
  team_id: string
  member_name: string
  member_type: 'agent' | 'human'
  member_id: string
  role: string
  joined_at: string
}

export interface TeamResponse {
  team: Team
  members: TeamMember[]
}

// ── App-level target union ──

// A "target" is the encoded channel/DM string passed to send/history
// e.g. "#all" or "dm:@alice"
export type Target = string
