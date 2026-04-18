import type { APIRequestContext } from '@playwright/test'
import { expect } from '@playwright/test'

export interface AgentRow {
  id: string
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

interface TeamRow {
  id: string
  name: string
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

async function requireAgentId(
  request: APIRequestContext,
  agentName: string
): Promise<string> {
  const agent = (await listAgents(request)).find((entry) => entry.name === agentName)
  if (!agent) {
    throw new Error(`Agent not found: ${agentName}`)
  }
  return agent.id
}

async function listTeams(request: APIRequestContext): Promise<TeamRow[]> {
  const res = await request.get('/api/teams')
  expect(res.ok(), await res.text()).toBeTruthy()
  return res.json()
}

export async function getAgentDetail(request: APIRequestContext, name: string): Promise<AgentDetail> {
  const agentId = await requireAgentId(request, name)
  const res = await request.get(`/api/agents/${encodeURIComponent(agentId)}`)
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
  },
  options?: {
    allowNameTaken?: boolean
  }
): Promise<{ name: string }> {
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
  if (res.ok()) {
    const json = await res.json() as { name: string }
    return { name: json.name }
  }

  const text = await res.text()
  if (options?.allowNameTaken) {
    const parsed = JSON.parse(text) as { error?: string; code?: string }
    if (
      parsed.code === 'AGENT_NAME_TAKEN' ||
      /agent name already in use/i.test(parsed.error ?? text)
    ) {
      return { name: body.name }
    }
  }

  expect(res.ok(), text).toBeTruthy()
  return { name: body.name }
}

/** Find the first agent whose name starts with `prefix`. */
export function findAgentByPrefix(agents: AgentRow[], prefix: string): AgentRow | undefined {
  return agents.find((a) => a.name === prefix || a.name.startsWith(`${prefix}-`))
}

/** Like requireAgentId but matches by name prefix (handles server-added suffixes). */
export async function requireAgentByPrefix(
  request: APIRequestContext,
  prefix: string
): Promise<AgentRow> {
  const agents = await listAgents(request)
  const agent = findAgentByPrefix(agents, prefix)
  if (!agent) throw new Error(`Agent not found with prefix: ${prefix}`)
  return agent
}

export interface TrioNames {
  botA: string; botB: string; botC: string          // API names (server-suffixed)
  displayA: string; displayB: string; displayC: string  // sidebar display names (original)
}

/** API precondition helper only — catalog AGT-001 still requires UI creation when run for that case.
 * Trio: bot-a=codex/gpt-5.4, bot-b=kimi/kimi-code/kimi-for-coding, bot-c=opencode/opencode/gpt-5-nano
 *
 * Agent names may be suffixed by the server (e.g. bot-a → bot-a-279b).
 * Returns the actual server-assigned names.
 */
export async function ensureMixedRuntimeTrio(request: APIRequestContext): Promise<TrioNames> {
  let agents = await listAgents(request)

  let botA = findAgentByPrefix(agents, 'bot-a')
  if (!botA) {
    const { name } = await createAgentApi(request, { name: 'bot-a', runtime: 'codex', model: 'gpt-5.4' })
    agents = await listAgents(request)
    botA = findAgentByPrefix(agents, 'bot-a') ?? { name, id: '', status: '' } as AgentRow
  }

  let botB = findAgentByPrefix(agents, 'bot-b')
  if (!botB) {
    const { name } = await createAgentApi(request, { name: 'bot-b', runtime: 'kimi', model: 'kimi-code/kimi-for-coding' })
    agents = await listAgents(request)
    botB = findAgentByPrefix(agents, 'bot-b') ?? { name, id: '', status: '' } as AgentRow
  }

  let botC = findAgentByPrefix(agents, 'bot-c')
  if (!botC) {
    const { name } = await createAgentApi(request, { name: 'bot-c', runtime: 'opencode', model: 'opencode/gpt-5-nano' })
    agents = await listAgents(request)
    botC = findAgentByPrefix(agents, 'bot-c') ?? { name, id: '', status: '' } as AgentRow
  }

  return {
    botA: botA.name, botB: botB.name, botC: botC.name,
    displayA: botA.display_name ?? 'bot-a',
    displayB: botB.display_name ?? 'bot-b',
    displayC: botC.display_name ?? 'bot-c',
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
  const agentId = await requireAgentId(request, name)
  const res = await request.post(`/api/agents/${encodeURIComponent(agentId)}/start`)
  expect(res.ok(), await res.text()).toBeTruthy()
}

export async function stopAgentApi(request: APIRequestContext, name: string): Promise<void> {
  const agentId = await requireAgentId(request, name)
  const res = await request.post(`/api/agents/${encodeURIComponent(agentId)}/stop`)
  expect(res.ok(), await res.text()).toBeTruthy()
}

export async function restartAgentApi(
  request: APIRequestContext,
  name: string,
  mode: 'restart' | 'reset_session' | 'full_reset'
): Promise<void> {
  const agentId = await requireAgentId(request, name)
  const res = await request.post(`/api/agents/${encodeURIComponent(agentId)}/restart`, {
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
  const agentId = await requireAgentId(request, name)
  const res = await request.patch(`/api/agents/${encodeURIComponent(agentId)}`, {
    data: payload,
  })
  expect(res.ok(), await res.text()).toBeTruthy()
}

export async function deleteAgentApi(
  request: APIRequestContext,
  name: string,
  mode: 'preserve_workspace' | 'delete_workspace'
): Promise<void> {
  const agentId = await requireAgentId(request, name)
  const res = await request.post(`/api/agents/${encodeURIComponent(agentId)}/delete`, {
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
  const agentId = await requireAgentId(request, name)
  const res = await request.get(`/api/agents/${encodeURIComponent(agentId)}/activity-log`)
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

export async function getWorkspaceApi(
  request: APIRequestContext,
  agentName: string
): Promise<{ path: string; files: string[] }> {
  const agentId = await requireAgentId(request, agentName)
  const res = await request.get(`/api/agents/${encodeURIComponent(agentId)}/workspace`)
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
  const agentId = await requireAgentId(request, agentName)
  const params = new URLSearchParams({ path })
  const res = await request.get(
    `/api/agents/${encodeURIComponent(agentId)}/workspace/file?${params.toString()}`
  )
  expect(res.ok(), await res.text()).toBeTruthy()
  return res.json()
}

export async function teamExists(request: APIRequestContext, name: string): Promise<boolean> {
  const team = (await listTeams(request)).find((entry) => entry.name === name)
  if (!team) return false
  const res = await request.get(`/api/teams/${encodeURIComponent(team.id)}`)
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

/**
 * Generic polling helper. `fn` returns a resolved value when the condition is
 * met, or `undefined` to keep waiting. Rejects with a timeout error on expiry.
 */
export async function pollUntil<T>(
  fn: () => Promise<T | undefined>,
  timeoutMs: number,
  intervalMs = 4_000
): Promise<T> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const result = await fn()
    if (result !== undefined) return result
    await new Promise((r) => setTimeout(r, intervalMs))
  }
  throw new Error(`pollUntil: timed out after ${timeoutMs}ms`)
}

/**
 * Ensure a dedicated isolated channel exists and all listed agents are
 * members. Idempotent: if the channel already exists it is left as-is.
 * The calling user is automatically a member as the channel creator.
 */
export async function ensureIsolatedChannel(
  request: APIRequestContext,
  channelName: string,
  agentNames: string[]
): Promise<void> {
  const channels = await listChannelsApi(request)
  if (channels.some((c) => c.name === channelName)) return

  const { id } = await createChannelApi(request, { name: channelName })
  for (const agentName of agentNames) {
    await inviteChannelMemberApi(request, id, agentName).catch(() => {
      // member already present — safe to ignore
    })
  }
}
