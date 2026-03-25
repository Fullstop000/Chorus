import type { APIRequestContext } from '@playwright/test'
import { expect } from '@playwright/test'

export interface AgentRow {
  name: string
  status: string
  display_name?: string
  runtime?: string
  model?: string
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

export async function createAgentApi(
  request: APIRequestContext,
  body: {
    name: string
    runtime: string
    model: string
    display_name?: string
    description?: string
  }
): Promise<void> {
  const res = await request.post('/api/agents', {
    data: {
      name: body.name,
      display_name: body.display_name ?? body.name,
      description: body.description ?? 'qa playwright seed',
      runtime: body.runtime,
      model: body.model,
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
  senderName?: string
  senderType?: string
  content?: string
  forwardedFrom?: unknown
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
