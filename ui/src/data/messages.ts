import type { HistoryMessage, StreamEvent } from './chat'

export function normalizeEvent(event: StreamEvent): HistoryMessage | null {
  if (event.eventType !== 'message.created') return null
  const p = event.payload
  if (!p.messageId || !p.content || !p.sender?.name) return null
  return {
    id: p.messageId,
    seq: p.seq ?? event.latestSeq,
    content: p.content,
    senderName: p.sender.name,
    senderType: p.sender.type ?? 'human',
    senderDeleted: p.senderDeleted ?? false,
    createdAt: p.createdAt ?? new Date().toISOString(),
    thread_parent_id: p.threadParentId ?? undefined,
  }
}

export function bumpReplyCount(messages: HistoryMessage[], parentId: string): HistoryMessage[] {
  return messages.map((message) =>
    message.id === parentId
      ? { ...message, replyCount: (message.replyCount ?? 0) + 1 }
      : message
  )
}

export function maxHistorySeq(messages: HistoryMessage[]): number {
  return messages.reduce((maxSeq, message) => Math.max(maxSeq, message.seq), 0)
}

