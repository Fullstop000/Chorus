import { describe, expect, it } from 'vitest'
import type { InboxState } from '../store/inbox'
import type { InboxConversationState } from '../data/inbox'
import { dmConversationNameForParticipants } from '../data/inbox'

// ── Helpers ──

function makeConversation(
  overrides: Partial<InboxConversationState> = {}
): InboxConversationState {
  return {
    conversationId: 'ch-1',
    conversationName: 'general',
    conversationType: 'channel',
    latestSeq: 10,
    lastReadSeq: 10,
    unreadCount: 0,
    lastReadMessageId: null,
    lastMessageId: null,
    lastMessageAt: null,
    ...overrides,
  }
}

function makeInboxState(
  conversations: InboxConversationState[] = [],
): InboxState {
  const convMap: Record<string, InboxConversationState> = {}
  for (const c of conversations) convMap[c.conversationId] = c
  return { conversations: convMap }
}

// ── Pure selector logic (mirrors useAppInboxSelectors) ──

function getConversationUnread(
  inboxState: InboxState,
  conversationId?: string | null
): number {
  if (!conversationId) return 0
  const conv = inboxState.conversations[conversationId]
  if (!conv) return 0
  return Math.max(conv.latestSeq - conv.lastReadSeq, 0)
}

function getAgentUnread(
  inboxState: InboxState,
  currentUser: string,
  agentName: string,
  dmChannels: { id: string; name: string }[]
): number {
  const dmName = dmConversationNameForParticipants(currentUser, agentName)
  const conversationId = dmChannels.find((ch) => ch.name === dmName)?.id ?? null
  return getConversationUnread(inboxState, conversationId)
}

// ── Tests ──

describe('getConversationUnread', () => {
  it('returns 0 when conversationId is null', () => {
    const state = makeInboxState([makeConversation()])
    expect(getConversationUnread(state, null)).toBe(0)
  })

  it('returns 0 when conversationId is not in inbox', () => {
    const state = makeInboxState([makeConversation()])
    expect(getConversationUnread(state, 'unknown')).toBe(0)
  })

  it('returns 0 when lastReadSeq equals latestSeq', () => {
    const state = makeInboxState([
      makeConversation({ conversationId: 'ch-1', latestSeq: 5, lastReadSeq: 5 }),
    ])
    expect(getConversationUnread(state, 'ch-1')).toBe(0)
  })

  it('returns latestSeq minus lastReadSeq when there are unread messages', () => {
    const state = makeInboxState([
      makeConversation({ conversationId: 'ch-1', latestSeq: 15, lastReadSeq: 10 }),
    ])
    expect(getConversationUnread(state, 'ch-1')).toBe(5)
  })

  it('clamps to 0 when lastReadSeq exceeds latestSeq', () => {
    const state = makeInboxState([
      makeConversation({ conversationId: 'ch-1', latestSeq: 3, lastReadSeq: 5 }),
    ])
    expect(getConversationUnread(state, 'ch-1')).toBe(0)
  })
})

describe('getAgentUnread', () => {
  it('returns unread count for the DM channel with the agent', () => {
    const dmChannels = [{ id: 'dm-1', name: 'dm-alice-bot' }]
    const state = makeInboxState([
      makeConversation({ conversationId: 'dm-1', latestSeq: 20, lastReadSeq: 15 }),
    ])
    expect(getAgentUnread(state, 'alice', 'bot', dmChannels)).toBe(5)
  })

  it('returns 0 when no DM channel exists for the agent', () => {
    const state = makeInboxState([])
    expect(getAgentUnread(state, 'alice', 'bot', [])).toBe(0)
  })
})
