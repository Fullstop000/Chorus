import React, { createContext, useContext, useCallback, useEffect, useMemo, useRef } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useUIStore } from './uiStore'
import type { ActiveTab } from './uiStore'
import type { ServerInfo, AgentInfo, ChannelInfo, HistoryMessage, Team, ThreadInboxEntry } from './types'
import {
  ensureDirectMessageConversation,
  getChannelThreads,
  getConversationInboxNotification,
  getInboxState,
  getWhoami,
  listAgents,
  listChannels,
  listHumans,
  listTeams,
} from './api'
import {
  bootstrapInboxState,
  buildConversationRegistry,
  conversationThreadUnreadCount,
  dmConversationNameForParticipants,
  ensureInboxConversations,
  mergeChannelThreadInboxEntries,
  mergeInboxNotificationRefresh,
  type InboxState,
  type ReadCursorAckPayload,
} from './inbox'
import { isVisibleSidebarChannel } from './sidebarChannels'
import { getRealtimeSession } from './transport/realtimeSession'

export type { ActiveTab }

export interface AppState {
  currentUser: string
  serverInfo: ServerInfo | null
  channels: ChannelInfo[]
  agents: AgentInfo[]
  teams: Team[]
  serverInfoLoading: boolean
  selectedChannel: string | null
  selectedChannelId: string | null
  selectedAgent: AgentInfo | null
  activeTab: ActiveTab
  openThreadMsg: HistoryMessage | null
  getConversationUnread: (conversationId?: string | null) => number
  getConversationThreads: (conversationId?: string | null) => ThreadInboxEntry[]
  getConversationThreadUnread: (conversationId?: string | null) => number
  getConversationThreadUnreadCount: (conversationId?: string | null) => number
  getAgentUnread: (agentName: string) => number
  getAgentConversationId: (agentName: string) => string | null
  applyReadCursorAck: (ack: ReadCursorAckPayload) => void
  setSelectedChannel: (ch: string | null, channelId?: string | null) => void
  setSelectedAgent: (agent: AgentInfo | null) => void
  setActiveTab: (tab: ActiveTab) => void
  setOpenThreadMsg: (msg: HistoryMessage | null) => void
  refreshServerInfo: () => Promise<void>
  refreshChannels: () => Promise<void>
  refreshConversationThreads: (conversationId: string) => Promise<void>
  refreshAgents: () => Promise<void>
  refreshTeams: () => Promise<void>
}

const AppContext = createContext<AppState | null>(null)

// Stable query key factory
const qk = {
  whoami: ['whoami'] as const,
  agents: ['agents'] as const,
  channels: (user: string) => ['channels', user] as const,
  teams: ['teams'] as const,
  humans: ['humans'] as const,
  inbox: (user: string) => ['inbox', user] as const,
}

