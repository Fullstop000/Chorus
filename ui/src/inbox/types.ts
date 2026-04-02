// ── Inbox / thread read state (API + client) ──

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

/** GET /api/conversations/{id}/inbox-notification */
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
