import { describe, expect, it } from 'vitest'
import type { InboxState } from '../inbox'
import type { InboxConversationState, ThreadInboxEntry } from '../data/inbox'
import {
  conversationThreadUnreadCount,
  dmConversationNameForParticipants,
  mergeChannelThreadInboxEntries,
  threadNotificationKey,
} from '../inbox'

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
    threadUnreadCount: 0,
    lastReadMessageId: null,
    lastMessageId: null,
    lastMessageAt: null,
    ...overrides,
  }
}

function makeInboxState(
  conversations: InboxConversationState[] = [],
  threads: InboxState['threads'] = {}
): InboxState {
  const convMap: Record<string, InboxConversationState> = {}
  for (const c of conversations) convMap[c.conversationId] = c
  return { conversations: convMap, threads }
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

describe('getConversationThreadUnreadCount', () => {
  it('returns threadUnreadCount from the conversation entry', () => {
    const state = makeInboxState([
      makeConversation({ conversationId: 'ch-1', threadUnreadCount: 3 }),
    ])
    expect(state.conversations['ch-1']?.threadUnreadCount ?? 0).toBe(3)
  })

  it('returns 0 when conversation does not exist', () => {
    const state = makeInboxState([])
    expect(state.conversations['ch-1']?.threadUnreadCount ?? 0).toBe(0)
  })
})

describe('conversationThreadUnreadCount (from inbox)', () => {
  it('sums unread across all threads in a conversation', () => {
    const key1 = threadNotificationKey('ch-1', 'thread-a')
    const key2 = threadNotificationKey('ch-1', 'thread-b')
    const state = makeInboxState([], {
      [key1]: {
        conversationId: 'ch-1',
        threadParentId: 'thread-a',
        latestSeq: 10,
        lastReadSeq: 8,
        unreadCount: 2,
      },
      [key2]: {
        conversationId: 'ch-1',
        threadParentId: 'thread-b',
        latestSeq: 5,
        lastReadSeq: 3,
        unreadCount: 2,
      },
    })
    expect(conversationThreadUnreadCount(state, 'ch-1')).toBe(4)
  })

  it('ignores threads from other conversations', () => {
    const key = threadNotificationKey('ch-2', 'thread-x')
    const state = makeInboxState([], {
      [key]: {
        conversationId: 'ch-2',
        threadParentId: 'thread-x',
        latestSeq: 10,
        lastReadSeq: 5,
        unreadCount: 5,
      },
    })
    expect(conversationThreadUnreadCount(state, 'ch-1')).toBe(0)
  })

  it('returns 0 for null conversationId', () => {
    const state = makeInboxState([])
    expect(conversationThreadUnreadCount(state, null)).toBe(0)
  })
})

describe('mergeChannelThreadInboxEntries', () => {
  const baseEntry: ThreadInboxEntry = {
    conversationId: 'ch-1',
    threadParentId: 'thread-a',
    parentSeq: 1,
    parentSenderName: 'alice',
    parentSenderType: 'human',
    parentContent: 'hello',
    parentCreatedAt: '2026-01-01T00:00:00Z',
    replyCount: 3,
    participantCount: 2,
    latestSeq: 5,
    lastReadSeq: 5,
    unreadCount: 0,
    lastReplyMessageId: null,
    lastReplyAt: null,
  }

  it('overlays live thread state onto API entries', () => {
    const key = threadNotificationKey('ch-1', 'thread-a')
    const state = makeInboxState([], {
      [key]: {
        conversationId: 'ch-1',
        threadParentId: 'thread-a',
        latestSeq: 8,
        lastReadSeq: 5,
        unreadCount: 3,
      },
    })
    const merged = mergeChannelThreadInboxEntries([baseEntry], state, 'ch-1')
    expect(merged).toHaveLength(1)
    expect(merged[0].latestSeq).toBe(8)
    expect(merged[0].unreadCount).toBe(3)
  })

  it('keeps original entry when no live state exists', () => {
    const state = makeInboxState([])
    const merged = mergeChannelThreadInboxEntries([baseEntry], state, 'ch-1')
    expect(merged).toHaveLength(1)
    expect(merged[0].latestSeq).toBe(5)
    expect(merged[0].unreadCount).toBe(0)
  })

  it('sorts by latestSeq descending', () => {
    const entryB: ThreadInboxEntry = {
      ...baseEntry,
      threadParentId: 'thread-b',
      parentSeq: 2,
      latestSeq: 10,
    }
    const state = makeInboxState([])
    const merged = mergeChannelThreadInboxEntries([baseEntry, entryB], state, 'ch-1')
    expect(merged[0].threadParentId).toBe('thread-b')
    expect(merged[1].threadParentId).toBe('thread-a')
  })
})
