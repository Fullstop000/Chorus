import { get } from './client'

// ── Types (source of truth — API responses) ──

export interface InboxConversationState {
  conversationId: string
  conversationName: string
  conversationType: string
  latestSeq: number
  lastReadSeq: number
  unreadCount: number
  lastReadMessageId?: string | null
  lastMessageId?: string | null
  lastMessageAt?: string | null
}

export interface InboxResponse {
  conversations: InboxConversationState[]
}

export interface ConversationInboxRefreshResponse {
  conversation: InboxConversationState
}

// ── API functions ──

export function getInboxState(_username: string): Promise<InboxResponse> {
  return get('/api/inbox')
}

// ── Conversation helpers ──

export function dmConversationNameForParticipants(left: string, right: string): string {
  return `dm-${[left, right].sort().join('-')}`
}
