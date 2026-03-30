import { describe, expect, it } from 'vitest'

import {
  applyConversationRead,
  applyInboxEvent,
  bootstrapInboxState,
  createInboxState,
  ensureInboxConversations,
} from './inbox'
import type { ChannelInfo, InboxConversationState, StreamEvent } from './types'

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
    lastReadMessageId: 'message-2',
    lastMessageId: 'message-2',
    lastMessageAt: '2026-03-30T00:00:00Z',
    ...overrides,
  }
}

function makeEvent(overrides: Partial<StreamEvent> = {}): StreamEvent {
  return {
    eventType: 'message.created',
    channelId: 'conversation-1',
    latestSeq: 3,
    payload: {
      messageId: 'message-3',
      conversationId: 'conversation-1',
      conversationType: 'channel',
      threadParentId: null,
    },
    schemaVersion: 1,
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

describe('applyInboxEvent', () => {
  it('advances unread state for an existing inactive conversation on message.created', () => {
    const state = bootstrapInboxState([makeConversation()])

    const next = applyInboxEvent(state, makeEvent())

    expect(next.conversations['conversation-1']).toEqual(
      expect.objectContaining({
        conversationId: 'conversation-1',
        latestSeq: 3,
        lastReadSeq: 2,
        unreadCount: 1,
        lastMessageId: 'message-3',
      })
    )
  })

  it('ignores events for conversations that are not already in the inbox registry', () => {
    const state = createInboxState()

    const next = applyInboxEvent(
      state,
      makeEvent({
        channelId: 'conversation-2',
        payload: {
          messageId: 'message-9',
          conversationId: 'conversation-2',
          conversationType: 'channel',
          threadParentId: null,
        },
      })
    )

    expect(next).toEqual(state)
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
