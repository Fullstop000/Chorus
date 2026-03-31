import React, { createContext, useContext, useState, useEffect, useCallback, useRef } from 'react'
import type { ServerInfo, AgentInfo, ChannelInfo, HistoryMessage, Team, ThreadInboxEntry, HumanInfo } from './types'
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
  createInboxState,
  dmConversationNameForParticipants,
  ensureInboxConversations,
  mergeChannelThreadInboxEntries,
  mergeInboxNotificationRefresh,
  mergeReadCursorAckIntoInboxState,
  type ReadCursorAckPayload,
} from './inbox'
import { isVisibleSidebarChannel } from './sidebarChannels'
import { getRealtimeSession } from './transport/realtimeSession'

export type ActiveTab = 'chat' | 'threads' | 'tasks' | 'workspace' | 'activity' | 'profile'

export interface AppState {
  currentUser: string                  // OS username from /api/whoami
  serverInfo: ServerInfo | null
  channels: ChannelInfo[]
  agents: AgentInfo[]
  teams: Team[]
  serverInfoLoading: boolean
  selectedChannel: string | null       // e.g. "#all"
  selectedChannelId: string | null
  selectedAgent: AgentInfo | null      // non-null when viewing a DM with an agent
  activeTab: ActiveTab
  openThreadMsg: HistoryMessage | null // non-null when thread panel is open
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

export function AppProvider({ children }: { children: React.ReactNode }) {
  const [currentUser, setCurrentUser] = useState('')
  const [channels, setChannels] = useState<ChannelInfo[]>([])
  const [systemConversationChannels, setSystemConversationChannels] = useState<ChannelInfo[]>([])
  const [humans, setHumans] = useState<HumanInfo[]>([])
  const [dmChannels, setDmChannels] = useState<ChannelInfo[]>([])
  const [agents, setAgents] = useState<AgentInfo[]>([])
  const [teams, setTeams] = useState<Team[]>([])
  const [serverInfoLoading, setServerInfoLoading] = useState(true)
  const [selectedChannel, setSelectedChannel] = useState<string | null>(null)
  const [selectedChannelId, setSelectedChannelId] = useState<string | null>(null)
  const [selectedAgent, setSelectedAgent] = useState<AgentInfo | null>(null)
  const [activeTab, setActiveTab] = useState<ActiveTab>('chat')
  const [openThreadMsg, setOpenThreadMsg] = useState<HistoryMessage | null>(null)
  const [inboxState, setInboxState] = useState(createInboxState)
  const [conversationThreads, setConversationThreads] = useState<Record<string, ThreadInboxEntry[]>>({})
  const [shellBootstrapped, setShellBootstrapped] = useState(false)
  // Ref so refreshServerInfo can check selectedAgent without re-creating the callback
  const selectedAgentRef = useRef<AgentInfo | null>(null)
  const selectedChannelRef = useRef<string | null>(null)
  const selectedChannelIdRef = useRef<string | null>(null)
  const conversationThreadsRefreshInFlight = useRef<Map<string, Promise<void>>>(new Map())

  // Fetch current user once on mount
  useEffect(() => {
    getWhoami()
      .then((r) => setCurrentUser(r.username))
      .catch(() => setCurrentUser('user'))
  }, [])

  useEffect(() => {
    setShellBootstrapped(false)
  }, [currentUser])

  const refreshTeams = useCallback(async () => {
    try {
      setTeams(await listTeams())
    } catch (error) {
      console.error('Failed to load teams', error)
    }
  }, [])

  const applyConversationLists = useCallback((allConversationChannels: ChannelInfo[]) => {
    const loadedChannels = allConversationChannels.filter(
      (channel) => channel.channel_type !== 'dm' && channel.channel_type !== 'system'
    )
    const loadedSystemConversationChannels = allConversationChannels.filter(
      (channel) => channel.channel_type === 'system'
    )
    const loadedDmChannels = allConversationChannels.filter(
      (channel) => channel.channel_type === 'dm'
    )

    setChannels(loadedChannels)
    setSystemConversationChannels(loadedSystemConversationChannels)
    setDmChannels(loadedDmChannels)

    if (selectedAgentRef.current) return

    const joinedChannels = [
      ...loadedSystemConversationChannels.filter((channel) => channel.joined),
      ...loadedChannels.filter(isVisibleSidebarChannel),
    ]
    let nextSelected = null as string | null
    let nextSelectedId = null as string | null

    if (selectedChannelIdRef.current) {
      const match = joinedChannels.find((channel) => channel.id === selectedChannelIdRef.current)
      if (match) {
        nextSelected = `#${match.name}`
        nextSelectedId = match.id ?? null
      }
    }
    if (!nextSelected && selectedChannelRef.current) {
      const match = joinedChannels.find(
        (channel) => `#${channel.name}` === selectedChannelRef.current
      )
      if (match) {
        nextSelected = `#${match.name}`
        nextSelectedId = match.id ?? null
      }
    }

    if (!nextSelected) {
      const first = joinedChannels[0]
      nextSelected = first ? `#${first.name}` : null
      nextSelectedId = first?.id ?? null
    }

    setSelectedChannel(nextSelected)
    setSelectedChannelId(nextSelectedId)
  }, [])

  const refreshChannels = useCallback(async () => {
    if (!currentUser) return
    try {
      const allConversationChannels = await listChannels({
        member: currentUser,
        includeDm: true,
        includeSystem: true,
      })
      applyConversationLists(allConversationChannels)
      setInboxState((current) => ensureInboxConversations(current, allConversationChannels))
    } catch (error) {
      console.error('Failed to load channels', error)
    }
  }, [applyConversationLists, currentUser])

  const refreshAgents = useCallback(async () => {
    if (!currentUser) return
    try {
      const loadedAgents = await listAgents()
      setAgents(loadedAgents)
      setSelectedAgent((prev) => {
        if (!prev) return prev
        return loadedAgents.find((agent) => agent.name === prev.name) ?? null
      })
    } catch (error) {
      console.error('Failed to load agents', error)
    }
  }, [currentUser])

  const refreshServerInfo = useCallback(async () => {
    if (!currentUser) return
    setServerInfoLoading(true)
    try {
      const [loadedAgents, loadedTeams, loadedHumans] = await Promise.all([
        listAgents(),
        listTeams(),
        listHumans(),
      ])
      const [allConversationChannels, inbox] = await Promise.all([
        listChannels({ member: currentUser, includeDm: true, includeSystem: true }),
        getInboxState(currentUser),
      ])
      applyConversationLists(allConversationChannels)
      setAgents(loadedAgents)
      setTeams(loadedTeams)
      setHumans(loadedHumans)
      setInboxState(bootstrapInboxState(inbox.conversations, allConversationChannels))
      setSelectedAgent((prev) => {
        if (!prev) return prev
        return loadedAgents.find((agent) => agent.name === prev.name) ?? null
      })
    } catch (error) {
      console.error(error)
    } finally {
      setServerInfoLoading(false)
      setShellBootstrapped(true)
    }
  }, [applyConversationLists, currentUser])

  // Bootstrap the shell once; follow-up refreshes happen on explicit mutations.
  useEffect(() => {
    if (!currentUser) return
    void refreshServerInfo()
  }, [currentUser, refreshServerInfo])

  useEffect(() => {
    if (!currentUser) {
      setInboxState(createInboxState())
      setConversationThreads({})
      setShellBootstrapped(false)
      return
    }
    if (!shellBootstrapped) return

    const conversationRegistry = buildConversationRegistry({
      currentUser,
      systemChannels: systemConversationChannels,
      channels,
      dmChannels,
      agents,
    })
    const targets = conversationRegistry.map((entry) => `conversation:${entry.conversationId}`)
    if (targets.length === 0) return

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
          void getConversationInboxNotification(channelId, threadParentId)
            .then((payload) => {
              setInboxState((current) => mergeInboxNotificationRefresh(current, payload))
            })
            .catch((error) => {
              console.error('Failed to refresh inbox after message', error)
            })
          return
        }
      },
    })
  }, [agents, channels, currentUser, dmChannels, refreshAgents, refreshChannels, refreshTeams, shellBootstrapped, systemConversationChannels])

  useEffect(() => {
    if (!currentUser || !selectedAgent) return
    const dmChannelName = dmConversationNameForParticipants(currentUser, selectedAgent.name)
    if (dmChannels.some((channel) => channel.name === dmChannelName)) return

    let cancelled = false
    ensureDirectMessageConversation(selectedAgent.name)
      .then((channel) => {
        if (cancelled) return
        setDmChannels((current) => {
          if (current.some((entry) => entry.id === channel.id || entry.name === channel.name)) {
            return current
          }
          return [...current, channel]
        })
        setInboxState((current) => ensureInboxConversations(current, [channel]))
      })
      .catch((error) => {
        if (!cancelled) {
          console.error('Failed to ensure direct-message conversation', error)
        }
      })

    return () => {
      cancelled = true
    }
  }, [currentUser, dmChannels, selectedAgent])

  // Keep ref in sync so refreshServerInfo can read it without stale closure
  useEffect(() => { selectedAgentRef.current = selectedAgent }, [selectedAgent])
  useEffect(() => { selectedChannelRef.current = selectedChannel }, [selectedChannel])
  useEffect(() => { selectedChannelIdRef.current = selectedChannelId }, [selectedChannelId])

  const refreshConversationThreads = useCallback(async (conversationId: string) => {
    if (!currentUser) return
    const inFlight = conversationThreadsRefreshInFlight.current
    const existing = inFlight.get(conversationId)
    if (existing) return existing
    const promise = (async () => {
      try {
        const response = await getChannelThreads(conversationId)
        setConversationThreads((current) => ({
          ...current,
          [conversationId]: response.threads,
        }))
      } catch (error) {
        console.error('Failed to load channel threads', error)
      } finally {
        inFlight.delete(conversationId)
      }
    })()
    inFlight.set(conversationId, promise)
    return promise
  }, [currentUser])

  // When selecting an agent, switch to chat tab and close thread
  const handleSetSelectedAgent = useCallback((agent: AgentInfo | null) => {
    setSelectedAgent(agent)
    setOpenThreadMsg(null)
    if (agent) {
      setSelectedChannel(null)
      setSelectedChannelId(null)
      setActiveTab('chat')
    }
  }, [])

  const handleSetSelectedChannel = useCallback((ch: string | null, channelId?: string | null) => {
    setSelectedChannel(ch)
    setSelectedChannelId(ch ? channelId ?? null : null)
    setOpenThreadMsg(null)
    if (ch) {
      setSelectedAgent(null)
      if (activeTab === 'workspace' || activeTab === 'activity' || activeTab === 'profile') {
        setActiveTab('chat')
      }
    }
  }, [activeTab])

  const getConversationUnread = useCallback((conversationId?: string | null) => {
    if (!conversationId) return 0
    return inboxState.conversations[conversationId]?.unreadCount ?? 0
  }, [inboxState.conversations])

  const getConversationThreadUnreadCount = useCallback((conversationId?: string | null) => {
    if (!conversationId) return 0
    return inboxState.conversations[conversationId]?.threadUnreadCount ?? 0
  }, [inboxState.conversations])

  const getConversationThreads = useCallback((conversationId?: string | null) => {
    if (!conversationId) return []
    return mergeChannelThreadInboxEntries(
      conversationThreads[conversationId] ?? [],
      inboxState,
      conversationId
    )
  }, [conversationThreads, inboxState])

  const getConversationThreadUnread = useCallback((conversationId?: string | null) => {
    if (!conversationId) return 0
    const cachedThreads = conversationThreads[conversationId]
    if (cachedThreads && cachedThreads.length > 0) {
      return mergeChannelThreadInboxEntries(cachedThreads, inboxState, conversationId)
        .reduce((sum, entry) => sum + entry.unreadCount, 0)
    }
    return conversationThreadUnreadCount(inboxState, conversationId)
  }, [conversationThreads, inboxState])

  const getAgentUnread = useCallback((agentName: string) => {
    const dmChannelName = dmConversationNameForParticipants(currentUser, agentName)
    const conversationId =
      dmChannels.find((channel) => channel.name === dmChannelName)?.id ?? null
    return getConversationUnread(conversationId)
  }, [currentUser, dmChannels, getConversationUnread])

  const getAgentConversationId = useCallback((agentName: string) => {
    const dmChannelName = dmConversationNameForParticipants(currentUser, agentName)
    return dmChannels.find((channel) => channel.name === dmChannelName)?.id ?? null
  }, [currentUser, dmChannels])

  const applyReadCursorAck = useCallback((ack: ReadCursorAckPayload) => {
    setInboxState((current) => mergeReadCursorAckIntoInboxState(current, ack))
    // If this is a thread read cursor update, refresh the threads list to update unread counts
    if (ack.threadParentId) {
      void refreshConversationThreads(ack.conversationId)
    }
  }, [refreshConversationThreads])

  const serverInfoValue: ServerInfo | null =
    humans.length > 0 || systemConversationChannels.length > 0
      ? { system_channels: systemConversationChannels, humans }
      : null

  return (
    <AppContext.Provider
      value={{
        currentUser,
        serverInfo: serverInfoValue,
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
        setSelectedChannel: handleSetSelectedChannel,
        setSelectedAgent: handleSetSelectedAgent,
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

// Derive the active "target" string for API calls
export function useTarget(): string | null {
  const { selectedChannel, selectedAgent } = useApp()
  if (selectedChannel) return selectedChannel
  if (selectedAgent) return `dm:@${selectedAgent.name}`
  return null
}
