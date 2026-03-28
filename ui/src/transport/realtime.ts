import { resolveChannel } from '../api'
import type {
  AttachmentRef,
  ForwardedFrom,
  HistoryMessage,
  RealtimeEvent,
  RealtimeMessage,
} from '../types'

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

function materializeLiveMessage(event: RealtimeEvent): HistoryMessage | null {
  const payload = event.payload
  const messageId = typeof payload.messageId === 'string' ? payload.messageId : null
  const content = typeof payload.content === 'string' ? payload.content : null
  const sender =
    typeof payload.sender === 'object' && payload.sender !== null
      ? (payload.sender as { name?: unknown; type?: unknown })
      : null
  const senderName = typeof sender?.name === 'string' ? sender.name : null
  if (!messageId || content == null || !senderName) {
    return null
  }

  const senderDeleted = payload.senderDeleted === true
  const attachments = Array.isArray(payload.attachments)
    ? payload.attachments
        .map((attachment) => {
          if (typeof attachment !== 'object' || attachment == null) return null
          const value = attachment as { id?: unknown; filename?: unknown }
          if (typeof value.id !== 'string' || typeof value.filename !== 'string') return null
          return {
            id: value.id,
            filename: value.filename,
          } satisfies AttachmentRef
        })
        .filter((attachment): attachment is AttachmentRef => attachment != null)
    : undefined
  const forwardedFrom =
    typeof payload.forwardedFrom === 'object' && payload.forwardedFrom !== null
      ? (() => {
          const value = payload.forwardedFrom as {
            channelName?: unknown
            senderName?: unknown
          }
          if (
            typeof value.channelName !== 'string' ||
            typeof value.senderName !== 'string'
          ) {
            return undefined
          }
          return {
            channelName: value.channelName,
            senderName: value.senderName,
          } satisfies ForwardedFrom
        })()
      : undefined

  return {
    id: messageId,
    seq: typeof payload.seq === 'number' ? payload.seq : 0,
    content,
    senderName,
    senderType: sender?.type === 'agent' ? 'agent' : 'human',
    senderDeleted,
    createdAt: typeof payload.createdAt === 'string' ? payload.createdAt : event.createdAt,
    ...(attachments ? { attachments } : {}),
    ...(forwardedFrom ? { forwardedFrom } : {}),
  }
}

export function applyRealtimeEvent(
  messages: HistoryMessage[],
  event: RealtimeEvent
): HistoryMessage[] {
  switch (event.eventType) {
    case 'message.created': {
      const liveMessage = materializeLiveMessage(event)
      if (!liveMessage) return messages
      const existingIndex = messages.findIndex((message) => message.id === liveMessage.id)
      if (existingIndex >= 0) {
        return messages.map((message, index) =>
          index === existingIndex ? { ...message, ...liveMessage } : message
        )
      }
      return [...messages, liveMessage].sort((left, right) => left.seq - right.seq)
    }
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
