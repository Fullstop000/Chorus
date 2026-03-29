import { describe, expect, it } from 'vitest'

import {
  applyRealtimeEvent,
  historyFetchAfterForNotification,
  nextRealtimeCursor,
} from './realtime'
import type { HistoryMessage, RealtimeEvent, RealtimeMessage } from '../types'

describe('realtime transport helpers', () => {
  it('treats message.created bus frames as notification-only', () => {
    const messages: HistoryMessage[] = []
    const event: RealtimeEvent = {
      eventId: 7,
      eventType: 'message.created',
      scopeKind: 'channel',
      scopeId: 'channel:abc',
      payload: {
        messageId: 'msg-1',
        content: 'hello',
        sender: { name: 'alice', type: 'human' },
        senderDeleted: true,
        forwardedFrom: {
          channelName: 'eng-team',
          senderName: 'bob',
        },
        seq: 1,
        createdAt: '2026-03-28T00:00:00Z',
      },
      createdAt: '2026-03-28T00:00:00Z',
    }

    expect(applyRealtimeEvent(messages, event)).toEqual([])
  })

  it('expects conversation.state notifications to carry absolute unread state', () => {
    const event: RealtimeEvent = {
      eventId: 8,
      eventType: 'conversation.state',
      scopeKind: 'channel',
      scopeId: 'channel:abc',
      payload: {
        conversationId: 'channel:abc',
        latestSeq: 12,
        lastReadSeq: 9,
        unreadCount: 3,
      },
      createdAt: '2026-03-28T00:00:00Z',
    }

    expect(event.eventType).toBe('conversation.state')
    expect(event.payload).not.toHaveProperty('content')
    expect(event.payload).toMatchObject({
      conversationId: 'channel:abc',
      latestSeq: 12,
      lastReadSeq: 9,
      unreadCount: 3,
    })
  })

  it('requests incremental history when the active conversation notification advances', () => {
    const event: RealtimeEvent = {
      eventId: 9,
      eventType: 'conversation.state',
      streamId: 'conversation:abc',
      scopeKind: 'channel',
      scopeId: 'channel:abc',
      payload: {
        conversationId: 'channel:abc',
        latestSeq: 12,
        lastReadSeq: 9,
        unreadCount: 3,
      },
      createdAt: '2026-03-28T00:00:00Z',
    }

    expect(historyFetchAfterForNotification('conversation:abc', event, 9)).toBe(9)
  })

  it('ignores stale or inactive notification frames for incremental fetches', () => {
    const conversationEvent: RealtimeEvent = {
      eventId: 10,
      eventType: 'conversation.state',
      streamId: 'conversation:def',
      scopeKind: 'channel',
      scopeId: 'channel:def',
      payload: {
        conversationId: 'channel:def',
        latestSeq: 4,
        lastReadSeq: 1,
        unreadCount: 3,
      },
      createdAt: '2026-03-28T00:00:00Z',
    }
    const threadEvent: RealtimeEvent = {
      eventId: 11,
      eventType: 'thread.state',
      streamId: 'conversation:abc',
      scopeKind: 'thread',
      scopeId: 'thread:msg-2',
      threadParentId: 'msg-2',
      payload: {
        conversationId: 'channel:abc',
        threadParentId: 'msg-2',
        latestSeq: 8,
        lastReadSeq: 2,
        unreadCount: 6,
      },
      createdAt: '2026-03-28T00:00:00Z',
    }

    expect(historyFetchAfterForNotification('conversation:abc', conversationEvent, 4)).toBeNull()
    expect(historyFetchAfterForNotification('conversation:def', conversationEvent, 4)).toBeNull()
    expect(historyFetchAfterForNotification('thread:msg-1', threadEvent, 2)).toBeNull()
  })

  it('trusts subscribed resumeFrom as the authoritative cursor', () => {
    const frame: RealtimeMessage = {
      type: 'subscribed',
      resumeFrom: 3,
      targets: [],
    }

    expect(nextRealtimeCursor(10, frame)).toBe(3)
  })

  it('preserves stream resume metadata on subscribed frames', () => {
    const frame: RealtimeMessage = {
      type: 'subscribed',
      resumeFrom: 0,
      streamId: 'conversation:abc',
      resumeFromStreamPos: 7,
      targets: ['conversation:abc'],
    }

    expect(frame.targets).toEqual(['conversation:abc'])
    expect(frame.streamId).toBe('conversation:abc')
    expect(frame.resumeFromStreamPos).toBe(7)
  })
})
