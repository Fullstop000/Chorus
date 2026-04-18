import type { HistoryMessage, StreamEvent } from './chat'
import { EventType } from '../transport'

export function normalizeEvent(event: StreamEvent): HistoryMessage | null {
  if (event.eventType !== EventType.MessageCreated) return null
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
    runId: p.runId ?? undefined,
    traceSummary: p.traceSummary ?? undefined,
  }
}

export function maxHistorySeq(messages: HistoryMessage[]): number {
  return messages.reduce((maxSeq, message) => Math.max(maxSeq, message.seq), 0)
}
