import { describe, expect, it } from 'vitest'

import { applyInboxEvent, buildConversationRegistry, createInboxState, threadNotificationKey } from './inbox'
import type { AgentInfo, RealtimeEvent } from './types'

describe('inbox notification state', () => {
  it('builds a conversation registry for visible channels and existing agent dms', () => {
    const registry = buildConversationRegistry({
      currentUser: 'alice',
      systemChannels: [
        {
          id: 'sys-1',
          name: 'all',
          joined: true,
          channel_type: 'system',
        },
      ],
      channels: [
        {
          id: 'ch-1',
          name: 'eng',
          joined: true,
          channel_type: 'channel',
        },
      ],
      dmChannels: [
        {
          id: 'dm-1',
          name: 'dm-alice-bot1',
          joined: true,
          channel_type: 'dm',
        },
      ],
      agents: [
        {
          name: 'bot1',
          status: 'sleeping',
        } satisfies AgentInfo,
      ],
    })

    expect(registry).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          conversationId: 'sys-1',
          target: '#all',
        }),
        expect.objectContaining({
          conversationId: 'ch-1',
          target: '#eng',
        }),
        expect.objectContaining({
          conversationId: 'dm-1',
          target: 'dm:@bot1',
        }),
      ])
    )
  })

  it('stores absolute conversation notification state and applies read cursor updates', () => {
    const createdAt = '2026-03-29T10:00:00Z'
    const conversationEvent: RealtimeEvent = {
      eventId: 11,
      eventType: 'conversation.state',
      scopeKind: 'channel',
      scopeId: 'channel:ch-1',
      payload: {
        conversationId: 'ch-1',
        latestSeq: 12,
        lastReadSeq: 9,
        unreadCount: 3,
        lastMessageId: 'msg-12',
        lastMessageAt: createdAt,
      },
      createdAt,
    }
    const readCursorEvent: RealtimeEvent = {
      eventId: 12,
      eventType: 'conversation.read_cursor_set',
      scopeKind: 'user',
      scopeId: 'user:alice',
      payload: {
        conversationId: 'ch-1',
        latestSeq: 12,
        lastReadSeq: 12,
        unreadCount: 0,
        lastReadMessageId: 'msg-12',
      },
      createdAt,
    }

    let state = applyInboxEvent(createInboxState(), conversationEvent)
    expect(state.conversations['ch-1']).toMatchObject({
      conversationId: 'ch-1',
      latestSeq: 12,
      lastReadSeq: 9,
      unreadCount: 3,
      lastMessageId: 'msg-12',
      lastMessageAt: createdAt,
    })

    state = applyInboxEvent(state, readCursorEvent)
    expect(state.conversations['ch-1']).toMatchObject({
      conversationId: 'ch-1',
      latestSeq: 12,
      lastReadSeq: 12,
      unreadCount: 0,
      lastReadMessageId: 'msg-12',
    })
  })

  it('tracks thread state separately from parent conversation state', () => {
    const createdAt = '2026-03-29T10:05:00Z'
    const threadEvent: RealtimeEvent = {
      eventId: 21,
      eventType: 'thread.state',
      scopeKind: 'thread',
      scopeId: 'thread:msg-1',
      threadParentId: 'msg-1',
      payload: {
        conversationId: 'ch-1',
        threadParentId: 'msg-1',
        latestSeq: 7,
        lastReadSeq: 4,
        unreadCount: 3,
        lastReplyMessageId: 'msg-7',
        lastReplyAt: createdAt,
      },
      createdAt,
    }
    const threadReadEvent: RealtimeEvent = {
      eventId: 22,
      eventType: 'thread.read_cursor_set',
      scopeKind: 'user',
      scopeId: 'user:alice',
      threadParentId: 'msg-1',
      payload: {
        conversationId: 'ch-1',
        threadParentId: 'msg-1',
        latestSeq: 7,
        lastReadSeq: 7,
        unreadCount: 0,
        lastReadMessageId: 'msg-7',
      },
      createdAt,
    }

    let state = applyInboxEvent(createInboxState(), threadEvent)
    expect(state.threads[threadNotificationKey('ch-1', 'msg-1')]).toMatchObject({
      conversationId: 'ch-1',
      threadParentId: 'msg-1',
      latestSeq: 7,
      lastReadSeq: 4,
      unreadCount: 3,
      lastReplyMessageId: 'msg-7',
      lastReplyAt: createdAt,
    })

    state = applyInboxEvent(state, threadReadEvent)
    expect(state.threads[threadNotificationKey('ch-1', 'msg-1')]).toMatchObject({
      latestSeq: 7,
      lastReadSeq: 7,
      unreadCount: 0,
      lastReadMessageId: 'msg-7',
    })
  })
})