export function AppProvider({ children }: { children: React.ReactNode }) {
  const queryClient = useQueryClient()
  // Refs for inbox refresh coordination (deduplication and trailing re-fetch)
  const inboxRefreshInFlight = useRef<Set<string>>(new Set())
  const inboxRefreshPending = useRef<Map<string, [string, string | undefined]>>(new Map())

  const {
    currentUser,
    selectedAgentName,
    selectedChannel,
    selectedChannelId,
    activeTab,
    openThreadMsg,
    inboxState,
    conversationThreads,
    shellBootstrapped,
    setCurrentUser,
    setSelectedAgentName,
    setSelectedChannel,
    setActiveTab,
    setOpenThreadMsg,
    applyReadCursorAck: storeApplyReadCursorAck,
    updateInboxState,
    setConversationThreads,
    setShellBootstrapped,
    resetUserSession,
  } = useUIStore()

  // ── Server queries ─────────────────────────────────────────────────────────

  const whoamiQuery = useQuery({
    queryKey: qk.whoami,
    queryFn: () => getWhoami().then((r) => r.username),
    staleTime: Infinity,
  })

  const agentsQuery = useQuery({
    queryKey: qk.agents,
    queryFn: listAgents,
    enabled: !!currentUser,
  })

  const channelsQuery = useQuery({
    queryKey: qk.channels(currentUser),
    queryFn: () =>
      listChannels({ member: currentUser, includeDm: true, includeSystem: true }),
    enabled: !!currentUser,
  })

  const teamsQuery = useQuery({
    queryKey: qk.teams,
    queryFn: listTeams,
    enabled: !!currentUser,
  })

  const humansQuery = useQuery({
    queryKey: qk.humans,
    queryFn: listHumans,
    enabled: !!currentUser,
  })

  // Inbox fetched once; after that, WebSocket events drive updates.
  const inboxQuery = useQuery({
    queryKey: qk.inbox(currentUser),
    queryFn: () => getInboxState(currentUser),
    enabled: !!currentUser && !shellBootstrapped,
    staleTime: Infinity,
  })

  // ── Sync whoami into Zustand ───────────────────────────────────────────────

  useEffect(() => {
    const username = whoamiQuery.data
    if (!username) return
    if (username === currentUser) return
    if (currentUser) resetUserSession()
    setCurrentUser(username)
  }, [whoamiQuery.data, currentUser, setCurrentUser, resetUserSession])

  // ── Derive channel lists (memoized to keep stable references for effects) ────

  const allChannels = useMemo(() => channelsQuery.data ?? [], [channelsQuery.data])
  const channels = useMemo(
    () => allChannels.filter((ch) => ch.channel_type !== 'dm' && ch.channel_type !== 'system'),
    [allChannels]
  )
  const systemChannels = useMemo(
    () => allChannels.filter((ch) => ch.channel_type === 'system'),
    [allChannels]
  )
  const dmChannels = useMemo(
    () => allChannels.filter((ch) => ch.channel_type === 'dm'),
    [allChannels]
  )
  const agents = useMemo(() => agentsQuery.data ?? [], [agentsQuery.data])
  const teams = useMemo(() => teamsQuery.data ?? [], [teamsQuery.data])
  const humans = useMemo(() => humansQuery.data ?? [], [humansQuery.data])

  // ── Bootstrap inbox once all initial queries have settled ─────────────────
  // Mirror the original finally-block pattern: bootstrap even on partial errors
  // so a single failed fetch can't keep the app in a permanent loading state.
  // Note: empty arrays ([], {}) are valid settled values — do not use !!data.

  const settled = (q: { data: unknown; isError: boolean }) =>
    q.data !== undefined || q.isError

  const allQueriesSettled =
    !!currentUser &&
    settled(channelsQuery) &&
    settled(agentsQuery) &&
    settled(teamsQuery) &&
    settled(humansQuery) &&
    settled(inboxQuery)

  useEffect(() => {
    if (!allQueriesSettled || shellBootstrapped) return
    updateInboxState(() =>
      bootstrapInboxState(
        inboxQuery.data?.conversations ?? [],
        channelsQuery.data ?? []
      )
    )
    setShellBootstrapped(true)
  }, [
    allQueriesSettled,
    shellBootstrapped,
    channelsQuery.data,
    inboxQuery.data,
    updateInboxState,
    setShellBootstrapped,
  ])

  // ── Keep inbox conversations in sync when channel list changes ─────────────

  useEffect(() => {
    if (!allChannels.length) return
    updateInboxState((current) => ensureInboxConversations(current, allChannels))
  }, [allChannels, updateInboxState])

  // ── Auto-select first channel on bootstrap / after channel list changes ────
  // Read selection from Zustand getState() so this effect only re-runs when the
  // *channel list* changes, not every time the user makes a selection. Without this,
  // calling setSelectedChannel() for a newly-created channel (before it appears in the
  // TanStack Query cache) would cause the effect to overwrite the selection with #all.

  useEffect(() => {
    if (!shellBootstrapped) return
    const { selectedAgentName: agentName, selectedChannelId: chId, selectedChannel: ch } =
      useUIStore.getState()
    if (agentName) return

    const joinedChannels = [
      ...systemChannels.filter((c) => c.joined),
      ...channels.filter(isVisibleSidebarChannel),
    ]

    // Keep current selection if still valid
    if (chId && joinedChannels.some((c) => c.id === chId)) return
    if (ch && joinedChannels.some((c) => `#${c.name}` === ch)) return

    const first = joinedChannels[0]
    setSelectedChannel(first ? `#${first.name}` : null, first?.id ?? null)
  }, [shellBootstrapped, channels, systemChannels, setSelectedChannel])

  // ── Ensure DM conversation exists when an agent is selected ───────────────

  useEffect(() => {
    if (!currentUser || !selectedAgentName) return
    const dmName = dmConversationNameForParticipants(currentUser, selectedAgentName)
    if (dmChannels.some((ch: ChannelInfo) => ch.name === dmName)) return

    let cancelled = false
    ensureDirectMessageConversation(selectedAgentName)
      .then((channel) => {
        if (cancelled) return
        queryClient.setQueryData<ChannelInfo[]>(qk.channels(currentUser), (current = []) => {
          if (current.some((ch: ChannelInfo) => ch.id === channel.id || ch.name === channel.name)) {
            return current
          }
          return [...current, channel]
        })
        updateInboxState((current: InboxState) => ensureInboxConversations(current, [channel]))
      })
      .catch((error) => {
        if (!cancelled) console.error('Failed to ensure DM conversation', error)
      })

    return () => {
      cancelled = true
    }
  }, [currentUser, dmChannels, selectedAgentName, queryClient, updateInboxState])

  // ── WebSocket: inbox unread tracking ──────────────────────────────────────

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
          const threadRaw = frame.event.payload.threadParentId
          const threadParentId =
            typeof threadRaw === 'string' && threadRaw.length > 0 ? threadRaw : undefined
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

  // ── Refresh helpers (invalidate TanStack Query caches) ────────────────────

  const conversationThreadsInFlight = useRef<Map<string, Promise<void>>>(new Map())

  const refreshConversationThreads = useCallback(
    async (conversationId: string) => {
      if (!currentUser) return
      const inFlight = conversationThreadsInFlight.current
      const existing = inFlight.get(conversationId)
      if (existing) return existing
      const promise = (async () => {
        try {
          const response = await getChannelThreads(conversationId)
          setConversationThreads(conversationId, response.threads)
        } catch (error) {
          console.error('Failed to load channel threads', error)
        } finally {
          inFlight.delete(conversationId)
        }
      })()
      inFlight.set(conversationId, promise)
      return promise
    },
    [currentUser, setConversationThreads]
  )

  const refreshChannels = useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: qk.channels(currentUser) })
  }, [currentUser, queryClient])

  const refreshAgents = useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: qk.agents })
  }, [queryClient])

  const refreshTeams = useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: qk.teams })
  }, [queryClient])

  const refreshServerInfo = useCallback(async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: qk.agents }),
      queryClient.invalidateQueries({ queryKey: qk.channels(currentUser) }),
      queryClient.invalidateQueries({ queryKey: qk.teams }),
      queryClient.invalidateQueries({ queryKey: qk.humans }),
    ])
  }, [currentUser, queryClient])

  // ── Derived inbox helpers ─────────────────────────────────────────────────

  const getConversationUnread = useCallback(
    (conversationId?: string | null) => {
      if (!conversationId) return 0
      return inboxState.conversations[conversationId]?.unreadCount ?? 0
    },
    [inboxState.conversations]
  )

  const getConversationThreadUnreadCount = useCallback(
    (conversationId?: string | null) => {
      if (!conversationId) return 0
      return inboxState.conversations[conversationId]?.threadUnreadCount ?? 0
    },
    [inboxState.conversations]
  )

  const getConversationThreads = useCallback(
    (conversationId?: string | null) => {
      if (!conversationId) return []
      return mergeChannelThreadInboxEntries(
        conversationThreads[conversationId] ?? [],
        inboxState,
        conversationId
      )
    },
    [conversationThreads, inboxState]
  )

  const getConversationThreadUnread = useCallback(
    (conversationId?: string | null) => {
      if (!conversationId) return 0
      const cached = conversationThreads[conversationId]
      if (cached && cached.length > 0) {
        return mergeChannelThreadInboxEntries(cached, inboxState, conversationId).reduce(
          (sum, entry) => sum + entry.unreadCount,
          0
        )
      }
      return conversationThreadUnreadCount(inboxState, conversationId)
    },
    [conversationThreads, inboxState]
  )

  const getAgentUnread = useCallback(
    (agentName: string) => {
      const dmName = dmConversationNameForParticipants(currentUser, agentName)
      const conversationId = dmChannels.find((ch: ChannelInfo) => ch.name === dmName)?.id ?? null
      return getConversationUnread(conversationId)
    },
    [currentUser, dmChannels, getConversationUnread]
  )

  const getAgentConversationId = useCallback(
    (agentName: string) => {
      const dmName = dmConversationNameForParticipants(currentUser, agentName)
      return dmChannels.find((ch: ChannelInfo) => ch.name === dmName)?.id ?? null
    },
    [currentUser, dmChannels]
  )

  // ── applyReadCursorAck: update inbox state + refresh threads ──────────────

  const applyReadCursorAck = useCallback(
    (ack: ReadCursorAckPayload) => {
      storeApplyReadCursorAck(ack)
      if (ack.threadParentId) {
        void refreshConversationThreads(ack.conversationId)
      }
    },
    [storeApplyReadCursorAck, refreshConversationThreads]
  )

  // ── Derive selectedAgent from agents list + stored name ───────────────────

  const selectedAgent = selectedAgentName
    ? (agents.find((a: AgentInfo) => a.name === selectedAgentName) ?? null)
    : null

  // Expose setSelectedAgent accepting AgentInfo | null (same public API as before)
  const setSelectedAgent = useCallback(
    (agent: AgentInfo | null) => setSelectedAgentName(agent?.name ?? null),
    [setSelectedAgentName]
  )

  const serverInfo: ServerInfo | null =
    humans.length > 0 || systemChannels.length > 0
      ? { system_channels: systemChannels, humans }
      : null

  const serverInfoLoading = !shellBootstrapped

  return (
    <AppContext.Provider
      value={{
        currentUser,
        serverInfo,
        channels,
        agents,
        teams,
        serverInfoLoading,
        selectedChannel,
        selectedChannelId,
        selectedAgent,
        activeTab,
        getConversationUnread,
        getConversationThreads,
        getConversationThreadUnread,
        getConversationThreadUnreadCount,
        getAgentUnread,
        getAgentConversationId,
        applyReadCursorAck,
        setSelectedChannel,
        setSelectedAgent,
        setActiveTab,
        openThreadMsg,
        setOpenThreadMsg,
        refreshServerInfo,
        refreshChannels,
        refreshConversationThreads,
        refreshAgents,
        refreshTeams,
      }}
    >
      {children}
    </AppContext.Provider>
  )
}

export function useApp(): AppState {
  const ctx = useContext(AppContext)
  if (!ctx) throw new Error('useApp must be used inside AppProvider')
  return ctx
}

export function useTarget(): string | null {
  const { selectedChannel, selectedAgent } = useApp()
  if (selectedChannel) return selectedChannel
  if (selectedAgent) return `dm:@${selectedAgent.name}`
  return null
}
