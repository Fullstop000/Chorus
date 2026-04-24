import { useMemo } from 'react'
import type { HistoryMessage } from '../data/chat'
import {
  parseTaskProposal,
  type TaskProposalPayload,
} from '../data/taskProposals'

export interface TaskProposalState extends TaskProposalPayload {
  /** seq of the latest snapshot applied — used for repeat-suppression. */
  latestSeq: number
}

export interface TaskProposalIndex {
  byProposalId: Map<string, TaskProposalState>
  /** Inverse index: seq → proposalId. Lets the render loop detect a
   *  task_proposal row in O(1) with no JSON re-parse. */
  proposalIdBySeq: Map<number, string>
}

export function deriveTaskProposalStates(
  messages: HistoryMessage[],
): TaskProposalIndex {
  const parsed: { msg: HistoryMessage; ev: TaskProposalPayload }[] = []
  for (const msg of messages) {
    if (msg.senderType !== 'system') continue
    const ev = parseTaskProposal(msg.content)
    if (!ev) continue
    parsed.push({ msg, ev })
  }
  parsed.sort((a, b) => a.msg.seq - b.msg.seq)

  const byProposalId = new Map<string, TaskProposalState>()
  const proposalIdBySeq = new Map<number, string>()

  for (const { msg, ev } of parsed) {
    proposalIdBySeq.set(msg.seq, ev.proposalId)
    const prev = byProposalId.get(ev.proposalId)
    if (!prev) {
      byProposalId.set(ev.proposalId, { ...ev, latestSeq: msg.seq })
    } else {
      // Mutate in place — the returned Map is built fresh per memo recompute.
      prev.status = ev.status
      prev.title = ev.title
      prev.taskNumber = ev.taskNumber
      prev.subChannelId = ev.subChannelId
      prev.subChannelName = ev.subChannelName
      prev.resolvedBy = ev.resolvedBy ?? prev.resolvedBy ?? null
      prev.resolvedAt = ev.resolvedAt ?? prev.resolvedAt ?? null
      // Snapshot fields are set once on the initial pending row and never
      // change across subsequent snapshots — but a later snapshot may still
      // carry them, so prefer incoming and fall back to prev for safety.
      prev.sourceMessageId = ev.sourceMessageId ?? prev.sourceMessageId ?? null
      prev.snapshotSenderName =
        ev.snapshotSenderName ?? prev.snapshotSenderName ?? null
      prev.snapshotExcerpt = ev.snapshotExcerpt ?? prev.snapshotExcerpt ?? null
      prev.snapshotCreatedAt =
        ev.snapshotCreatedAt ?? prev.snapshotCreatedAt ?? null
      prev.latestSeq = msg.seq
    }
  }
  return { byProposalId, proposalIdBySeq }
}

export function useTaskProposalLog(
  messages: HistoryMessage[],
): TaskProposalIndex {
  return useMemo(() => deriveTaskProposalStates(messages), [messages])
}
