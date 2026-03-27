import type { APIRequestContext } from '@playwright/test'
import { expect } from '@playwright/test'

export interface AgentRow {
  id?: string
  name: string
  status: string
  display_name?: string
  runtime?: string
  model?: string
  reasoningEffort?: string | null
  description?: string | null
  session_id?: string | null
}

export interface AgentDetail {
  agent: AgentRow
  envVars: Array<{ key: string; value: string }>
}

export interface ChannelRow {
  id?: string
  name: string
  description?: string
  joined?: boolean
  channel_type?: 'channel' | 'dm' | 'system' | 'team'
  read_only?: boolean
}

export interface ChannelMembersResponse {
  channelId: string
  memberCount: number
  members: Array<{
    memberName: string
    memberType: 'human' | 'agent'
    displayName?: string
  }>
}

export interface KnowledgeEntry {
  id: string
  key: string
  value: string
  tags: string
  author_agent_id: string
  channel_context?: string | null
  created_at: string
}

export async function getWhoami(request: APIRequestContext): Promise<{ username: string }> {
  const res = await request.get('/api/whoami')
  expect(res.ok()).toBeTruthy()
  return res.json()
}

export async function listAgents(request: APIRequestContext): Promise<AgentRow[]> {
  const res = await request.get('/api/agents')
  expect(res.ok()).toBeTruthy()
  return res.json()
}

export async function getAgentDetail(request: APIRequestContext, name: string): Promise<AgentDetail> {
  const res = await request.get(`/api/agents/${encodeURIComponent(name)}`)
  expect(res.ok(), await res.text()).toBeTruthy()
  return res.json()
}

export async function createAgentApi(
  request: APIRequestContext,
  body: {
    name: string
    runtime: string
    model: string
    display_name?: string
    description?: string
    reasoningEffort?: string | null
    envVars?: Array<{ key: string; value: string }>
  }
): Promise<void> {
  const res = await request.post('/api/agents', {
    data: {
      name: body.name,
      display_name: body.display_name ?? body.name,
      description: body.description ?? 'qa playwright seed',
      runtime: body.runtime,
      model: body.model,
      reasoningEffort: body.reasoningEffort ?? null,
      envVars: body.envVars ?? [],
    },
  })
  expect(res.ok(), await res.text()).toBeTruthy()
}

/** API precondition helper only — catalog AGT-001 still requires UI creation when run for that case. */
export async function ensureMixedRuntimeTrio(request: APIRequestContext): Promise<void> {
  const agents = await listAgents(request)
  const names = new Set(agents.map((a) => a.name))
  if (!names.has('bot-a')) {
    await createAgentApi(request, { name: 'bot-a', runtime: 'claude', model: 'sonnet' })
  }
  if (!names.has('bot-b')) {
    await createAgentApi(request, { name: 'bot-b', runtime: 'claude', model: 'opus' })
  }
  if (!names.has('bot-c')) {
    await createAgentApi(request, { name: 'bot-c', runtime: 'codex', model: 'gpt-5.4-mini' })
  }
}

export async function waitForAgentActive(
  request: APIRequestContext,
  name: string,
  timeoutMs = 120_000
): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const agents = await listAgents(request)
    const a = agents.find((x) => x.name === name)
    if (a?.status === 'active') return
    await new Promise((r) => setTimeout(r, 2000))
  }
  throw new Error(`Agent ${name} did not become active within ${timeoutMs}ms`)
}

export async function waitForAgentStatus(
  request: APIRequestContext,
  name: string,
  status: string,
  timeoutMs = 120_000
): Promise<AgentRow> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const agents = await listAgents(request)
    const agent = agents.find((entry) => entry.name === name)
    if (agent?.status === status) return agent
    await new Promise((r) => setTimeout(r, 2000))
  }
  throw new Error(`Agent ${name} did not reach status ${status} within ${timeoutMs}ms`)
}

export async function startAgentApi(request: APIRequestContext, name: string): Promise<void> {
  const res = await request.post(`/api/agents/${encodeURIComponent(name)}/start`)
  expect(res.ok(), await res.text()).toBeTruthy()
}

