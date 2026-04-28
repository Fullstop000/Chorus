import { describe, it, expect } from 'vitest'
import { parseTaskEvent } from './taskEvents'
import type { MessagePayload } from './chat'

describe('parseTaskEvent', () => {
  it('parses a well-formed task_event payload', () => {
    const payload: MessagePayload = {
      kind: 'task_event',
      action: 'claimed',
      taskNumber: 7,
      title: 'wire up the bridge',
      subChannelId: 'sub-1',
      actor: 'alice',
      prevStatus: 'todo',
      nextStatus: 'in_progress',
      claimedBy: 'alice',
    }
    const parsed = parseTaskEvent(payload)
    expect(parsed).not.toBeNull()
    expect(parsed!.action).toBe('claimed')
    expect(parsed!.taskNumber).toBe(7)
    expect(parsed!.nextStatus).toBe('in_progress')
  })

  it('returns null when payload is undefined', () => {
    expect(parseTaskEvent(undefined)).toBeNull()
  })

  it('returns null when kind is not task_event', () => {
    expect(parseTaskEvent({ kind: 'other' } as MessagePayload)).toBeNull()
  })

  it('returns null when required fields are missing', () => {
    expect(
      parseTaskEvent({ kind: 'task_event', action: 'claimed' } as MessagePayload),
    ).toBeNull()
  })

  it('returns null for an unknown action', () => {
    const payload: MessagePayload = {
      kind: 'task_event',
      action: 'deleted',
      taskNumber: 1,
      title: 't',
      subChannelId: 's',
      actor: 'a',
      nextStatus: 'todo',
    }
    expect(parseTaskEvent(payload)).toBeNull()
  })

  it('returns null when prevStatus is present but not a valid status', () => {
    const payload: MessagePayload = {
      kind: 'task_event',
      action: 'claimed',
      taskNumber: 1,
      title: 't',
      subChannelId: 's',
      actor: 'a',
      prevStatus: 'garbage',
      nextStatus: 'in_progress',
    }
    expect(parseTaskEvent(payload)).toBeNull()
  })

  it('returns null when taskNumber is not an integer', () => {
    const payload: MessagePayload = {
      kind: 'task_event',
      action: 'created',
      taskNumber: 1.5,
      title: 't',
      subChannelId: 's',
      actor: 'a',
      nextStatus: 'todo',
    }
    expect(parseTaskEvent(payload)).toBeNull()
  })

  it('returns null when claimedBy is present but wrong type', () => {
    const payload: MessagePayload = {
      kind: 'task_event',
      action: 'claimed',
      taskNumber: 1,
      title: 't',
      subChannelId: 's',
      actor: 'a',
      nextStatus: 'in_progress',
      claimedBy: 42,
    }
    expect(parseTaskEvent(payload)).toBeNull()
  })
})
