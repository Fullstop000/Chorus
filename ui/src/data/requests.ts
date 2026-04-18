import type { AgentEnvVar } from './agents'

// ── Channel requests ──

export interface ListChannelsParams {
  member?: string
  include_archived?: boolean
  include_dm?: boolean
  include_system?: boolean
  include_team?: boolean
}

export interface CreateChannelRequest {
  name: string
  description: string
}

export interface UpdateChannelRequest {
  name: string
  description: string
}

export interface InviteChannelMemberRequest {
  memberName: string
}

// ── Chat / message requests ──

export interface SendMessageRequest {
  content: string
  attachmentIds?: string[]
  suppressAgentDelivery?: boolean
  suppressEvent?: boolean
}

export interface GetHistoryParams {
  limit?: number
  before?: number
  after?: number
}

export interface UpdateReadCursorRequest {
  lastReadSeq: number
}

// ── Task requests ──

export interface CreateTasksRequest {
  tasks: Array<{ title: string }>
}

export interface ClaimTasksRequest {
  task_numbers: number[]
}

export interface UnclaimTaskRequest {
  task_number: number
}

export interface UpdateTaskStatusRequest {
  task_number: number
  status: 'todo' | 'in_progress' | 'in_review' | 'done'
}

// ── Agent requests ──

export interface UpdateAgentRequest {
  display_name: string
  description: string
  runtime: string
  model: string
  reasoningEffort?: string | null
  envVars: AgentEnvVar[]
}

export interface RestartAgentRequest {
  mode: 'restart' | 'reset_session' | 'full_reset'
}

export interface DeleteAgentRequest {
  mode: 'preserve_workspace' | 'delete_workspace'
}

// ── Team requests ──

export interface CreateTeamRequest {
  name: string
  display_name: string
  members: Array<{
    member_name: string
    member_type: 'agent' | 'human'
    member_id: string
    role: string
  }>
}

export interface UpdateTeamRequest {
  display_name?: string
}

export interface AddTeamMemberRequest {
  member_name: string
  member_type: 'agent' | 'human'
  member_id: string
  role: string
}