export async function stopAgentApi(request: APIRequestContext, name: string): Promise<void> {
  const res = await request.post(`/api/agents/${encodeURIComponent(name)}/stop`)
  expect(res.ok(), await res.text()).toBeTruthy()
}

export async function restartAgentApi(
  request: APIRequestContext,
  name: string,
  mode: 'restart' | 'reset_session' | 'full_reset'
): Promise<void> {
  const res = await request.post(`/api/agents/${encodeURIComponent(name)}/restart`, {
    data: { mode },
  })
  expect(res.ok(), await res.text()).toBeTruthy()
}

export async function updateAgentApi(
  request: APIRequestContext,
  name: string,
  payload: {
    display_name: string
    description: string
    runtime: string
    model: string
    reasoningEffort?: string | null
    envVars: Array<{ key: string; value: string }>
  }
): Promise<void> {
  const res = await request.patch(`/api/agents/${encodeURIComponent(name)}`, {
    data: payload,
  })
  expect(res.ok(), await res.text()).toBeTruthy()
}

export async function deleteAgentApi(
  request: APIRequestContext,
  name: string,
  mode: 'preserve_workspace' | 'delete_workspace'
): Promise<void> {
  const res = await request.post(`/api/agents/${encodeURIComponent(name)}/delete`, {
    data: { mode },
  })
  expect(res.ok(), await res.text()).toBeTruthy()
}

export async function sendAsUser(
  request: APIRequestContext,
  username: string,
  target: string,
  content: string
): Promise<void> {
  const res = await request.post(`/internal/agent/${encodeURIComponent(username)}/send`, {
    data: { target, content },
  })
  expect(res.ok(), await res.text()).toBeTruthy()
}

export interface HistoryMessage {
  id?: string
  seq?: number
  senderName?: string
  senderType?: string
  content?: string
  forwardedFrom?: unknown
  senderDeleted?: boolean
  attachments?: Array<{ id: string; filename: string }>
  replyCount?: number
}

export interface ActivityLogEntry {
  seq: number
  timestamp_ms: number
  entry: {
    kind: string
    text?: string
    content?: string
    tool_name?: string
    tool_input?: string
    target?: string
    activity?: string
    detail?: string
  }
}

export interface AgentActivityLogResponse {
  entries: ActivityLogEntry[]
  agent_activity: string
  agent_detail: string
}

export async function historyForUser(
  request: APIRequestContext,
  username: string,
  channel: string,
  limit = 80
): Promise<HistoryMessage[]> {
  const q = new URLSearchParams({ channel, limit: String(limit) })
  const res = await request.get(
    `/internal/agent/${encodeURIComponent(username)}/history?${q.toString()}`
  )
  expect(res.ok(), await res.text()).toBeTruthy()
  const j = await res.json()
  return j.messages ?? []
}

export async function getAgentActivityLogApi(
  request: APIRequestContext,
  name: string
): Promise<AgentActivityLogResponse> {
  const res = await request.get(`/api/agents/${encodeURIComponent(name)}/activity-log`)
  expect(res.ok(), await res.text()).toBeTruthy()
  return res.json()
}

export async function listChannelsApi(
  request: APIRequestContext,
  params?: {
    member?: string
    includeArchived?: boolean
    includeDm?: boolean
    includeSystem?: boolean
    includeTeam?: boolean
  }
): Promise<ChannelRow[]> {
  const search = new URLSearchParams()
  if (params?.member) search.set('member', params.member)
  if (params?.includeArchived) search.set('include_archived', 'true')
  if (params?.includeDm) search.set('include_dm', 'true')
  if (params?.includeSystem) search.set('include_system', 'true')
  if (params?.includeTeam === false) search.set('include_team', 'false')
  const suffix = search.size > 0 ? `?${search.toString()}` : ''
  const res = await request.get(`/api/channels${suffix}`)
  expect(res.ok(), await res.text()).toBeTruthy()
  return res.json()
}

