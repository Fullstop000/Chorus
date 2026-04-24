import { useMutation } from '@tanstack/react-query'

export type TaskProposalStatus = 'pending' | 'accepted' | 'dismissed'

export interface TaskProposalPayload {
  proposalId: string
  status: TaskProposalStatus
  title: string
  proposedBy: string
  proposedAt: string
  taskNumber: number | null
  subChannelId: string | null
  subChannelName: string | null
  resolvedBy?: string | null
  resolvedAt?: string | null
  // v2 additions — nullable (legacy v1 proposals have none):
  sourceMessageId: string | null
  snapshotSenderName: string | null
  snapshotExcerpt: string | null
  snapshotCreatedAt: string | null
}

function isStatus(v: unknown): v is TaskProposalStatus {
  return v === 'pending' || v === 'accepted' || v === 'dismissed'
}

/**
 * Strict parser for task_proposal chat message content. Rejects malformed
 * payloads by returning null rather than throwing — matches parseTaskEvent.
 * Present-but-null on taskNumber/subChannelId/subChannelName is allowed
 * (that's the pending-state shape).
 */
export function parseTaskProposal(content: string): TaskProposalPayload | null {
  let v: unknown
  try {
    v = JSON.parse(content)
  } catch {
    return null
  }
  if (typeof v !== 'object' || v === null) return null
  const o = v as Record<string, unknown>
  if (o.kind !== 'task_proposal') return null
  if (typeof o.proposalId !== 'string') return null
  if (!isStatus(o.status)) return null
  if (typeof o.title !== 'string') return null
  if (typeof o.proposedBy !== 'string') return null
  if (typeof o.proposedAt !== 'string') return null

  const taskNumber =
    o.taskNumber === null || o.taskNumber === undefined
      ? null
      : typeof o.taskNumber === 'number' && Number.isInteger(o.taskNumber)
        ? o.taskNumber
        : undefined
  if (taskNumber === undefined) return null

  const subChannelId =
    o.subChannelId === null || o.subChannelId === undefined
      ? null
      : typeof o.subChannelId === 'string'
        ? o.subChannelId
        : undefined
  if (subChannelId === undefined) return null

  const subChannelName =
    o.subChannelName === null || o.subChannelName === undefined
      ? null
      : typeof o.subChannelName === 'string'
        ? o.subChannelName
        : undefined
  if (subChannelName === undefined) return null

  return {
    proposalId: o.proposalId,
    status: o.status,
    title: o.title,
    proposedBy: o.proposedBy,
    proposedAt: o.proposedAt,
    taskNumber,
    subChannelId,
    subChannelName,
    resolvedBy: typeof o.resolvedBy === 'string' ? o.resolvedBy : null,
    resolvedAt: typeof o.resolvedAt === 'string' ? o.resolvedAt : null,
    // v2 snapshot fields — missing or wrong-typed coerce to null. Keep the
    // graceful-degradation shape from the v1 fields above; tightening is a
    // post-merge follow-up, not this PR.
    sourceMessageId:
      typeof o.sourceMessageId === 'string' ? o.sourceMessageId : null,
    snapshotSenderName:
      typeof o.snapshotSenderName === 'string' ? o.snapshotSenderName : null,
    snapshotExcerpt:
      typeof o.snapshotExcerpt === 'string' ? o.snapshotExcerpt : null,
    snapshotCreatedAt:
      typeof o.snapshotCreatedAt === 'string' ? o.snapshotCreatedAt : null,
  }
}

// ── API hooks ──
// No read-hook: proposal state flows via the message-log fold, not a
// dedicated query. So no `useQueryClient().invalidateQueries(...)` on
// mutation success — there's nothing to invalidate. The card re-renders
// when the acceptance/dismissal snapshot message arrives via the existing
// message stream.

interface AcceptBody {
  accepter: string
}
interface DismissBody {
  resolver: string
}
interface AcceptResponse {
  taskNumber: number
  subChannelId: string
  subChannelName: string
}

export function useAcceptTaskProposal(proposalId: string) {
  return useMutation({
    mutationFn: async (body: AcceptBody): Promise<AcceptResponse> => {
      const r = await fetch(`/api/task-proposals/${proposalId}/accept`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      })
      if (!r.ok) throw new Error(`accept failed: ${r.status}`)
      return r.json()
    },
  })
}

export function useDismissTaskProposal(proposalId: string) {
  return useMutation({
    mutationFn: async (body: DismissBody): Promise<void> => {
      const r = await fetch(`/api/task-proposals/${proposalId}/dismiss`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      })
      if (!r.ok) throw new Error(`dismiss failed: ${r.status}`)
    },
  })
}
