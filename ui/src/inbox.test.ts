import { describe, expect, it } from 'vitest'

import {
  applyInboxEvent,
  buildConversationRegistry,
  conversationThreadUnreadCount,
  createInboxState,
  mergeChannelThreadInboxEntries,
  threadNotificationKey,
} from './inbox'
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

  it('computes per-conversation thread unread totals from tracked thread state', () => {
    let state = createInboxState()
    state = applyInboxEvent(state, {
      eventId: 1,
      eventType: 'thread.state',
      scopeKind: 'thread',
      scopeId: 'thread:msg-1',
      threadParentId: 'msg-1',
      payload: {
        conversationId: 'ch-1',
        threadParentId: 'msg-1',
        latestSeq: 5,
        lastReadSeq: 3,
        unreadCount: 2,
      },
      createdAt: '2026-03-29T10:00:00Z',
    })
    state = applyInboxEvent(state, {
      eventId: 2,
      eventType: 'thread.state',
      scopeKind: 'thread',
      scopeId: 'thread:msg-2',
      threadParentId: 'msg-2',
      payload: {
        conversationId: 'ch-1',
        threadParentId: 'msg-2',
        latestSeq: 8,
        lastReadSeq: 7,
        unreadCount: 1,
      },
      createdAt: '2026-03-29T10:01:00Z',
    })

    expect(conversationThreadUnreadCount(state, 'ch-1')).toBe(3)
    expect(conversationThreadUnreadCount(state, 'ch-2')).toBe(0)
  })

  it('merges fetched channel thread rows with live thread state and keeps latest-reply ordering', () => {
    let state = createInboxState()
    state = applyInboxEvent(state, {
      eventId: 3,
      eventType: 'thread.state',
      scopeKind: 'thread',
      scopeId: 'thread:msg-old',
      threadParentId: 'msg-old',
      payload: {
        conversationId: 'ch-1',
        threadParentId: 'msg-old',
        latestSeq: 11,
        lastReadSeq: 10,
        unreadCount: 1,
        lastReplyMessageId: 'reply-old',
        lastReplyAt: '2026-03-29T10:10:00Z',
      },
      createdAt: '2026-03-29T10:10:00Z',
    })

    const merged = mergeChannelThreadInboxEntries(
      [
        {
          conversationId: 'ch-1',
          threadParentId: 'msg-old',
          parentSeq: 1,
          parentSenderName: 'alice',
          parentSenderType: 'human',
          parentContent: 'older unread thread',
          parentCreatedAt: '2026-03-29T08:00:00Z',
          replyCount: 1,
          participantCount: 2,
          latestSeq: 9,
          lastReadSeq: 9,
          unreadCount: 0,
          lastReplyMessageId: 'reply-old-initial',
          lastReplyAt: '2026-03-29T09:30:00Z',
        },
        {
          conversationId: 'ch-1',
          threadParentId: 'msg-read',
          parentSeq: 3,
          parentSenderName: 'alice',
          parentSenderType: 'human',
          parentContent: 'already read thread',
          parentCreatedAt: '2026-03-29T09:00:00Z',
          replyCount: 1,
          participantCount: 2,
          latestSeq: 12,
          lastReadSeq: 12,
          unreadCount: 0,
          lastReplyMessageId: 'reply-read',
          lastReplyAt: '2026-03-29T10:12:00Z',
        },
      ],
      state,
      'ch-1'
    )

    expect(merged[0]).toMatchObject({
      threadParentId: 'msg-read',
      unreadCount: 0,
    })
    expect(merged[1]).toMatchObject({
      threadParentId: 'msg-old',
      unreadCount: 1,
      latestSeq: 11,
      lastReplyMessageId: 'reply-old',
    })
  })
})
