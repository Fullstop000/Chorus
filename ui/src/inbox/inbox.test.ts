import { describe, expect, it } from 'vitest'

import {
  applyConversationRead,
  bootstrapInboxState,
  ensureInboxConversations,
  mergeInboxNotificationRefresh,
  mergeReadCursorAckIntoInboxState,
} from './inbox'
import type { ChannelInfo } from '../components/channels/types'
import type { InboxConversationState } from './types'

function makeConversation(
  overrides: Partial<InboxConversationState> = {}
): InboxConversationState {
  return {
    conversationId: 'conversation-1',
    conversationName: 'general',
    conversationType: 'channel',
    latestSeq: 2,
    lastReadSeq: 2,
    unreadCount: 0,
    threadUnreadCount: 0,
    lastReadMessageId: 'message-2',
    lastMessageId: 'message-2',
    lastMessageAt: '2026-03-30T00:00:00Z',
    ...overrides,
  }
}

function makeChannel(overrides: Partial<ChannelInfo> = {}): ChannelInfo {
  return {
    id: 'conversation-1',
    name: 'general',
    description: 'General',
    joined: true,
    channel_type: 'channel',
    ...overrides,
  }
}

describe('mergeInboxNotificationRefresh', () => {
  it('applies server conversation + thread rows from inbox-notification', () => {
    const state = bootstrapInboxState([makeConversation({ latestSeq: 2, unreadCount: 0 })])

    const next = mergeInboxNotificationRefresh(state, {
      conversation: {
        conversationId: 'conversation-1',
        conversationName: 'general',
        conversationType: 'channel',
        latestSeq: 5,
        lastReadSeq: 2,
        unreadCount: 2,
        threadUnreadCount: 0,
        lastMessageId: 'm-new',
        lastMessageAt: '2026-03-30T01:00:00Z',
      },
      thread: {
        conversationId: 'conversation-1',
        threadParentId: 'parent-1',
        latestSeq: 4,
        lastReadSeq: 1,
        unreadCount: 1,
        lastReplyMessageId: 'reply-1',
        lastReplyAt: '2026-03-30T00:30:00Z',
      },
    })

    expect(next.conversations['conversation-1']).toEqual(
      expect.objectContaining({
        latestSeq: 5,
        lastReadSeq: 2,
        unreadCount: 2,
        lastMessageId: 'm-new',
      })
    )
    expect(next.threads['conversation-1:parent-1']).toEqual(
      expect.objectContaining({
        unreadCount: 1,
        latestSeq: 4,
        lastReadSeq: 1,
      })
    )
  })

  it('ignores stale payloads when latestSeq regressed', () => {
    const state = bootstrapInboxState([
      makeConversation({ latestSeq: 10, unreadCount: 1 }),
    ])

    const next = mergeInboxNotificationRefresh(state, {
      conversation: {
        conversationId: 'conversation-1',
        conversationName: 'general',
        conversationType: 'channel',
        latestSeq: 9,
        lastReadSeq: 2,
        unreadCount: 99,
        threadUnreadCount: 0,
        lastMessageId: 'old',
        lastMessageAt: null,
      },
    })

    expect(next.conversations['conversation-1']?.latestSeq).toBe(10)
    expect(next.conversations['conversation-1']?.unreadCount).toBe(1)
  })
})

describe('bootstrapInboxState', () => {
  it('seeds zeroed entries for joined conversations missing from the inbox snapshot', () => {
    const state = bootstrapInboxState([], [
      makeChannel({
        id: 'conversation-2',
        name: 'qa-unread',
      }),
    ])

    expect(state.conversations['conversation-2']).toEqual(
      expect.objectContaining({
        conversationId: 'conversation-2',
        conversationName: 'qa-unread',
        conversationType: 'channel',
        latestSeq: 0,
        lastReadSeq: 0,
        unreadCount: 0,
      })
    )
  })
})

describe('ensureInboxConversations', () => {
  it('adds joined channels discovered after bootstrap without resetting existing unread state', () => {
    const state = bootstrapInboxState([
      makeConversation({
        conversationId: 'conversation-1',
        latestSeq: 5,
        lastReadSeq: 2,
        unreadCount: 3,
      }),
    ])

    const next = ensureInboxConversations(state, [
      makeChannel({
        id: 'conversation-2',
        name: 'late-join',
      }),
    ])

    expect(next.conversations['conversation-1']).toEqual(
      expect.objectContaining({
        latestSeq: 5,
        lastReadSeq: 2,
        unreadCount: 3,
      })
    )
    expect(next.conversations['conversation-2']).toEqual(
      expect.objectContaining({
        conversationId: 'conversation-2',
        conversationName: 'late-join',
        latestSeq: 0,
        lastReadSeq: 0,
        unreadCount: 0,
      })
    )
  })
})

describe('mergeReadCursorAckIntoInboxState', () => {
  it('applies server conversation unread after a thread read (channel badge drops thread replies)', () => {
    const state = bootstrapInboxState([
      makeConversation({
        latestSeq: 10,
        lastReadSeq: 5,
        unreadCount: 4,
      }),
    ])

    const next = mergeReadCursorAckIntoInboxState(state, {
      conversationId: 'conversation-1',
      conversationUnreadCount: 2,
      conversationLastReadSeq: 5,
      conversationLatestSeq: 10,
      threadParentId: 'parent-1',
      threadUnreadCount: 0,
      threadLastReadSeq: 8,
      threadLatestSeq: 8,
    })

    expect(next.conversations['conversation-1']).toEqual(
      expect.objectContaining({
        unreadCount: 2,
        lastReadSeq: 5,
        latestSeq: 10,
      })
    )
    expect(next.threads['conversation-1:parent-1']).toEqual(
      expect.objectContaining({
        unreadCount: 0,
        lastReadSeq: 8,
        latestSeq: 8,
      })
    )
  })
})

describe('applyConversationRead', () => {
  it('reduces unreadCount when the local conversation read cursor advances', () => {
    const state = bootstrapInboxState([
      makeConversation({
        latestSeq: 5,
        lastReadSeq: 2,
        unreadCount: 3,
      }),
    ])

    const next = applyConversationRead(state, 'conversation-1', 5)

    expect(next.conversations['conversation-1']).toEqual(
      expect.objectContaining({
        latestSeq: 5,
        lastReadSeq: 5,
        unreadCount: 0,
      })
    )
  })
})
