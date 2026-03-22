import React, { createContext, useContext, useState, useEffect, useCallback, useRef } from 'react'
import type { ServerInfo, AgentInfo, HistoryMessage } from './types'
import { getWhoami, getServerInfo } from './api'

export type ActiveTab = 'chat' | 'tasks' | 'workspace' | 'activity' | 'profile'

export interface AppState {
  currentUser: string                  // OS username from /api/whoami
  serverInfo: ServerInfo | null
  serverInfoLoading: boolean
  selectedChannel: string | null       // e.g. "#general"
  selectedAgent: AgentInfo | null      // non-null when viewing a DM with an agent
  activeTab: ActiveTab
  openThreadMsg: HistoryMessage | null // non-null when thread panel is open
  setSelectedChannel: (ch: string | null) => void
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
  const [selectedAgent, setSelectedAgent] = useState<AgentInfo | null>(null)
  const [activeTab, setActiveTab] = useState<ActiveTab>('chat')
  const [openThreadMsg, setOpenThreadMsg] = useState<HistoryMessage | null>(null)
  // Ref so refreshServerInfo can check selectedAgent without re-creating the callback
  const selectedAgentRef = useRef<AgentInfo | null>(null)

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
        // Auto-select first joined channel only if nothing is selected (no channel AND no agent)
        setSelectedChannel((prev) => {
          if (prev || selectedAgentRef.current) return prev
          const first = info.channels.find((c) => c.joined)
          return first ? `#${first.name}` : null
        })
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

  // When selecting an agent, switch to chat tab and close thread
  const handleSetSelectedAgent = useCallback((agent: AgentInfo | null) => {
    setSelectedAgent(agent)
    setOpenThreadMsg(null)
    if (agent) {
      setSelectedChannel(null)
      setActiveTab('chat')
    }
  }, [])

  const handleSetSelectedChannel = useCallback((ch: string | null) => {
    setSelectedChannel(ch)
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
