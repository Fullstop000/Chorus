import { describe, expect, it } from 'vitest'

import { normalizeEvent, bumpReplyCount } from './messages'
import type { HistoryMessage, StreamEvent } from './chat'

function makeEvent(overrides: Partial<StreamEvent> = {}): StreamEvent {
  return {
    eventType: 'message.created',
    channelId: 'conversation-1',
    latestSeq: 6,
    payload: {
      messageId: 'message-6',
      conversationId: 'conversation-1',
      conversationType: 'channel',
      threadParentId: null,
    },
    schemaVersion: 1,
    ...overrides,
  }
}

describe('normalizeEvent', () => {
  it('returns null for non-message events', () => {
    const event: StreamEvent = { ...makeEvent(), eventType: 'tombstone_changed' }
    expect(normalizeEvent(event)).toBeNull()
  })

  it('returns null when required fields are missing', () => {
    expect(normalizeEvent({ ...makeEvent(), payload: {} })).toBeNull()
  })

  it('converts a valid WS event into a HistoryMessage', () => {
    const msg = normalizeEvent(makeEvent({
      payload: {
        messageId: 'msg-1',
        conversationId: 'conv-1',
        conversationType: 'channel',
        threadParentId: null,
        content: 'hello',
        sender: { name: 'alice', type: 'human' },
        seq: 10,
        createdAt: '2025-01-01T00:00:00Z',
      },
    }))
    expect(msg).toEqual({
      id: 'msg-1',
      seq: 10,
      content: 'hello',
      senderName: 'alice',
      senderType: 'human',
      senderDeleted: false,
      createdAt: '2025-01-01T00:00:00Z',
    })
  })
})

describe('bumpReplyCount', () => {
  it('increments replyCount on the parent message', () => {
    const messages: HistoryMessage[] = [
      { id: 'parent-1', seq: 3, content: 'root', senderName: 'alice', senderType: 'human', senderDeleted: false, createdAt: '2025-01-01T00:00:00Z', replyCount: 2 },
    ]
    const result = bumpReplyCount(messages, 'parent-1')
    expect(result[0]?.replyCount).toBe(3)
  })

  it('initializes replyCount from undefined to 1', () => {
    const messages: HistoryMessage[] = [
      { id: 'parent-1', seq: 3, content: 'root', senderName: 'alice', senderType: 'human', senderDeleted: false, createdAt: '2025-01-01T00:00:00Z' },
    ]
    const result = bumpReplyCount(messages, 'parent-1')
    expect(result[0]?.replyCount).toBe(1)
  })

  it('leaves other messages untouched', () => {
    const messages: HistoryMessage[] = [
      { id: 'other-1', seq: 1, content: 'other', senderName: 'bob', senderType: 'human', senderDeleted: false, createdAt: 'T' },
      { id: 'parent-1', seq: 3, content: 'root', senderName: 'alice', senderType: 'human', senderDeleted: false, createdAt: 'T' },
    ]
    const result = bumpReplyCount(messages, 'parent-1')
    expect(result[0].id).toBe('other-1')
    expect(result[0]).not.toHaveProperty('replyCount')
  })
})
