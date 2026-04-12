import { describe, expect, it } from 'vitest'

import {
  bootstrapInboxState,
  ensureInboxConversations,
} from './inbox'
import type { ChannelInfo } from '../components/channels/types'
import type { InboxConversationState } from '../data/inbox'

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
