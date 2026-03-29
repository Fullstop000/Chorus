import type { HistoryMessage, RealtimeEvent, RealtimeMessage } from '../types'

const BASE = ''

export function createRealtimeSocket(viewer: string): WebSocket {
  const url = new URL(`${BASE}/api/events/ws`, window.location.origin)
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
  url.searchParams.set('viewer', viewer)
  return new WebSocket(url)
}

export function applyRealtimeEvent(
  messages: HistoryMessage[],
  event: RealtimeEvent
): HistoryMessage[] {
  switch (event.eventType) {
    case 'conversation.state':
    case 'thread.state':
      return messages
    case 'thread.reply_count_changed': {
      const payload = event.payload
      const parentMessageId =
        typeof payload.parentMessageId === 'string' ? payload.parentMessageId : null
      const replyCount = typeof payload.replyCount === 'number' ? payload.replyCount : null
      if (!parentMessageId || replyCount == null) return messages
      return messages.map((message) =>
        message.id === parentMessageId ? { ...message, replyCount } : message
      )
    }
    case 'message.tombstone_changed': {
      const payload = event.payload
      const messageId = typeof payload.messageId === 'string' ? payload.messageId : null
      if (!messageId) return messages
      return messages.map((message) =>
        message.id === messageId ? { ...message, senderDeleted: true } : message
      )
    }
    default:
      return messages
  }
}

export function maxHistorySeq(messages: HistoryMessage[]): number {
  return messages.reduce((maxSeq, message) => Math.max(maxSeq, message.seq), 0)
}

export function mergeHistoryMessages(
  current: HistoryMessage[],
  incoming: HistoryMessage[]
): HistoryMessage[] {
  const byId = new Map<string, HistoryMessage>()
  for (const message of current) {
    byId.set(message.id, message)
  }
  for (const message of incoming) {
    const prior = byId.get(message.id)
    byId.set(message.id, prior ? { ...prior, ...message } : message)
  }
  return [...byId.values()].sort((left, right) => left.seq - right.seq)
}

function notificationLatestSeq(event: RealtimeEvent): number | null {
  const latestSeq = event.payload.latestSeq
  return typeof latestSeq === 'number' ? latestSeq : null
}

function eventMatchesActiveRealtimeTarget(
  activeRealtimeTarget: string | null,
  event: RealtimeEvent
): boolean {
  if (!activeRealtimeTarget) return false
  if (activeRealtimeTarget.startsWith('conversation:')) {
    return event.streamId === activeRealtimeTarget
  }
  if (!activeRealtimeTarget.startsWith('thread:')) {
    return false
  }
  const threadParentId =
    typeof event.threadParentId === 'string'
      ? event.threadParentId
      : typeof event.payload.threadParentId === 'string'
        ? event.payload.threadParentId
        : null
  return event.scopeId === activeRealtimeTarget || threadParentId === activeRealtimeTarget.slice(7)
}

export function historyFetchAfterForNotification(
  activeRealtimeTarget: string | null,
  event: RealtimeEvent,
  loadedMaxSeq: number
): number | null {
  if (event.eventType !== 'conversation.state' && event.eventType !== 'thread.state') {
    return null
  }
  if (!eventMatchesActiveRealtimeTarget(activeRealtimeTarget, event)) {
    return null
  }
  const latestSeq = notificationLatestSeq(event)
  if (latestSeq == null || latestSeq <= loadedMaxSeq) {
    return null
  }
  return loadedMaxSeq
}

export function nextRealtimeCursor(currentCursor: number, frame: RealtimeMessage): number {
  if (frame.type === 'subscribed') {
    return frame.resumeFrom ?? 0
  }
  if (frame.type === 'event') {
    return Math.max(currentCursor, frame.event.eventId)
  }
  return currentCursor
}
