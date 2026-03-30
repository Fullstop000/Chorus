import { describe, expect, it } from 'vitest'

import { applyRealtimeEvent, historyFetchAfterForNotification } from './realtime'
import type { HistoryMessage, StreamEvent } from '../types'

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

describe('applyRealtimeEvent', () => {
  it('increments replyCount on the thread parent when a thread reply is created', () => {
    const messages: HistoryMessage[] = [
      {
        id: 'parent-1',
        seq: 3,
        content: 'root',
        senderName: 'alice',
        senderType: 'human',
        senderDeleted: false,
        createdAt: '2025-01-01T00:00:00Z',
        replyCount: 2,
      },
    ]
    const event = makeEvent({
      latestSeq: 5,
      payload: {
        messageId: 'reply-1',
        conversationId: 'conversation-1',
        conversationType: 'channel',
        threadParentId: 'parent-1',
      },
    })
    const next = applyRealtimeEvent(messages, event)
    expect(next[0]?.replyCount).toBe(3)
  })
})
