import type {
  AgentInfo,
  ChannelInfo,
  InboxConversationState,
  StreamEvent,
  ThreadInboxEntry,
} from './types'

export interface ThreadInboxState {
  conversationId: string
  threadParentId: string
  latestSeq: number
  lastReadSeq: number
  unreadCount: number
  lastReadMessageId?: string | null
  lastReplyMessageId?: string | null
  lastReplyAt?: string | null
}

export interface InboxState {
  conversations: Record<string, InboxConversationState>
  threads: Record<string, ThreadInboxState>
}

export interface ConversationRegistryEntry {
  conversationId: string
  target: string
  conversationType: string
  label: string
}

interface BuildConversationRegistryOptions {
  currentUser: string
  systemChannels: ChannelInfo[]
  channels: ChannelInfo[]
  dmChannels: ChannelInfo[]
  agents: AgentInfo[]
}

export function createInboxState(): InboxState {
  return {
    conversations: {},
    threads: {},
  }
}

export function threadNotificationKey(conversationId: string, threadParentId: string): string {
  return `${conversationId}:${threadParentId}`
}

export function conversationThreadUnreadCount(
  state: InboxState,
  conversationId?: string | null
): number {
  if (!conversationId) return 0
  let unreadCount = 0
  for (const threadState of Object.values(state.threads)) {
    if (threadState.conversationId !== conversationId) continue
    unreadCount += threadState.unreadCount
  }
  return unreadCount
}

export function mergeChannelThreadInboxEntries(
  entries: ThreadInboxEntry[],
  state: InboxState,
  conversationId?: string | null
): ThreadInboxEntry[] {
  const merged = entries
    .filter((entry) => !conversationId || entry.conversationId === conversationId)
    .map((entry) => {
      const liveState = state.threads[threadNotificationKey(entry.conversationId, entry.threadParentId)]
      if (!liveState) return entry
      return {
        ...entry,
        latestSeq: liveState.latestSeq,
        lastReadSeq: liveState.lastReadSeq,
        unreadCount: liveState.unreadCount,
        lastReplyMessageId: liveState.lastReplyMessageId ?? entry.lastReplyMessageId ?? null,
        lastReplyAt: liveState.lastReplyAt ?? entry.lastReplyAt ?? null,
      }
    })

  merged.sort((left, right) =>
    (right.latestSeq - left.latestSeq) ||
    (right.parentSeq - left.parentSeq)
  )

  return merged
}

export function dmConversationNameForParticipants(left: string, right: string): string {
  return `dm-${[left, right].sort().join('-')}`
}

function dmPeerName(currentUser: string, dmChannelName: string): string | null {
  if (!dmChannelName.startsWith('dm-')) return null
  const participants = dmChannelName.slice(3).split('-')
  if (participants.length < 2) return null
  return participants.find((participant) => participant !== currentUser) ?? null
}

export function buildConversationRegistry(
  options: BuildConversationRegistryOptions
): ConversationRegistryEntry[] {
  const entries: ConversationRegistryEntry[] = []
  const seenConversationIds = new Set<string>()

  const pushEntry = (entry: ConversationRegistryEntry | null) => {
    if (!entry || seenConversationIds.has(entry.conversationId)) return
    seenConversationIds.add(entry.conversationId)
    entries.push(entry)
  }

  for (const channel of options.systemChannels) {
    if (!channel.id || channel.joined === false) continue
    pushEntry({
      conversationId: channel.id,
      target: `#${channel.name}`,
      conversationType: channel.channel_type ?? 'system',
      label: channel.name,
    })
  }

  for (const channel of options.channels) {
    if (!channel.id || channel.joined === false) continue
    pushEntry({
      conversationId: channel.id,
      target: `#${channel.name}`,
      conversationType: channel.channel_type ?? 'channel',
      label: channel.name,
    })
  }

  const knownAgents = new Set(options.agents.map((agent) => agent.name))
  for (const channel of options.dmChannels) {
    if (!channel.id || channel.joined === false) continue
    const peer = dmPeerName(options.currentUser, channel.name)
    if (!peer || !knownAgents.has(peer)) continue
    pushEntry({
      conversationId: channel.id,
      target: `dm:@${peer}`,
      conversationType: 'dm',
      label: peer,
    })
  }

  return entries
}

export function applyInboxEvent(state: InboxState, _event: StreamEvent): InboxState {
  return state
}

export function bootstrapInboxState(
  conversations: InboxConversationState[]
): InboxState {
  const nextState = createInboxState()
  for (const conversation of conversations) {
    nextState.conversations[conversation.conversationId] = conversation
  }
  return nextState
}
