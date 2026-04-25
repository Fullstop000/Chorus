import { describe, it, expect } from 'vitest'
import { deriveTaskEventRows } from './useTaskEventLog'
import type { HistoryMessage } from '../data/chat'

function taskEventMsg(
  seq: number,
  event: Record<string, unknown>,
): HistoryMessage {
  return {
    id: `m-${seq}`,
    seq,
    content: JSON.stringify({ kind: 'task_event', ...event }),
    senderName: 'system',
    senderType: 'system',
    createdAt: new Date(seq * 1000).toISOString(),
    senderDeleted: false,
  }
}

describe('deriveTaskEventRows', () => {
  it('returns an empty array when no task events present', () => {
    const msgs: HistoryMessage[] = [
      {
        id: 'm1',
        seq: 1,
        content: 'hello',
        senderName: 'alice',
        senderType: 'human',
        createdAt: '2026-04-23T10:00:00Z',
        senderDeleted: false,
      },
    ]
    expect(deriveTaskEventRows(msgs)).toEqual([])
  })

  it('captures every task_event message in seq order', () => {
    const msgs: HistoryMessage[] = [
      taskEventMsg(2, {
        action: 'claimed',
        taskNumber: 7,
        title: 't',
        subChannelId: 's',
        actor: 'alice',
        prevStatus: 'todo',
        nextStatus: 'in_progress',
        claimedBy: 'alice',
      }),
      taskEventMsg(1, {
        action: 'created',
        taskNumber: 7,
        title: 't',
        subChannelId: 's',
        actor: 'alice',
        nextStatus: 'todo',
      }),
      taskEventMsg(3, {
        action: 'status_changed',
        taskNumber: 7,
        title: 't',
        subChannelId: 's',
        actor: 'alice',
        prevStatus: 'in_progress',
        nextStatus: 'in_review',
      }),
    ]
    const rows = deriveTaskEventRows(msgs)
    expect(rows.map((r) => r.seq)).toEqual([1, 2, 3])
    expect(rows.map((r) => r.payload.action)).toEqual([
      'created',
      'claimed',
      'status_changed',
    ])
  })

  it('skips messages whose content is not a parseable task_event', () => {
    const msgs: HistoryMessage[] = [
      taskEventMsg(1, {
        action: 'claimed',
        taskNumber: 7,
        title: 't',
        subChannelId: 's',
        actor: 'alice',
        prevStatus: 'todo',
        nextStatus: 'in_progress',
        claimedBy: 'alice',
      }),
      {
        id: 'm-bad',
        seq: 2,
        content: '{"kind":"some_other_thing"}',
        senderName: 'system',
        senderType: 'system',
        createdAt: '2026-04-23T10:00:00Z',
        senderDeleted: false,
      },
    ]
    const rows = deriveTaskEventRows(msgs)
    expect(rows).toHaveLength(1)
    expect(rows[0].payload.action).toBe('claimed')
  })
})
