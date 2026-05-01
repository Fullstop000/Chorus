import { get, post } from './client'

// Public shape of a decision option as the agent submitted it.
export type DecisionOption = {
  key: string
  label: string
  body: string
}

export type DecisionPayload = {
  headline: string
  question: string
  options: DecisionOption[]
  recommended_key: string
  context: string | null
}

export type DecisionStatus = 'open' | 'resolved'

export type DecisionView = {
  id: string
  agent_id: string
  agent_name: string
  channel_id: string
  channel_name: string
  created_at: string
  status: DecisionStatus
  payload: DecisionPayload
  picked_key: string | null
  picked_note: string | null
  resolved_at: string | null
}

export type ListDecisionsResponse = {
  decisions: DecisionView[]
}

export type ResolveDecisionResponse = {
  decision_id: string
  status: DecisionStatus
}

export type DecisionStatusFilter = 'open' | 'resolved' | 'all'

export async function listDecisions(
  status: DecisionStatusFilter = 'open',
): Promise<ListDecisionsResponse> {
  return get<ListDecisionsResponse>(`/api/decisions?status=${status}`)
}

export async function resolveDecision(
  decisionId: string,
  pickedKey: string,
  note?: string,
): Promise<ResolveDecisionResponse> {
  return post<ResolveDecisionResponse>(`/api/decisions/${decisionId}/resolve`, {
    picked_key: pickedKey,
    note,
  })
}
