import { get, post, patch } from './client'
import { queryOptions } from '@tanstack/react-query'
import type {
  AgentInfo,
  AgentEnvVar,
  RuntimeStatusInfo,
  AgentDetailResponse,
  ActivityResponse,
  ActivityLogResponse,
  WorkspaceResponse,
  WorkspaceFileResponse,
} from '../types'

export type {
  AgentInfo,
  AgentEnvVar,
  RuntimeAuthStatus,
  RuntimeStatusInfo,
  AgentDetailResponse,
  ActivityMessage,
  ActivityResponse,
  ActivityEntryKind,
  ActivityEntry,
  ActivityLogEntry,
  ActivityLogResponse,
  WorkspaceResponse,
  WorkspaceFileResponse,
} from '../types'

export function listAgents(): Promise<AgentInfo[]> {
  return get('/api/agents')
}

export function getAgentDetail(agentName: string): Promise<AgentDetailResponse> {
  return get(`/api/agents/${encodeURIComponent(agentName)}`)
}

export function updateAgent(
  agentName: string,
  payload: {
    display_name: string
    description: string
    runtime: string
    model: string
    reasoningEffort?: string | null
    envVars: AgentEnvVar[]
  }
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
  mode: 'restart' | 'reset_session' | 'full_reset'
): Promise<void> {
  return post(`/api/agents/${encodeURIComponent(agentName)}/restart`, { mode })
}

export function deleteAgent(
  agentName: string,
  mode: 'preserve_workspace' | 'delete_workspace'
): Promise<void> {
  return post(`/api/agents/${encodeURIComponent(agentName)}/delete`, { mode })
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

export function toAgentLabel(agent: AgentInfo): string {
  return agent.display_name ?? agent.name
}

export function isAgentActive(agent: AgentInfo): boolean {
  return agent.status === 'active' || agent.status === 'sleeping'
}

export const agentQueryKeys = {
  agents: ['agents'] as const,
} as const

export const agentsQuery = (currentUser: string) =>
  queryOptions({
    queryKey: agentQueryKeys.agents,
    queryFn: listAgents,
    enabled: !!currentUser,
  })
