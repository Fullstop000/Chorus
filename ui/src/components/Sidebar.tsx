import { useEffect, useRef, useState } from 'react'
import { Ellipsis, Pencil, Plus, Settings2, Trash2, Users } from 'lucide-react'
import { useApp } from '../store'
import type { AgentInfo, ChannelInfo } from '../types'
import { isVisibleSidebarChannel } from '../sidebarChannels'
import { CreateAgentModal } from './CreateAgentModal'
import { CreateChannelModal } from './Channels/CreateChannelModal'
import { DeleteChannelModal, EditChannelModal } from './EditChannelModal'
import './Sidebar.css'

function agentColor(name: string): string {
  const colors = ['#FF6B6B','#4ECDC4','#45B7D1','#96CEB4','#FFEAA7','#DDA0DD','#98D8C8']
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff
  return colors[Math.abs(h) % colors.length]
}

function agentDotClass(status: string, activity?: string): string {
  if (status !== 'active') return status === 'sleeping' ? 'offline' : 'offline'
  if (activity === 'thinking' || activity === 'working') return activity
  return 'online'
}

function AgentAvatar({ name, status, activity }: { name: string; status: string; activity?: string }) {
  const color = agentColor(name)
  const initial = name[0]?.toUpperCase() ?? '?'
  const dotClass = agentDotClass(status, activity)
  return (
    <div className="agent-avatar" style={{ position: 'relative' }}>
      <div
        className="agent-avatar-img"
        style={{
          background: color,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          fontSize: 12,
          fontWeight: 700,
          color: '#fff',
          fontFamily: 'var(--font-mono)',
        }}
      >
        {initial}
      </div>
      <span className={`status-dot ${dotClass}`} />
    </div>
  )
}

