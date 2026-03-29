import type {
  ServerInfo,
  ChannelInfo,
  AgentInfo,
  HumanInfo,
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
  RuntimeStatusInfo,
  ChannelMembersResponse,
  Team,
  TeamResponse,
  InboxResponse,
  ThreadInboxResponse,
} from './types'

const BASE = ''  // same origin in prod; Vite proxy in dev

function conversationApiPath(conversationId: string, suffix = ''): string {
  return `${BASE}/api/conversations/${encodeURIComponent(conversationId)}${suffix}`
}

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

export async function listHumans(): Promise<HumanInfo[]> {
  return json(await fetch(`${BASE}/api/humans`))
}

export async function listChannels(params?: {
  member?: string
  includeArchived?: boolean
  includeDm?: boolean
  includeSystem?: boolean
  includeTeam?: boolean
}): Promise<ChannelInfo[]> {
  const search = new URLSearchParams()
  if (params?.member) search.set('member', params.member)
  if (params?.includeArchived) search.set('include_archived', 'true')
  if (params?.includeDm) search.set('include_dm', 'true')
  if (params?.includeSystem) search.set('include_system', 'true')
  if (params?.includeTeam === false) search.set('include_team', 'false')
  const suffix = search.size > 0 ? `?${search.toString()}` : ''
  return json(await fetch(`${BASE}/api/channels${suffix}`))
}

export async function listAgents(): Promise<AgentInfo[]> {
  return json(await fetch(`${BASE}/api/agents`))
}

export async function ensureDirectMessageConversation(
  peerName: string
): Promise<ChannelInfo> {
  return json(
    await fetch(`${BASE}/api/dms/${encodeURIComponent(peerName)}`, {
      method: 'PUT',
    })
  )
}

export async function listRuntimeStatuses(): Promise<RuntimeStatusInfo[]> {
  return json(await fetch(`${BASE}/api/runtimes`))
}

export async function createChannel(payload: {
  name: string
  description: string
}): Promise<{ id: string; name: string }> {
  return json(
    await fetch(`${BASE}/api/channels`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    })
  )
}

export async function updateChannel(
  channelId: string,
  payload: { name: string; description: string }
): Promise<{ id: string; name: string; description?: string | null }> {
  return json(
    await fetch(`${BASE}/api/channels/${encodeURIComponent(channelId)}`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    })
  )
}

export async function archiveChannel(channelId: string): Promise<{ ok: boolean }> {
  return json(
    await fetch(`${BASE}/api/channels/${encodeURIComponent(channelId)}/archive`, {
      method: 'POST',
    })
  )
}

export async function deleteChannel(channelId: string): Promise<{ ok: boolean }> {
  return json(
    await fetch(`${BASE}/api/channels/${encodeURIComponent(channelId)}`, {
      method: 'DELETE',
    })
  )
}

export async function getChannelMembers(channelId: string): Promise<ChannelMembersResponse> {
  return json(await fetch(`${BASE}/api/channels/${encodeURIComponent(channelId)}/members`))
}

export async function inviteChannelMember(
  channelId: string,
  memberName: string
): Promise<ChannelMembersResponse> {
  return json(
    await fetch(`${BASE}/api/channels/${encodeURIComponent(channelId)}/members`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ memberName }),
    })
  )
}

export async function sendMessage(
  conversationId: string,
  content: string,
  attachmentIds?: string[],
  options?: {
    suppressAgentDelivery?: boolean
    clientNonce?: string
    threadParentId?: string
  }
): Promise<{ messageId: string; seq: number; createdAt: string; clientNonce?: string }> {
  return json(
    await fetch(conversationApiPath(conversationId, '/messages'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        content,
        attachmentIds: attachmentIds ?? [],
        clientNonce: options?.clientNonce,
        suppressAgentDelivery: options?.suppressAgentDelivery ?? false,
        threadParentId: options?.threadParentId,
      }),
    })
  )
}

export async function getInboxState(username: string): Promise<InboxResponse> {
  void username
  return json(await fetch(`${BASE}/api/inbox`))
}

export async function getChannelThreads(
  conversationId: string
): Promise<ThreadInboxResponse> {
  return json(await fetch(conversationApiPath(conversationId, '/threads')))
}

