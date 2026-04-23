import { describe, it, expect } from 'vitest'
import { parseTaskEvent } from './taskEvents'

describe('parseTaskEvent', () => {
  it('parses a well-formed task_event JSON string', () => {
    const content = JSON.stringify({
      kind: 'task_event',
      action: 'claimed',
      taskNumber: 7,
      title: 'wire up the bridge',
      subChannelId: 'sub-1',
      actor: 'alice',
      prevStatus: 'todo',
      nextStatus: 'in_progress',
      claimedBy: 'alice',
    })
    const parsed = parseTaskEvent(content)
    expect(parsed).not.toBeNull()
    expect(parsed!.action).toBe('claimed')
    expect(parsed!.taskNumber).toBe(7)
    expect(parsed!.nextStatus).toBe('in_progress')
  })

  it('returns null for non-JSON content', () => {
    expect(parseTaskEvent('hello world')).toBeNull()
  })

  it('returns null when kind is not task_event', () => {
    expect(parseTaskEvent(JSON.stringify({ kind: 'other' }))).toBeNull()
  })

  it('returns null when required fields are missing', () => {
    expect(
      parseTaskEvent(JSON.stringify({ kind: 'task_event', action: 'claimed' })),
    ).toBeNull()
  })

  it('returns null for an unknown action', () => {
    const content = JSON.stringify({
      kind: 'task_event',
      action: 'deleted',
      taskNumber: 1,
      title: 't',
      subChannelId: 's',
      actor: 'a',
      nextStatus: 'todo',
    })
    expect(parseTaskEvent(content)).toBeNull()
  })

  it('returns null when prevStatus is present but not a valid status', () => {
    const content = JSON.stringify({
      kind: 'task_event',
      action: 'claimed',
      taskNumber: 1,
      title: 't',
      subChannelId: 's',
      actor: 'a',
      prevStatus: 'garbage',
      nextStatus: 'in_progress',
    })
    expect(parseTaskEvent(content)).toBeNull()
  })

  it('returns null when taskNumber is not an integer', () => {
    const content = JSON.stringify({
      kind: 'task_event',
      action: 'created',
      taskNumber: 1.5,
      title: 't',
      subChannelId: 's',
      actor: 'a',
      nextStatus: 'todo',
    })
    expect(parseTaskEvent(content)).toBeNull()
  })

  it('returns null when claimedBy is present but wrong type', () => {
    const content = JSON.stringify({
      kind: 'task_event',
      action: 'claimed',
      taskNumber: 1,
      title: 't',
      subChannelId: 's',
      actor: 'a',
      nextStatus: 'in_progress',
      claimedBy: 42,
    })
    expect(parseTaskEvent(content)).toBeNull()
  })
})
