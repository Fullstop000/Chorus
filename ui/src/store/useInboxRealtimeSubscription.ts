import { useEffect, useRef } from 'react'
import type { AgentInfo, ChannelInfo } from '../data'
import { getConversationInboxNotification } from '../data'
import {
  buildConversationRegistry,
  mergeInboxNotificationRefresh,
  type InboxState,
} from '../inbox'
import { getRealtimeSession } from '../transport/realtimeSession'

function parseThreadParentId(raw: unknown): string | undefined {
  return typeof raw === 'string' && raw.length > 0 ? raw : undefined
}

/** WebSocket-driven inbox unread refresh with in-flight dedup and trailing coalescing. */
export function subscribeInbox(params: {
  currentUser: string
  shellBootstrapped: boolean
  systemChannels: ChannelInfo[]
  channels: ChannelInfo[]
  dmChannels: ChannelInfo[]
  agents: AgentInfo[]
  updateInboxState: (u: (c: InboxState) => InboxState) => void
}): void {
  const { currentUser, shellBootstrapped, systemChannels, channels, dmChannels, agents, updateInboxState } =
    params

  const inboxRefreshInFlight = useRef<Set<string>>(new Set())
  const inboxRefreshPending = useRef<Map<string, [string, string | undefined]>>(new Map())

  useEffect(() => {
    if (!currentUser || !shellBootstrapped) return

    const conversationRegistry = buildConversationRegistry({
      currentUser,
      systemChannels,
      channels,
      dmChannels,
      agents,
    })
    const targets = conversationRegistry.map((e) => `conversation:${e.conversationId}`)
    if (targets.length === 0) return

    const scheduleInboxRefresh = (key: string, channelId: string, threadParentId: string | undefined): void => {
      inboxRefreshInFlight.current.add(key)
      void getConversationInboxNotification(channelId, threadParentId)
        .then((payload) => {
          updateInboxState((current: InboxState) => mergeInboxNotificationRefresh(current, payload))
        })
        .catch((error) => {
          console.error('Failed to refresh inbox after message', error)
        })
        .finally(() => {
          inboxRefreshInFlight.current.delete(key)
          const pending = inboxRefreshPending.current.get(key)
          if (pending) {
            inboxRefreshPending.current.delete(key)
            scheduleInboxRefresh(key, pending[0], pending[1])
          }
        })
    }

    return getRealtimeSession(currentUser).subscribe({
      targets,
      onFrame: (frame) => {
        if (frame.type === 'error') {
          console.error('Inbox realtime subscription failed', frame.message)
          return
        }
        if (frame.event.eventType === 'message.created') {
          const channelId = frame.event.channelId
          const threadParentId = parseThreadParentId(frame.event.payload.threadParentId)
          const key = `${channelId}:${threadParentId ?? ''}`
          if (inboxRefreshInFlight.current.has(key)) {
            inboxRefreshPending.current.set(key, [channelId, threadParentId])
          } else {
            void getConversationInboxNotification(channelId, threadParentId)
              .then((payload) => {
                updateInboxState((current: InboxState) => mergeInboxNotificationRefresh(current, payload))
                inboxRefreshInFlight.current.delete(key)
                const pending = inboxRefreshPending.current.get(key)
                if (pending) {
                  inboxRefreshPending.current.delete(key)
                  void getConversationInboxNotification(pending[0], pending[1] || undefined)
                    .then((p) => updateInboxState((c: InboxState) => mergeInboxNotificationRefresh(c, p)))
                }
              })
              .catch((error) => {
                inboxRefreshInFlight.current.delete(key)
                console.error('Failed to refresh inbox after message', error)
              })
            inboxRefreshInFlight.current.add(key)
          }
          return
        }
      },
    })
  }, [agents, channels, currentUser, dmChannels, shellBootstrapped, systemChannels, updateInboxState])
}
