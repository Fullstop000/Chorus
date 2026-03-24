import React, { createContext, useContext, useState, useEffect, useCallback, useRef } from 'react'
import type { ServerInfo, AgentInfo, HistoryMessage } from './types'
import { getWhoami, getServerInfo } from './api'

export type ActiveTab = 'chat' | 'tasks' | 'workspace' | 'activity' | 'profile'

export interface AppState {
  currentUser: string                  // OS username from /api/whoami
  serverInfo: ServerInfo | null
  serverInfoLoading: boolean
  selectedChannel: string | null       // e.g. "#all"
  selectedChannelId: string | null
  selectedAgent: AgentInfo | null      // non-null when viewing a DM with an agent
  activeTab: ActiveTab
  openThreadMsg: HistoryMessage | null // non-null when thread panel is open
  setSelectedChannel: (ch: string | null, channelId?: string | null) => void
  setSelectedAgent: (agent: AgentInfo | null) => void
  setActiveTab: (tab: ActiveTab) => void
  setOpenThreadMsg: (msg: HistoryMessage | null) => void
  refreshServerInfo: () => void
}

const AppContext = createContext<AppState | null>(null)

export function AppProvider({ children }: { children: React.ReactNode }) {
  const [currentUser, setCurrentUser] = useState('')
  const [serverInfo, setServerInfo] = useState<ServerInfo | null>(null)
  const [serverInfoLoading, setServerInfoLoading] = useState(true)
  const [selectedChannel, setSelectedChannel] = useState<string | null>(null)
  const [selectedChannelId, setSelectedChannelId] = useState<string | null>(null)
  const [selectedAgent, setSelectedAgent] = useState<AgentInfo | null>(null)
  const [activeTab, setActiveTab] = useState<ActiveTab>('chat')
  const [openThreadMsg, setOpenThreadMsg] = useState<HistoryMessage | null>(null)
  // Ref so refreshServerInfo can check selectedAgent without re-creating the callback
  const selectedAgentRef = useRef<AgentInfo | null>(null)
  const selectedChannelRef = useRef<string | null>(null)
  const selectedChannelIdRef = useRef<string | null>(null)

  // Fetch current user once on mount
  useEffect(() => {
    getWhoami()
      .then((r) => setCurrentUser(r.username))
      .catch(() => setCurrentUser('user'))
  }, [])

  const refreshServerInfo = useCallback(() => {
    if (!currentUser) return
    setServerInfoLoading(true)
    getServerInfo(currentUser)
      .then((info) => {
        setServerInfo(info)
        setSelectedAgent((prev) => {
          if (!prev) return prev
          return info.agents.find((agent) => agent.name === prev.name) ?? null
        })
        if (selectedAgentRef.current) return

        const joinedChannels = [
          ...info.system_channels.filter((c) => c.joined),
          ...info.channels.filter((c) => c.joined),
        ]
        let nextSelected = null as string | null
        let nextSelectedId = null as string | null

        if (selectedChannelIdRef.current) {
          const match = joinedChannels.find((channel) => channel.id === selectedChannelIdRef.current)
          if (match) {
            nextSelected = `#${match.name}`
            nextSelectedId = match.id ?? null
          }
        } else if (selectedChannelRef.current) {
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
      })
      .catch(console.error)
      .finally(() => setServerInfoLoading(false))
  }, [currentUser])

  // Poll server info every 10s
  useEffect(() => {
    if (!currentUser) return
    refreshServerInfo()
    const id = setInterval(refreshServerInfo, 5_000)
    return () => clearInterval(id)
  }, [currentUser, refreshServerInfo])

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
    }
  }, [])

  return (
    <AppContext.Provider
      value={{
        currentUser,
        serverInfo,
        serverInfoLoading,
        selectedChannel,
        selectedChannelId,
        selectedAgent,
        activeTab,
        setSelectedChannel: handleSetSelectedChannel,
        setSelectedAgent: handleSetSelectedAgent,
        setActiveTab,
        openThreadMsg,
        setOpenThreadMsg,
        refreshServerInfo,
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
