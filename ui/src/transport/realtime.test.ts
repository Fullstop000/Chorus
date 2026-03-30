import { describe, expect, it } from 'vitest'

import { historyFetchAfterForNotification } from './realtime'
import type { StreamEvent } from '../types'

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
})
