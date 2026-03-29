import React, { createContext, useContext, useState, useEffect, useCallback, useRef } from 'react'
import type { ServerInfo, AgentInfo, ChannelInfo, HistoryMessage, Team } from './types'
import { getWhoami, getServerInfo, listAgents, listChannels, listTeams, resolveChannel, getInboxState } from './api'
import { applyInboxEvent, bootstrapInboxState, buildConversationRegistry, createInboxState, dmConversationNameForParticipants } from './inbox'
import { isVisibleSidebarChannel } from './sidebarChannels'
import { nextRealtimeCursor } from './transport/realtime'
import { getRealtimeSession } from './transport/realtimeSession'

export type ActiveTab = 'chat' | 'tasks' | 'workspace' | 'activity' | 'profile'

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
  getAgentUnread: (agentName: string) => number
  setSelectedChannel: (ch: string | null, channelId?: string | null) => void
  setSelectedAgent: (agent: AgentInfo | null) => void
  setActiveTab: (tab: ActiveTab) => void
  setOpenThreadMsg: (msg: HistoryMessage | null) => void
  refreshServerInfo: () => Promise<void>
  refreshChannels: () => Promise<void>
  refreshAgents: () => Promise<void>
  refreshTeams: () => Promise<void>
}

const AppContext = createContext<AppState | null>(null)

export function AppProvider({ children }: { children: React.ReactNode }) {
  const [currentUser, setCurrentUser] = useState('')
  const [serverInfo, setServerInfo] = useState<ServerInfo | null>(null)
  const [channels, setChannels] = useState<ChannelInfo[]>([])
  const [systemConversationChannels, setSystemConversationChannels] = useState<ChannelInfo[]>([])
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
  const [shellBootstrapped, setShellBootstrapped] = useState(false)
  // Ref so refreshServerInfo can check selectedAgent without re-creating the callback
  const selectedAgentRef = useRef<AgentInfo | null>(null)
  const selectedChannelRef = useRef<string | null>(null)
  const selectedChannelIdRef = useRef<string | null>(null)
  const inboxCursorRef = useRef(0)

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

  const applyConversationLists = useCallback((
    info: ServerInfo,
    allConversationChannels: ChannelInfo[]
  ) => {
    const loadedChannels = allConversationChannels.filter(
      (channel) => channel.channel_type !== 'dm' && channel.channel_type !== 'system'
    )
    const loadedSystemConversationChannels = allConversationChannels.filter(
      (channel) => channel.channel_type === 'system'
    )
    const loadedDmChannels = allConversationChannels.filter(
      (channel) => channel.channel_type === 'dm'
    )

    setServerInfo(info)
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
      const [info, allConversationChannels] = await Promise.all([
        getServerInfo(currentUser),
        listChannels({ member: currentUser, includeDm: true, includeSystem: true }),
      ])
      applyConversationLists(info, allConversationChannels)
    } catch (error) {
      console.error('Failed to load channels', error)
    }
  }, [applyConversationLists, currentUser])

  const refreshAgents = useCallback(async () => {
    if (!currentUser) return
    try {
      const loadedAgents = await listAgents()
      await Promise.all(
        loadedAgents.map((agent) =>
          resolveChannel(currentUser, `dm:@${agent.name}`).catch((error) => {
            console.error('Failed to pre-resolve agent DM', error)
            return null
          })
        )
      )
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
      const [info, loadedAgents, loadedTeams] = await Promise.all([
        getServerInfo(currentUser),
        listAgents(),
        listTeams(),
      ])
      await Promise.all(
        loadedAgents.map((agent) =>
          resolveChannel(currentUser, `dm:@${agent.name}`).catch((error) => {
            console.error('Failed to pre-resolve agent DM', error)
            return null
          })
        )
      )
      const [allConversationChannels, inbox] = await Promise.all([
        listChannels({ member: currentUser, includeDm: true, includeSystem: true }),
        getInboxState(currentUser),
      ])
      applyConversationLists(info, allConversationChannels)
      setAgents(loadedAgents)
      setTeams(loadedTeams)
      setInboxState(bootstrapInboxState(inbox.conversations))
      inboxCursorRef.current = inbox.latestEventId ?? 0
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
      inboxCursorRef.current = 0
      setInboxState(createInboxState())
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
    const targets = [
      ...conversationRegistry.map((entry) => `conversation:${entry.conversationId}`),
      'workspace:default',
      `inbox:${currentUser}`,
    ]
    if (targets.length === 0) return

    return getRealtimeSession(currentUser).subscribe({
      targets,
      resumeFrom: inboxCursorRef.current,
      onFrame: (frame) => {
        if (frame.type === 'subscribed') {
          inboxCursorRef.current = nextRealtimeCursor(inboxCursorRef.current, frame)
          return
        }
        if (frame.type === 'error') {
          console.error('Inbox realtime subscription failed', frame.message)
          return
        }
        inboxCursorRef.current = nextRealtimeCursor(inboxCursorRef.current, frame)
        if (frame.type === 'event' && frame.event.streamId === 'workspace:default') {
          switch (frame.event.eventType) {
            case 'conversation.membership_changed':
            case 'conversation.archived':
            case 'conversation.deleted':
              void refreshChannels()
              return
            case 'team.updated':
              void Promise.all([refreshTeams(), refreshChannels()])
              return
            case 'agent.updated':
              void refreshAgents()
              return
            default:
              return
          }
        }
        setInboxState((current) => applyInboxEvent(current, frame.event))
      },
    })
  }, [agents, channels, currentUser, dmChannels, refreshAgents, refreshChannels, refreshTeams, shellBootstrapped, systemConversationChannels])

  // Keep ref in sync so refreshServerInfo can read it without stale closure
  useEffect(() => { selectedAgentRef.current = selectedAgent }, [selectedAgent])
  useEffect(() => { selectedChannelRef.current = selectedChannel }, [selectedChannel])
  useEffect(() => { selectedChannelIdRef.current = selectedChannelId }, [selectedChannelId])

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
      setActiveTab('chat')
      if (!channelId && currentUser) {
        void resolveChannel(currentUser, ch)
          .then((response) => {
            if (selectedChannelRef.current === ch) {
              setSelectedChannelId(response.channelId)
            }
          })
          .catch((error) => {
            console.error('Failed to resolve channel', error)
          })
      }
    }
  }, [currentUser])

  const getConversationUnread = useCallback((conversationId?: string | null) => {
    if (!conversationId) return 0
    return inboxState.conversations[conversationId]?.unreadCount ?? 0
  }, [inboxState.conversations])

  const getAgentUnread = useCallback((agentName: string) => {
    const dmChannelName = dmConversationNameForParticipants(currentUser, agentName)
    const conversationId =
      dmChannels.find((channel) => channel.name === dmChannelName)?.id ?? null
    return getConversationUnread(conversationId)
  }, [currentUser, dmChannels, getConversationUnread])

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
        getAgentUnread,
        setSelectedChannel: handleSetSelectedChannel,
        setSelectedAgent: handleSetSelectedAgent,
        setActiveTab,
        openThreadMsg,
        setOpenThreadMsg,
        refreshServerInfo,
        refreshChannels,
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