export async function getHistory(
  conversationId: string,
  limit = 50,
  threadParentId?: string,
  before?: number,
  after?: number
): Promise<HistoryResponse> {
  const params = new URLSearchParams({ limit: String(limit) })
  if (threadParentId) params.set('threadParentId', threadParentId)
  if (before != null) params.set('before', String(before))
  if (after != null) params.set('after', String(after))
  return json(await fetch(`${conversationApiPath(conversationId, '/messages')}?${params}`))
}

export async function getHistoryAfter(
  conversationId: string,
  after: number,
  limit = 50,
  threadParentId?: string
): Promise<HistoryResponse> {
  return getHistory(conversationId, limit, threadParentId, undefined, after)
}

export async function updateReadCursor(
  conversationId: string,
  lastReadSeq: number,
  threadParentId?: string
): Promise<{ ok: boolean }> {
  return json(
    await fetch(conversationApiPath(conversationId, '/read-cursor'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ lastReadSeq, threadParentId }),
    })
  )
}

export async function getTasks(
  conversationId: string,
  status: 'all' | TaskStatus = 'all'
): Promise<TasksResponse> {
  const params = new URLSearchParams({ status })
  return json(await fetch(`${conversationApiPath(conversationId, '/tasks')}?${params}`))
}

export async function createTasks(
  conversationId: string,
  titles: string[]
): Promise<TasksResponse> {
  return json(
    await fetch(conversationApiPath(conversationId, '/tasks'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ tasks: titles.map((title) => ({ title })) }),
    })
  )
}

export async function claimTasks(
  conversationId: string,
  taskNumbers: number[]
): Promise<{ results: Array<{ taskNumber: number; success: boolean; reason?: string }> }> {
  return json(
    await fetch(conversationApiPath(conversationId, '/tasks/claim'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ task_numbers: taskNumbers }),
    })
  )
}

export async function unclaimTask(
  conversationId: string,
  taskNumber: number
): Promise<void> {
  await json(
    await fetch(conversationApiPath(conversationId, '/tasks/unclaim'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ task_number: taskNumber }),
    })
  )
}

export async function updateTaskStatus(
  conversationId: string,
  taskNumber: number,
  status: TaskStatus
): Promise<void> {
  await json(
    await fetch(conversationApiPath(conversationId, '/tasks/update-status'), {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ task_number: taskNumber, status }),
    })
  )
}

export async function uploadFile(
  file: File
): Promise<UploadResponse> {
  const form = new FormData()
  form.append('file', file)
  return json(
    await fetch(`${BASE}/api/attachments`, {
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
    reasoningEffort?: string | null
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

export async function listTeams(): Promise<Team[]> {
  return json(await fetch(`${BASE}/api/teams`))
}

export async function getTeam(name: string): Promise<TeamResponse> {
  return json(await fetch(`${BASE}/api/teams/${encodeURIComponent(name)}`))
}

export async function createTeam(payload: {
  name: string
  display_name: string
  collaboration_model: 'leader_operators' | 'swarm'
  leader_agent_name: string | null
  members: Array<{ member_name: string; member_type: 'agent' | 'human'; member_id: string; role: string }>
}): Promise<TeamResponse> {
  return json(
    await fetch(`${BASE}/api/teams`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    })
  )
}

export async function updateTeam(
  name: string,
  payload: {
    display_name?: string
    collaboration_model?: 'leader_operators' | 'swarm'
    leader_agent_name?: string | null
  }
): Promise<Team> {
  return json(
    await fetch(`${BASE}/api/teams/${encodeURIComponent(name)}`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    })
  )
}

export async function deleteTeam(name: string): Promise<void> {
  await json(
    await fetch(`${BASE}/api/teams/${encodeURIComponent(name)}`, {
      method: 'DELETE',
    })
  )
}

export async function addTeamMember(
  teamName: string,
  member: {
    member_name: string
    member_type: 'agent' | 'human'
    member_id: string
    role: string
  }
): Promise<void> {
  await json(
    await fetch(`${BASE}/api/teams/${encodeURIComponent(teamName)}/members`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(member),
    })
  )
}

export async function removeTeamMember(teamName: string, memberName: string): Promise<void> {
  await json(
    await fetch(
      `${BASE}/api/teams/${encodeURIComponent(teamName)}/members/${encodeURIComponent(memberName)}`,
      { method: 'DELETE' }
    )
  )
}
