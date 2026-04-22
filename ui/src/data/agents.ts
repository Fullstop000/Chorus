import { get, post, patch } from './client'
import { queryOptions } from '@tanstack/react-query'
import type {
  UpdateAgentRequest,
  RestartAgentRequest,
  DeleteAgentRequest,
} from './requests'

// ── Types (source of truth) ──

export interface AgentInfo {
  id: string
  name: string
  display_name?: string
  status: 'working' | 'ready' | 'asleep' | 'failed'
  runtime?: string
  model?: string
  reasoningEffort?: string
  description?: string
  systemPrompt?: string
  activity?: string
  activity_detail?: string
}

export interface AgentEnvVar {
  key: string
  value: string
}

export type ProbeAuth = 'not_installed' | 'unauthed' | 'authed'

export interface RuntimeStatusInfo {
  runtime: 'claude' | 'codex' | 'kimi' | 'opencode' | string
  auth: ProbeAuth
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

export function getAgentDetail(agentId: string): Promise<AgentDetailResponse> {
  return get(`/api/agents/${encodeURIComponent(agentId)}`)
}

export function updateAgent(
  agentId: string,
  payload: UpdateAgentRequest
): Promise<{ ok: boolean; restarted: boolean }> {
  return patch(`/api/agents/${encodeURIComponent(agentId)}`, payload)
}

export function startAgent(agentId: string): Promise<void> {
  return post(`/api/agents/${encodeURIComponent(agentId)}/start`)
}

export function stopAgent(agentId: string): Promise<void> {
  return post(`/api/agents/${encodeURIComponent(agentId)}/stop`)
}

export function restartAgent(
  agentId: string,
  mode: RestartMode
): Promise<void> {
  return post(`/api/agents/${encodeURIComponent(agentId)}/restart`, { mode } as RestartAgentRequest)
}

export function deleteAgent(
  agentId: string,
  mode: DeleteMode
): Promise<{ ok: boolean; warning?: string; code?: string }> {
  return post(`/api/agents/${encodeURIComponent(agentId)}/delete`, { mode } as DeleteAgentRequest)
}

export function listRuntimeStatuses(): Promise<RuntimeStatusInfo[]> {
  return get('/api/runtimes')
}

export function listRuntimeModels(runtime: string): Promise<string[]> {
  return get(`/api/runtimes/${encodeURIComponent(runtime)}/models`)
}

export function getAgentActivity(agentId: string, limit = 50): Promise<ActivityResponse> {
  return get(`/api/agents/${encodeURIComponent(agentId)}/activity?limit=${limit}`)
}

export function getAgentActivityLog(agentId: string, afterSeq?: number): Promise<ActivityLogResponse> {
  const params = afterSeq != null ? `?after=${afterSeq}` : ''
  return get(`/api/agents/${encodeURIComponent(agentId)}/activity-log${params}`)
}

export function getAgentWorkspace(agentId: string): Promise<WorkspaceResponse> {
  return get(`/api/agents/${encodeURIComponent(agentId)}/workspace`)
}

export function getAgentWorkspaceFile(agentId: string, path: string): Promise<WorkspaceFileResponse> {
  return get(`/api/agents/${encodeURIComponent(agentId)}/workspace/file?path=${encodeURIComponent(path)}`)
}

// ── Agent runs (trace history) ──

export interface AgentRunInfo {
  messageId: string
  runId: string
  traceSummary: {
    toolCalls: number
    duration: number
    status: 'completed' | 'error'
    categories: Record<string, number>
  }
  createdAt: string
}

export interface AgentRunsResponse {
  runs: AgentRunInfo[]
}

export function getAgentRuns(agentId: string): Promise<AgentRunsResponse> {
  return get(`/api/agents/${encodeURIComponent(agentId)}/runs`)
}

// ── Transforms ──

export function toAgentLabel(agent: AgentInfo): string {
  return agent.display_name ?? agent.name
}

export function isAgentActive(agent: AgentInfo): boolean {
  return agent.status === 'ready' || agent.status === 'working'
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
