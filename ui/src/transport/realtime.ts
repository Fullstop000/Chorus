import type { HistoryMessage, StreamEvent } from '../types'

const BASE = ''

export function createRealtimeSocket(viewer: string): WebSocket {
  const url = new URL(`${BASE}/api/events/ws`, window.location.origin)
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
  url.searchParams.set('viewer', viewer)
  return new WebSocket(url)
}

export function applyRealtimeEvent(
  messages: HistoryMessage[],
  event: StreamEvent
): HistoryMessage[] {
  switch (event.eventType) {
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

function eventMatchesActiveRealtimeTarget(
  activeRealtimeTarget: string | null,
  event: StreamEvent
): boolean {
  if (!activeRealtimeTarget) return false
  if (!activeRealtimeTarget.startsWith('conversation:')) {
    return false
  }
  return event.channelId === activeRealtimeTarget.slice('conversation:'.length)
}

export function historyFetchAfterForNotification(
  activeRealtimeTarget: string | null,
  event: StreamEvent,
  loadedMaxSeq: number,
  threadParentId?: string | null
): number | null {
  if (event.eventType !== 'message.created') {
    return null
  }
  if (!eventMatchesActiveRealtimeTarget(activeRealtimeTarget, event)) {
    return null
  }
  if (threadParentId) {
    const eventThreadParentId = event.payload.threadParentId
    if (eventThreadParentId !== threadParentId) {
      return null
    }
  }
  if (event.latestSeq <= loadedMaxSeq) {
    return null
  }
  return loadedMaxSeq
}
