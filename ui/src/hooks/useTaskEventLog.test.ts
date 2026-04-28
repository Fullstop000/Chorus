import { describe, it, expect } from 'vitest'
import { deriveTaskStates } from './useTaskEventLog'
import type { HistoryMessage, MessagePayload } from '../data/chat'

function taskEventMsg(
  seq: number,
  event: Record<string, unknown>,
): HistoryMessage {
  return {
    id: `m-${seq}`,
    seq,
    content: 'task event',
    senderName: 'system',
    senderType: 'system',
    createdAt: new Date(seq * 1000).toISOString(),
    senderDeleted: false,
    payload: { kind: 'task_event', ...event } as MessagePayload,
  }
}

describe('deriveTaskStates', () => {
  it('returns empty map when no task events present', () => {
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
    const index = deriveTaskStates(msgs)
    expect(index.byTaskNumber.size).toBe(0)
    expect(index.taskNumberBySeq.size).toBe(0)
  })

  it('converges to the correct state after create → claim → in_review → done', () => {
    const msgs: HistoryMessage[] = [
      taskEventMsg(1, {
        action: 'created',
        taskNumber: 7,
        title: 't',
        subChannelId: 's',
        actor: 'alice',
        nextStatus: 'todo',
      }),
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
      taskEventMsg(3, {
        action: 'status_changed',
        taskNumber: 7,
        title: 't',
        subChannelId: 's',
        actor: 'alice',
        prevStatus: 'in_progress',
        nextStatus: 'in_review',
      }),
      taskEventMsg(4, {
        action: 'status_changed',
        taskNumber: 7,
        title: 't',
        subChannelId: 's',
        actor: 'alice',
        prevStatus: 'in_review',
        nextStatus: 'done',
      }),
    ]
    const index = deriveTaskStates(msgs)
    const task = index.byTaskNumber.get(7)!
    expect(task.status).toBe('done')
    expect(task.claimedBy).toBe('alice')
    expect(task.events).toHaveLength(4)
    expect(task.title).toBe('t')
    // Inverse index covers every emitted task_event seq.
    expect(index.taskNumberBySeq.get(1)).toBe(7)
    expect(index.taskNumberBySeq.get(4)).toBe(7)
  })

  it('applies events in seq order regardless of array order', () => {
    const msgs: HistoryMessage[] = [
      taskEventMsg(3, {
        action: 'status_changed',
        taskNumber: 1,
        title: 't',
        subChannelId: 's',
        actor: 'a',
        prevStatus: 'in_progress',
        nextStatus: 'in_review',
      }),
      taskEventMsg(1, {
        action: 'created',
        taskNumber: 1,
        title: 't',
        subChannelId: 's',
        actor: 'a',
        nextStatus: 'todo',
      }),
      taskEventMsg(2, {
        action: 'claimed',
        taskNumber: 1,
        title: 't',
        subChannelId: 's',
        actor: 'a',
        prevStatus: 'todo',
        nextStatus: 'in_progress',
        claimedBy: 'a',
      }),
    ]
    const index = deriveTaskStates(msgs)
    expect(index.byTaskNumber.get(1)!.status).toBe('in_review')
  })

  it('handles unclaimed by clearing claimedBy', () => {
    const msgs: HistoryMessage[] = [
      taskEventMsg(1, {
        action: 'created',
        taskNumber: 1,
        title: 't',
        subChannelId: 's',
        actor: 'a',
        nextStatus: 'todo',
      }),
      taskEventMsg(2, {
        action: 'claimed',
        taskNumber: 1,
        title: 't',
        subChannelId: 's',
        actor: 'a',
        nextStatus: 'in_progress',
        claimedBy: 'a',
      }),
      taskEventMsg(3, {
        action: 'unclaimed',
        taskNumber: 1,
        title: 't',
        subChannelId: 's',
        actor: 'a',
        nextStatus: 'todo',
        claimedBy: null,
      }),
    ]
    const index = deriveTaskStates(msgs)
    expect(index.byTaskNumber.get(1)!.claimedBy).toBeNull()
    expect(index.byTaskNumber.get(1)!.status).toBe('todo')
  })
})