export function Sidebar() {
  const {
    currentUser,
    serverInfo,
    channels: loadedChannels,
    agents,
    selectedChannel,
    selectedChannelId,
    selectedAgent,
    setSelectedChannel,
    setSelectedAgent,
    getConversationUnread,
    getConversationThreadUnreadCount,
    getAgentUnread,
    getAgentConversationId,
    refreshChannels,
    refreshAgents,
    refreshTeams,
  } = useApp()
  const [showCreateAgent, setShowCreateAgent] = useState(false)
  const [showCreateChannel, setShowCreateChannel] = useState(false)
  const [createModalMode, setCreateModalMode] = useState<'channel' | 'team'>('channel')
  const [editingChannel, setEditingChannel] = useState<ChannelInfo | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<ChannelInfo | null>(null)
  const [openChannelMenuId, setOpenChannelMenuId] = useState<string | null>(null)
  const menuRef = useRef<HTMLDivElement | null>(null)
  const [channelsCollapsed, setChannelsCollapsed] = useState(false)
  const [agentsCollapsed, setAgentsCollapsed] = useState(false)
  const [humansCollapsed, setHumansCollapsed] = useState(false)

  const channels = loadedChannels.filter(isVisibleSidebarChannel)
  const systemChannels = serverInfo?.system_channels ?? []
  const humans = serverInfo?.humans ?? []

  useEffect(() => {
    function handlePointerDown(event: MouseEvent) {
      if (!menuRef.current?.contains(event.target as Node)) {
        setOpenChannelMenuId(null)
      }
    }
    document.addEventListener('mousedown', handlePointerDown)
    return () => document.removeEventListener('mousedown', handlePointerDown)
  }, [])

  function selectChannel(channel: ChannelInfo | null) {
    if (!channel) {
      setSelectedChannel(null)
      return
    }
    setSelectedChannel(`#${channel.name}`, channel.id ?? null)
  }

  function recoverSelectionAfterChannelRemoval(channelId?: string) {
    if (!channelId || selectedChannelId !== channelId) return
    const fallback = channels.find((channel) => channel.id !== channelId) ?? null
    selectChannel(fallback)
  }

  return (
    <>
      <nav className="sidebar">
        <div className="sidebar-header">
          <div className="sidebar-server-block">
            <span className="sidebar-server-label">[chorus::workspace]</span>
            <span className="sidebar-server-name">Chorus</span>
          </div>
        </div>

        <div className="sidebar-body">
          <div className="sidebar-section">
            <div className="sidebar-section-header" onClick={() => setChannelsCollapsed(!channelsCollapsed)}>
              <span className="sidebar-section-label">Channels</span>
              <div style={{ display: 'flex', gap: 4 }}>
                <button
                  type="button"
                  className="sidebar-add-btn"
                  title="Add channel"
                  onClick={(e) => {
                    e.stopPropagation()
                    setCreateModalMode('channel')
                    setShowCreateChannel(true)
                  }}
                >
                  <Plus size={14} />
                </button>
                <button
                  type="button"
                  className="sidebar-add-btn"
                  title="Add team"
                  onClick={(e) => {
                    e.stopPropagation()
                    setCreateModalMode('team')
                    setShowCreateChannel(true)
                  }}
                >
                  <Users size={14} />
                </button>
              </div>
            </div>
            {!channelsCollapsed && systemChannels.map((ch) => {
              const target = `#${ch.name}`
              const unreadCount = getConversationUnread(ch.id ?? null)
              const threadUnreadCount = getConversationThreadUnreadCount(ch.id ?? null)
              const showUnreadBadge = unreadCount > 0
              const showThreadDot = !showUnreadBadge && threadUnreadCount > 0
              return (
                <button
                  key={ch.id ?? ch.name}
                  type="button"
                  className={`sidebar-item${selectedChannel === target ? ' active' : ''}`}
                  onClick={() => setSelectedChannel(target, ch.id ?? null)}
                  title={ch.description ?? ch.name}
                >
                  <span className="sidebar-item-hash">#</span>
                  <span className="sidebar-item-main">
                    <span className="sidebar-item-text">{ch.name}</span>
                    {ch.id && <span className="sidebar-item-meta sidebar-item-id">{ch.id}</span>}
                  </span>
                  {showUnreadBadge && (
                    <span className="sidebar-unread-badge">{unreadCount}</span>
                  )}
                  {showThreadDot && (
                    <span className="sidebar-thread-unread-dot" title={`${threadUnreadCount} unread in threads`} />
                  )}
                  <span className="sidebar-channel-badge">sys</span>
                </button>
              )
            })}
            {!channelsCollapsed && channels.map((ch) => {
              const target = `#${ch.name}`
              const isActive = selectedChannel === target
              const isMenuOpen = openChannelMenuId === ch.id
              const unreadCount = getConversationUnread(ch.id ?? null)
              const threadUnreadCount = getConversationThreadUnreadCount(ch.id ?? null)
              const showUnreadBadge = unreadCount > 0
              const showThreadDot = !showUnreadBadge && threadUnreadCount > 0
              return (
                <div
                  key={ch.id ?? ch.name}
                  className={`sidebar-channel-row${isActive ? ' active' : ''}`}
                  ref={isMenuOpen ? menuRef : undefined}
                >
                  <button
                    type="button"
                    className={`sidebar-item sidebar-channel-button${
                      ch.channel_type !== 'team' ? ' has-actions' : ''
                    }${isActive ? ' active' : ''}`}
                    onClick={() => selectChannel(ch)}
                    title={ch.description ?? ch.name}
                  >
                    <span className="sidebar-item-hash">#</span>
                    <span className="sidebar-item-main">
                      <span className="sidebar-item-text">{ch.name}</span>
                      {ch.description && <span className="sidebar-item-meta">{ch.description}</span>}
                      {ch.id && <span className="sidebar-item-meta sidebar-item-id">{ch.id}</span>}
                    </span>
                    {showUnreadBadge && (
                      <span className="sidebar-unread-badge">{unreadCount}</span>
                    )}
                    {showThreadDot && (
                      <span className="sidebar-thread-unread-dot" title={`${threadUnreadCount} unread in threads`} />
                    )}
                    {ch.channel_type === 'team' && (
                      <span className="sidebar-channel-badge team">team</span>
                    )}
                  </button>
                  {ch.channel_type !== 'team' && (
                    <div className="sidebar-channel-actions">
                      <button
                        type="button"
                        className="sidebar-channel-action"
                        aria-label={`Edit #${ch.name}`}
                        title={`Edit #${ch.name}`}
                        onClick={(event) => {
                          event.stopPropagation()
                          setOpenChannelMenuId(null)
                          setEditingChannel(ch)
                        }}
                      >
                        <Pencil size={12} />
                      </button>
                      <button
                        type="button"
                        className="sidebar-channel-action"
                        aria-label={`Open menu for #${ch.name}`}
                        title={`Open menu for #${ch.name}`}
                        onClick={(event) => {
                          event.stopPropagation()
                          setOpenChannelMenuId((current) => (current === ch.id ? null : ch.id ?? null))
                        }}
                      >
                        <Ellipsis size={12} />
                      </button>
                      {isMenuOpen && (
                        <div className="sidebar-channel-menu">
                          <button
                            type="button"
                            className="sidebar-channel-menu-item danger"
                            onClick={(event) => {
                              event.stopPropagation()
                              setOpenChannelMenuId(null)
                              setDeleteTarget(ch)
                            }}
                          >
                            <Trash2 size={12} />
                            <span>Delete Channel</span>
                          </button>
                        </div>
                      )}
                    </div>
                  )}
                </div>
              )
            })}
          </div>

          <div className="sidebar-section">
            <div className="sidebar-section-header" onClick={() => setAgentsCollapsed(!agentsCollapsed)}>
              <span className="sidebar-section-label">Agents</span>
              <button
                type="button"
                className="sidebar-add-btn"
                title="Create agent"
                onClick={(e) => {
                  e.stopPropagation()
                  setShowCreateAgent(true)
                }}
              >
                <Plus size={14} />
              </button>
            </div>
            {!agentsCollapsed && agents.map((agent) => {
              const unreadCount = getAgentUnread(agent.name)
              const conversationId = getAgentConversationId(agent.name)
              return (
                <button
                  key={agent.name}
                  type="button"
                  className={`sidebar-item${
                    selectedAgent?.name === agent.name ? ' active' : ''
                  }`}
                  onClick={() => setSelectedAgent(agent as AgentInfo)}
                >
                  <AgentAvatar name={agent.name} status={agent.status} activity={agent.activity} />
                  <span className="sidebar-item-main">
                    <span className="sidebar-item-text">{agent.display_name ?? agent.name}</span>
                    <span className="sidebar-item-meta">:: {agent.runtime ?? 'agent'}</span>
                    {conversationId && (
                      <span className="sidebar-item-meta sidebar-item-id">{conversationId}</span>
                    )}
                  </span>
                  {unreadCount > 0 && (
                    <span className="sidebar-unread-badge">{unreadCount}</span>
                  )}
                </button>
              )
            })}
          </div>

          <div className="sidebar-section">
            <div className="sidebar-section-header" onClick={() => setHumansCollapsed(!humansCollapsed)}>
              <span className="sidebar-section-label">Humans</span>
            </div>
            {!humansCollapsed && humans.map((h) => (
              <div key={h.name} className="sidebar-item">
                <div
                  className="agent-avatar"
                  style={{
                    background: agentColor(h.name),
                    borderRadius: 4,
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    fontSize: 12,
                    fontWeight: 700,
                    color: '#fff',
                  }}
                >
                  {h.name[0]?.toUpperCase()}
                </div>
                <span className="sidebar-item-main">
                  <span className="sidebar-item-text">{h.name}</span>
                  <span className="sidebar-item-meta">:: human</span>
                </span>
                {h.name === currentUser && <span className="you-badge">you</span>}
              </div>
            ))}
          </div>
        </div>

        <div className="sidebar-footer">
          <div
            className="sidebar-footer-avatar"
            style={{
              width: 32,
              height: 32,
              borderRadius: 6,
              background: agentColor(currentUser),
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              fontSize: 14,
              fontWeight: 700,
              color: '#fff',
              flexShrink: 0,
            }}
          >
            {currentUser[0]?.toUpperCase() ?? '?'}
          </div>
          <div className="sidebar-footer-main">
            <span className="sidebar-footer-name">{currentUser}</span>
            <span className="sidebar-footer-meta">[operator::active]</span>
          </div>
          <button className="sidebar-footer-cog" type="button" aria-label="Open settings">
            <Settings2 size={15} />
          </button>
        </div>
      </nav>

      <CreateAgentModal
        open={showCreateAgent}
        onOpenChange={setShowCreateAgent}
        onCreated={() => {
          setShowCreateAgent(false)
          void refreshAgents()
        }}
      />
      {showCreateChannel && (
        <CreateChannelModal
          defaultMode={createModalMode}
          open={showCreateChannel}
          onOpenChange={(open) => setShowCreateChannel(open)}
          onCreated={(created) => {
            setShowCreateChannel(false)
            setSelectedChannel(`#${created.name}`, created.id ?? null)
            void refreshChannels()
            void refreshTeams()
          }}
        />
      )}
      {editingChannel && (
        <EditChannelModal
          channel={editingChannel}
          open={!!editingChannel}
          onOpenChange={(open) => !open && setEditingChannel(null)}
          onSaved={(updated) => {
            if (selectedChannelId === updated.id) {
              setSelectedChannel(`#${updated.name}`, updated.id)
            }
            setEditingChannel(null)
            void refreshChannels()
          }}
        />
      )}
      {deleteTarget && (
        <DeleteChannelModal
          channel={deleteTarget}
          open={!!deleteTarget}
          onOpenChange={(open) => !open && setDeleteTarget(null)}
          onArchived={() => {
            recoverSelectionAfterChannelRemoval(deleteTarget.id)
            setDeleteTarget(null)
            void refreshChannels()
          }}
          onDeleted={() => {
            recoverSelectionAfterChannelRemoval(deleteTarget.id)
            setDeleteTarget(null)
            void refreshChannels()
          }}
        />
      )}
    </>
  )
}
