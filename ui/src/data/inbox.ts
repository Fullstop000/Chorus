import { get } from './client'

// ── Types (source of truth — API responses) ──

export interface InboxConversationState {
  conversationId: string
  conversationName: string
  conversationType: string
  latestSeq: number
  lastReadSeq: number
  unreadCount: number
  threadUnreadCount: number
  lastReadMessageId?: string | null
  lastMessageId?: string | null
  lastMessageAt?: string | null
}

export interface InboxResponse {
  conversations: InboxConversationState[]
}

export interface ConversationInboxRefreshResponse {
  conversation: InboxConversationState
  thread?: {
    conversationId: string
    threadParentId: string
    latestSeq: number
    lastReadSeq: number
    unreadCount: number
    lastReplyMessageId?: string | null
    lastReplyAt?: string | null
  }
}

export interface ThreadInboxEntry {
  conversationId: string
  threadParentId: string
  parentSeq: number
  parentSenderName: string
  parentSenderType: 'human' | 'agent'
  parentContent: string
  parentCreatedAt: string
  replyCount: number
  participantCount: number
  latestSeq: number
  lastReadSeq: number
  unreadCount: number
  lastReplyMessageId?: string | null
  lastReplyAt?: string | null
}

export interface ThreadInboxResponse {
  unreadCount: number
  threads: ThreadInboxEntry[]
}

// ── API functions ──

export function getInboxState(_username: string): Promise<InboxResponse> {
  return get('/api/inbox')
}

export function getChannelThreads(conversationId: string): Promise<ThreadInboxResponse> {
  return get(`/api/conversations/${encodeURIComponent(conversationId)}/threads`)
}

// ── Conversation helpers ──

export function dmConversationNameForParticipants(left: string, right: string): string {
  return `dm-${[left, right].sort().join('-')}`
}
