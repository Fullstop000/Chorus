import type {
  ServerInfo,
  HistoryResponse,
  TasksResponse,
  TaskStatus,
  UploadResponse,
  ResolveChannelResponse,
  WhoamiResponse,
  ActivityResponse,
  ActivityLogResponse,
  WorkspaceResponse,
  WorkspaceFileResponse,
  AgentDetailResponse,
  AgentEnvVar,
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

export async function getServerInfo(_username: string): Promise<ServerInfo> {
  return json(await fetch(`${BASE}/api/server-info`))
}

export async function sendMessage(
  username: string,
  target: string,
  content: string,
  attachmentIds?: string[],
  options?: { suppressAgentDelivery?: boolean }
): Promise<{ messageId: string }> {
  return json(
    await fetch(`${BASE}/internal/agent/${encodeURIComponent(username)}/send`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        target,
        content,
        attachmentIds: attachmentIds ?? [],
        suppressAgentDelivery: options?.suppressAgentDelivery ?? false,
      }),
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

export async function getAgentActivity(agentName: string, limit = 50): Promise<ActivityResponse> {
  return json(await fetch(`${BASE}/api/agents/${encodeURIComponent(agentName)}/activity?limit=${limit}`))
}

export async function startAgent(agentName: string): Promise<void> {
  await json(await fetch(`${BASE}/api/agents/${encodeURIComponent(agentName)}/start`, { method: 'POST' }))
}

export async function stopAgent(agentName: string): Promise<void> {
  await json(await fetch(`${BASE}/api/agents/${encodeURIComponent(agentName)}/stop`, { method: 'POST' }))
}

export async function getAgentActivityLog(agentName: string, afterSeq?: number): Promise<ActivityLogResponse> {
  const params = afterSeq != null ? `?after=${afterSeq}` : ''
  return json(await fetch(`${BASE}/api/agents/${encodeURIComponent(agentName)}/activity-log${params}`))
}

export async function getAgentWorkspace(agentName: string): Promise<WorkspaceResponse> {
  return json(await fetch(`${BASE}/api/agents/${encodeURIComponent(agentName)}/workspace`))
}

export async function getAgentWorkspaceFile(agentName: string, path: string): Promise<WorkspaceFileResponse> {
  const params = new URLSearchParams({ path })
  return json(await fetch(`${BASE}/api/agents/${encodeURIComponent(agentName)}/workspace/file?${params.toString()}`))
}

export async function getAgentDetail(agentName: string): Promise<AgentDetailResponse> {
  return json(await fetch(`${BASE}/api/agents/${encodeURIComponent(agentName)}`))
}

export async function updateAgent(
  agentName: string,
  payload: {
    display_name: string
    description: string
    runtime: string
    model: string
    envVars: AgentEnvVar[]
  }
): Promise<{ ok: boolean; restarted: boolean }> {
  return json(
    await fetch(`${BASE}/api/agents/${encodeURIComponent(agentName)}`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    })
  )
}

export async function restartAgent(
  agentName: string,
  mode: 'restart' | 'reset_session' | 'full_reset'
): Promise<void> {
  await json(
    await fetch(`${BASE}/api/agents/${encodeURIComponent(agentName)}/restart`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ mode }),
    })
  )
}

export async function deleteAgent(
  agentName: string,
  mode: 'preserve_workspace' | 'delete_workspace'
): Promise<void> {
  await json(
    await fetch(`${BASE}/api/agents/${encodeURIComponent(agentName)}/delete`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ mode }),
    })
  )
}
