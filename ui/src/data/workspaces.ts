import { queryOptions } from '@tanstack/react-query'
import { del, get, post } from './client'

export type WorkspaceMode = 'local_only' | 'cloud'

export interface WorkspaceInfo {
  id: string
  name: string
  slug: string
  mode: WorkspaceMode
  created_by_human?: string | null
  created_at: string
  active: boolean
}

export interface DeleteWorkspaceResponse {
  deleted_id: string
  active_workspace: WorkspaceInfo | null
}

export function listWorkspaces(): Promise<WorkspaceInfo[]> {
  return get('/api/workspaces')
}

export function getCurrentWorkspace(): Promise<WorkspaceInfo> {
  return get('/api/workspaces/current')
}

export function createWorkspace(name: string): Promise<WorkspaceInfo> {
  return post('/api/workspaces', { name })
}

export function switchWorkspace(workspace: string): Promise<WorkspaceInfo> {
  return post('/api/workspaces/switch', { workspace })
}

export function deleteWorkspace(workspace: string): Promise<DeleteWorkspaceResponse> {
  return del(`/api/workspaces/${encodeURIComponent(workspace)}`)
}

export const workspaceQueryKeys = {
  workspaces: ['workspaces'] as const,
  current: ['workspaces', 'current'] as const,
} as const

export const workspacesQuery = queryOptions({
  queryKey: workspaceQueryKeys.workspaces,
  queryFn: listWorkspaces,
})

export const currentWorkspaceQuery = queryOptions({
  queryKey: workspaceQueryKeys.current,
  queryFn: getCurrentWorkspace,
  retry: false,
})