export async function createChannelApi(
  request: APIRequestContext,
  payload: { name: string; description?: string }
): Promise<{ id: string; name: string }> {
  const res = await request.post('/api/channels', {
    data: {
      name: payload.name,
      description: payload.description ?? '',
    },
  })
  expect(res.ok(), await res.text()).toBeTruthy()
  return res.json()
}

export async function updateChannelApi(
  request: APIRequestContext,
  channelId: string,
  payload: { name: string; description?: string }
): Promise<void> {
  const res = await request.patch(`/api/channels/${encodeURIComponent(channelId)}`, {
    data: {
      name: payload.name,
      description: payload.description ?? '',
    },
  })
  expect(res.ok(), await res.text()).toBeTruthy()
}

export async function deleteChannelApi(
  request: APIRequestContext,
  channelId: string
): Promise<void> {
  const res = await request.delete(`/api/channels/${encodeURIComponent(channelId)}`)
  expect(res.ok(), await res.text()).toBeTruthy()
}

export async function getChannelMembersApi(
  request: APIRequestContext,
  channelId: string
): Promise<ChannelMembersResponse> {
  const res = await request.get(`/api/channels/${encodeURIComponent(channelId)}/members`)
  expect(res.ok(), await res.text()).toBeTruthy()
  return res.json()
}

export async function inviteChannelMemberApi(
  request: APIRequestContext,
  channelId: string,
  memberName: string
): Promise<ChannelMembersResponse> {
  const res = await request.post(`/api/channels/${encodeURIComponent(channelId)}/members`, {
    data: { memberName },
  })
  expect(res.ok(), await res.text()).toBeTruthy()
  return res.json()
}

export async function rememberApi(
  request: APIRequestContext,
  agentName: string,
  payload: {
    key: string
    value: string
    tags?: string[]
    channelContext?: string
  }
): Promise<{ id: string }> {
  const res = await request.post(`/internal/agent/${encodeURIComponent(agentName)}/remember`, {
    data: payload,
  })
  expect(res.ok(), await res.text()).toBeTruthy()
  return res.json()
}

export async function recallApi(
  request: APIRequestContext,
  agentName: string,
  params?: { query?: string; tags?: string }
): Promise<{ entries: KnowledgeEntry[] }> {
  const search = new URLSearchParams()
  if (params?.query) search.set('query', params.query)
  if (params?.tags) search.set('tags', params.tags)
  const suffix = search.size > 0 ? `?${search.toString()}` : ''
  const res = await request.get(`/internal/agent/${encodeURIComponent(agentName)}/recall${suffix}`)
  expect(res.ok(), await res.text()).toBeTruthy()
  return res.json()
}

export async function getWorkspaceApi(
  request: APIRequestContext,
  agentName: string
): Promise<{ path: string; files: string[] }> {
  const res = await request.get(`/api/agents/${encodeURIComponent(agentName)}/workspace`)
  expect(res.ok(), await res.text()).toBeTruthy()
  return res.json()
}

export async function getWorkspaceFileApi(
  request: APIRequestContext,
  agentName: string,
  path: string
): Promise<{
  path: string
  content: string
  truncated: boolean
  sizeBytes: number
  modifiedMs?: number
}> {
  const params = new URLSearchParams({ path })
  const res = await request.get(
    `/api/agents/${encodeURIComponent(agentName)}/workspace/file?${params.toString()}`
  )
  expect(res.ok(), await res.text()).toBeTruthy()
  return res.json()
}

export async function teamExists(request: APIRequestContext, name: string): Promise<boolean> {
  const res = await request.get(`/api/teams/${encodeURIComponent(name)}`)
  return res.ok()
}

export async function createTeamApi(
  request: APIRequestContext,
  body: {
    name: string
    display_name: string
    collaboration_model: 'leader_operators' | 'swarm'
    leader_agent_name: string | null
    members: Array<{
      member_name: string
      member_type: 'agent' | 'human'
      member_id: string
      role: string
    }>
  }
): Promise<void> {
  const res = await request.post('/api/teams', { data: body })
  expect(res.ok(), await res.text()).toBeTruthy()
}
