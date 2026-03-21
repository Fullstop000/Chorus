import React, { createContext, useContext, useState, useEffect, useCallback } from 'react'
import type { ServerInfo, AgentInfo } from './types'
import { getWhoami, getServerInfo } from './api'

export type ActiveTab = 'chat' | 'tasks' | 'workspace' | 'activity' | 'profile'

export interface AppState {
  currentUser: string                  // OS username from /api/whoami
  serverInfo: ServerInfo | null
  serverInfoLoading: boolean
  selectedChannel: string | null       // e.g. "#general"
  selectedAgent: AgentInfo | null      // non-null when viewing a DM with an agent
  activeTab: ActiveTab
  setSelectedChannel: (ch: string | null) => void
  setSelectedAgent: (agent: AgentInfo | null) => void
  setActiveTab: (tab: ActiveTab) => void
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
        // Auto-select first joined channel if nothing selected
        setSelectedChannel((prev) => {
          if (prev) return prev
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

  // When selecting an agent, switch to chat tab
  const handleSetSelectedAgent = useCallback((agent: AgentInfo | null) => {
    setSelectedAgent(agent)
    if (agent) {
      setSelectedChannel(null)
      setActiveTab('chat')
    }
  }, [])

  const handleSetSelectedChannel = useCallback((ch: string | null) => {
    setSelectedChannel(ch)
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
