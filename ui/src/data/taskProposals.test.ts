import { describe, it, expect } from 'vitest'
import { parseTaskProposal } from './taskProposals'

describe('parseTaskProposal', () => {
  it('parses a pending snapshot', () => {
    const p = parseTaskProposal(
      JSON.stringify({
        kind: 'task_proposal',
        proposalId: 'p1',
        status: 'pending',
        title: 'fix login',
        proposedBy: 'claude',
        proposedAt: '2026-04-23T10:00:00Z',
        taskNumber: null,
        subChannelId: null,
        subChannelName: null,
      }),
    )
    expect(p).not.toBeNull()
    expect(p!.proposalId).toBe('p1')
    expect(p!.status).toBe('pending')
    expect(p!.taskNumber).toBeNull()
  })

  it('parses an accepted snapshot with task coords', () => {
    const p = parseTaskProposal(
      JSON.stringify({
        kind: 'task_proposal',
        proposalId: 'p1',
        status: 'accepted',
        title: 'fix login',
        proposedBy: 'claude',
        proposedAt: '2026-04-23T10:00:00Z',
        taskNumber: 7,
        subChannelId: 'sub1',
        subChannelName: 'eng__task-7',
      }),
    )
    expect(p).not.toBeNull()
    expect(p!.status).toBe('accepted')
    expect(p!.taskNumber).toBe(7)
    expect(p!.subChannelName).toBe('eng__task-7')
  })

  it('returns null for a non-task-proposal kind', () => {
    expect(
      parseTaskProposal(
        JSON.stringify({ kind: 'task_event', taskNumber: 1 }),
      ),
    ).toBeNull()
  })

  it('returns null for malformed JSON', () => {
    expect(parseTaskProposal('not json')).toBeNull()
  })

  it('returns null when status is unknown', () => {
    expect(
      parseTaskProposal(
        JSON.stringify({
          kind: 'task_proposal',
          proposalId: 'p1',
          status: 'weird',
          title: 't',
          proposedBy: 'claude',
          proposedAt: '2026-04-23T10:00:00Z',
        }),
      ),
    ).toBeNull()
  })

  it('rejects missing proposalId', () => {
    expect(
      parseTaskProposal(
        JSON.stringify({
          kind: 'task_proposal',
          status: 'pending',
          title: 't',
          proposedBy: 'claude',
          proposedAt: '2026-04-23T10:00:00Z',
        }),
      ),
    ).toBeNull()
  })
})
