import { get, post, patch } from './client'
import { queryOptions } from '@tanstack/react-query'
import type {
  UpdateAgentRequest,
  RestartAgentRequest,
  DeleteAgentRequest,
} from './requests'

// ── Types (source of truth) ──

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

export enum ActivityEntryKind {
  Start = 'start',
  Thinking = 'thinking',
  ToolCall = 'tool_call',
  ToolResult = 'tool_result',
  Text = 'text',
}

export interface ActivityEntry {
  kind: ActivityEntryKind
  is_resume?: boolean
  text?: string
  content?: string
  tool_name?: string
  tool_input?: string
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

export type RestartMode = 'restart' | 'reset_session' | 'full_reset'

export type DeleteMode = 'preserve_workspace' | 'delete_workspace'

export interface WorkspaceFileResponse {
  path: string
  content: string
  truncated: boolean
  sizeBytes: number
  modifiedMs?: number
}

// ── API functions ──

export function listAgents(): Promise<AgentInfo[]> {
  return get('/api/agents')
}

export function getAgentDetail(agentName: string): Promise<AgentDetailResponse> {
  return get(`/api/agents/${encodeURIComponent(agentName)}`)
}

export function updateAgent(
  agentName: string,
  payload: UpdateAgentRequest
): Promise<{ ok: boolean; restarted: boolean }> {
  return patch(`/api/agents/${encodeURIComponent(agentName)}`, payload)
}

export function startAgent(agentName: string): Promise<void> {
  return post(`/api/agents/${encodeURIComponent(agentName)}/start`)
}

export function stopAgent(agentName: string): Promise<void> {
  return post(`/api/agents/${encodeURIComponent(agentName)}/stop`)
}

export function restartAgent(
  agentName: string,
  mode: RestartMode
): Promise<void> {
  return post(`/api/agents/${encodeURIComponent(agentName)}/restart`, { mode } as RestartAgentRequest)
}

export function deleteAgent(
  agentName: string,
  mode: DeleteMode
): Promise<void> {
  return post(`/api/agents/${encodeURIComponent(agentName)}/delete`, { mode } as DeleteAgentRequest)
}

export function listRuntimeStatuses(): Promise<RuntimeStatusInfo[]> {
  return get('/api/runtimes')
}

export function listRuntimeModels(runtime: string): Promise<string[]> {
  return get(`/api/runtimes/${encodeURIComponent(runtime)}/models`)
}

export function getAgentActivity(agentName: string, limit = 50): Promise<ActivityResponse> {
  return get(`/api/agents/${encodeURIComponent(agentName)}/activity?limit=${limit}`)
}

export function getAgentActivityLog(agentName: string, afterSeq?: number): Promise<ActivityLogResponse> {
  const params = afterSeq != null ? `?after=${afterSeq}` : ''
  return get(`/api/agents/${encodeURIComponent(agentName)}/activity-log${params}`)
}

export function getAgentWorkspace(agentName: string): Promise<WorkspaceResponse> {
  return get(`/api/agents/${encodeURIComponent(agentName)}/workspace`)
}

export function getAgentWorkspaceFile(agentName: string, path: string): Promise<WorkspaceFileResponse> {
  return get(`/api/agents/${encodeURIComponent(agentName)}/workspace/file?path=${encodeURIComponent(path)}`)
}

// ── Transforms ──

export function toAgentLabel(agent: AgentInfo): string {
  return agent.display_name ?? agent.name
}

export function isAgentActive(agent: AgentInfo): boolean {
  return agent.status === 'active' || agent.status === 'sleeping'
}

// ── Query definitions ──

export const agentQueryKeys = {
  agents: ['agents'] as const,
} as const

export const agentsQuery = (currentUser: string) =>
  queryOptions({
    queryKey: agentQueryKeys.agents,
    queryFn: listAgents,
    enabled: !!currentUser,
  })
