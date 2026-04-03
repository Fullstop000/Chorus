import { describe, expect, it } from 'vitest'

import { normalizeEvent, upsertMessage, bumpReplyCount, historyFetchAfterForNotification } from './realtime'
import type { HistoryMessage, StreamEvent } from '../components/chat/types'

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

describe('historyFetchAfterForNotification', () => {
  it('refetches the active root conversation when a newer message arrives', () => {
    expect(
      historyFetchAfterForNotification('conversation:conversation-1', makeEvent(), 5, null)
    ).toBe(5)
  })

  it('only refetches an open thread when the incoming event belongs to that thread', () => {
    expect(
      historyFetchAfterForNotification(
        'conversation:conversation-1',
        makeEvent({
          payload: {
            messageId: 'reply-9',
            conversationId: 'conversation-1',
            conversationType: 'channel',
            threadParentId: 'thread-123',
          },
        }),
        5,
        'thread-123'
      )
    ).toBe(5)

    expect(
      historyFetchAfterForNotification(
        'conversation:conversation-1',
        makeEvent({
          latestSeq: 7,
          payload: {
            messageId: 'reply-10',
            conversationId: 'conversation-1',
            conversationType: 'channel',
            threadParentId: 'thread-other',
          },
        }),
        5,
        'thread-123'
      )
    ).toBeNull()
  })

  it('does not refetch root history for thread replies (they are omitted from root pages)', () => {
    expect(
      historyFetchAfterForNotification(
        'conversation:conversation-1',
        makeEvent({
          latestSeq: 9,
          payload: {
            messageId: 'reply-9',
            conversationId: 'conversation-1',
            conversationType: 'channel',
            threadParentId: 'thread-123',
          },
        }),
        5,
        null
      )
    ).toBeNull()
  })
})

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
      clientNonce: undefined,
    })
  })
})

describe('upsertMessage', () => {
  it('appends a new message', () => {
    const messages: HistoryMessage[] = [
      { id: 'm1', seq: 1, content: 'hello', senderName: 'a', senderType: 'human', senderDeleted: false, createdAt: 'T' },
    ]
    const incoming: HistoryMessage = { id: 'm2', seq: 2, content: 'world', senderName: 'b', senderType: 'human', senderDeleted: false, createdAt: 'T' }
    const result = upsertMessage(messages, incoming)
    expect(result).toHaveLength(2)
    expect(result[1].id).toBe('m2')
  })

  it('deduplicates by id', () => {
    const messages: HistoryMessage[] = [
      { id: 'm1', seq: 1, content: 'hello', senderName: 'a', senderType: 'human', senderDeleted: false, createdAt: 'T' },
    ]
    const incoming: HistoryMessage = { id: 'm1', seq: 1, content: 'updated', senderName: 'a', senderType: 'human', senderDeleted: false, createdAt: 'T' }
    expect(upsertMessage(messages, incoming)).toEqual(messages)
  })

  it('deduplicates by clientNonce', () => {
    const messages: HistoryMessage[] = [
      { id: 'temp:1', seq: 2, content: 'optimistic', senderName: 'a', senderType: 'human', senderDeleted: false, createdAt: 'T', clientNonce: 'nonce-abc' },
    ]
    const incoming: HistoryMessage = { id: 'real-uuid', seq: 2, content: 'confirmed', senderName: 'a', senderType: 'human', senderDeleted: false, createdAt: 'T', clientNonce: 'nonce-abc' }
    expect(upsertMessage(messages, incoming)).toEqual(messages)
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
