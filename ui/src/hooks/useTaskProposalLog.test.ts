import { describe, it, expect } from 'vitest'
import { deriveTaskProposalStates } from './useTaskProposalLog'
import type { HistoryMessage } from '../data/chat'

function proposalMsg(
  seq: number,
  event: Record<string, unknown>,
): HistoryMessage {
  return {
    id: `m-${seq}`,
    seq,
    content: JSON.stringify({ kind: 'task_proposal', ...event }),
    senderName: 'system',
    senderType: 'system',
    createdAt: new Date(seq * 1000).toISOString(),
    senderDeleted: false,
  }
}

describe('deriveTaskProposalStates', () => {
  it('empty log returns empty index', () => {
    const idx = deriveTaskProposalStates([])
    expect(idx.byProposalId.size).toBe(0)
    expect(idx.proposalIdBySeq.size).toBe(0)
  })

  it('folds pending + accepted snapshots for same proposalId', () => {
    const msgs: HistoryMessage[] = [
      proposalMsg(1, {
        proposalId: 'p1',
        status: 'pending',
        title: 'fix login',
        proposedBy: 'claude',
        proposedAt: '2026-04-23T10:00:00Z',
      }),
      proposalMsg(2, {
        proposalId: 'p1',
        status: 'accepted',
        title: 'fix login',
        proposedBy: 'claude',
        proposedAt: '2026-04-23T10:00:00Z',
        taskNumber: 7,
        subChannelId: 's',
        subChannelName: 'eng__task-7',
      }),
    ]
    const idx = deriveTaskProposalStates(msgs)
    const p = idx.byProposalId.get('p1')!
    expect(p.status).toBe('accepted')
    expect(p.taskNumber).toBe(7)
    expect(p.latestSeq).toBe(2)
    expect(idx.proposalIdBySeq.get(1)).toBe('p1')
    expect(idx.proposalIdBySeq.get(2)).toBe('p1')
  })

  it('out-of-order snapshots apply in seq order', () => {
    const msgs: HistoryMessage[] = [
      proposalMsg(2, {
        proposalId: 'p1',
        status: 'accepted',
        title: 't',
        proposedBy: 'claude',
        proposedAt: 'x',
        taskNumber: 1,
        subChannelId: 's',
        subChannelName: 'eng__task-1',
      }),
      proposalMsg(1, {
        proposalId: 'p1',
        status: 'pending',
        title: 't',
        proposedBy: 'claude',
        proposedAt: 'x',
      }),
    ]
    expect(deriveTaskProposalStates(msgs).byProposalId.get('p1')!.status).toBe(
      'accepted',
    )
  })
})
