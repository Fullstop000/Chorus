import { resolveChannel } from '../api'
import type { HistoryMessage, RealtimeEvent, RealtimeMessage } from '../types'

const BASE = ''

export function createRealtimeSocket(viewer: string): WebSocket {
  const url = new URL(`${BASE}/api/events/ws`, window.location.origin)
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
  url.searchParams.set('viewer', viewer)
  return new WebSocket(url)
}

export function parseHistoryTarget(target: string): {
  conversationTarget: string
  threadParentId: string | null
} {
  if (target.startsWith('dm:@')) {
    const firstColon = target.indexOf(':')
    const lastColon = target.lastIndexOf(':')
    if (lastColon > firstColon) {
      return {
        conversationTarget: target.slice(0, lastColon),
        threadParentId: target.slice(lastColon + 1),
      }
    }
  }
  if (target.startsWith('#')) {
    const separator = target.lastIndexOf(':')
    if (separator > 0) {
      return {
        conversationTarget: target.slice(0, separator),
        threadParentId: target.slice(separator + 1),
      }
    }
  }
  return { conversationTarget: target, threadParentId: null }
}

export async function resolveRealtimeTarget(
  username: string,
  target: string
): Promise<string> {
  const { conversationTarget, threadParentId } = parseHistoryTarget(target)
  const { channelId } = await resolveChannel(username, conversationTarget)
  if (threadParentId) {
    return `thread:${threadParentId}`
  }
  return `conversation:${channelId}`
}

export function applyRealtimeEvent(
  messages: HistoryMessage[],
  event: RealtimeEvent
): HistoryMessage[] {
  switch (event.eventType) {
    case 'message.created':
    case 'conversation.state':
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

export function nextRealtimeCursor(currentCursor: number, frame: RealtimeMessage): number {
  if (frame.type === 'subscribed') {
    return frame.resumeFrom ?? 0
  }
  if (frame.type === 'event') {
    return Math.max(currentCursor, frame.event.eventId)
  }
  return currentCursor
}
