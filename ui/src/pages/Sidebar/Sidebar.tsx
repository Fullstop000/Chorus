import { useEffect, useRef, useState } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { Ellipsis, Inbox, Pencil, Plus, Settings2, Trash2, Users } from 'lucide-react'
import { useStore } from '../../store'
import { useAgents, useChannels, useHumans, useInbox, useRefresh, useWorkspaces } from '../../hooks/data'
import { useCurrentAgent, useCurrentChannel } from '../../hooks/useRouteSubject'
import type { ChannelInfo } from '../../components/channels/types'
import { isVisibleSidebarChannel } from './sidebarChannels'
import { CreateAgentModal } from '../../components/agents/CreateAgentModal'
import { CreateChannelModal } from '../../components/channels/CreateChannelModal'
import { DeleteChannelModal, EditChannelModal } from '../../components/channels/EditChannelModal'
import { channelPath, dmPath, inboxPath, rootPath, settingsPath } from '../../lib/routes'
import './Sidebar.css'

function agentColor(name: string): string {
  const colors = ['#FF6B6B','#4ECDC4','#45B7D1','#96CEB4','#FFEAA7','#DDA0DD','#98D8C8']
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff
  return colors[Math.abs(h) % colors.length]
}

function agentDotClass(status: string, activity?: string): string {
  if (status === 'working') {
    if (activity === 'thinking' || activity === 'working') return activity
    return 'working'
  }
  if (status === 'ready') {
    if (activity === 'thinking' || activity === 'working') return activity
    return 'online'
  }
  if (status === 'failed') return 'failed'
  return 'offline'
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
  const { currentUser, currentUserId } = useStore()
  const currentChannel = useCurrentChannel()
  const currentAgent = useCurrentAgent()
  const navigate = useNavigate()
  const location = useLocation()
  const showConversationIds = useStore((s) => s.showConversationIds)
  const agents = useAgents()
  const { channels: loadedChannels, systemChannels } = useChannels()
  const humans = useHumans()
  const { workspaces } = useWorkspaces()
  const { getConversationUnread, getAgentUnread, getAgentConversationId } = useInbox()
  const { refreshChannels, refreshAgents, refreshTeams } = useRefresh()
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
  const currentHumanInfo = humans.find((h) => h.id === currentUserId)
  const activeWorkspace = workspaces.find((workspace) => workspace.active) ?? null

  useEffect(() => {
    function handlePointerDown(event: MouseEvent) {
      if (!menuRef.current?.contains(event.target as Node)) {
        setOpenChannelMenuId(null)
      }
    }
    document.addEventListener('mousedown', handlePointerDown)
    return () => document.removeEventListener('mousedown', handlePointerDown)
  }, [])

  function recoverSelectionAfterChannelRemoval(channelId?: string) {
    if (!channelId || currentChannel?.id !== channelId) return
    const fallback = channels.find((channel) => channel.id !== channelId)
    navigate(fallback ? channelPath(fallback.name) : rootPath(), { replace: true })
  }

  const showSettings = location.pathname.startsWith('/settings')
  const showDecisions = location.pathname === inboxPath()

  function toggleOverlay(target: string) {
    if (location.pathname === target || location.pathname.startsWith(target + '/')) {
      // We're already on the overlay; close it. If `from` was set when
      // we navigated in, pop that entry (don't push); otherwise replace
      // with root so browser back doesn't return to the overlay.
      const from = (location.state as { from?: string } | null)?.from
      if (from) {
        navigate(-1)
      } else {
        navigate(rootPath(), { replace: true })
      }
    } else {
      navigate(target, { state: { from: location.pathname } })
    }
  }

  return (
    <>
      <nav className="sidebar">
        <div className="sidebar-header">
          <div className="sidebar-server-block">
            <span className="sidebar-server-label">[chorus::workspace]</span>
            <span className="sidebar-server-name">Chorus</span>
            {activeWorkspace && (
              <span className="sidebar-workspace-ref" title={`Current workspace: ${activeWorkspace.name}`}>
                current: {activeWorkspace.name}
              </span>
            )}
          </div>
        </div>

        <div className="sidebar-body">
          <div className="sidebar-section">
            <div
              className="sidebar-section-header"
              role="button"
              tabIndex={0}
              aria-expanded={!channelsCollapsed}
              onClick={() => setChannelsCollapsed((v) => !v)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault()
                  setChannelsCollapsed((v) => !v)
                }
              }}
            >
              <span className="sidebar-section-label">Channels</span>
              <div style={{ display: 'flex', gap: 4 }}>
                <button
                  className="sidebar-add-btn"
                  type="button"
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
                  className="sidebar-add-btn"
                  type="button"
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
              const unreadCount = getConversationUnread(ch.id ?? null)
              const showUnreadBadge = unreadCount > 0
              return (
                <button
                  key={ch.id ?? ch.name}
                  type="button"
                  className={`sidebar-item${currentChannel?.name === ch.name ? ' active' : ''}`}
                  onClick={() => navigate(channelPath(ch.name))}
                  title={ch.description ?? ch.name}
                >
                  <span className="sidebar-item-hash">#</span>
                  <span className="sidebar-item-main">
                    <span className="sidebar-item-text">{ch.name}</span>
                    {showConversationIds && ch.id && <span className="sidebar-item-meta sidebar-item-id">{ch.id}</span>}
                  </span>
                  {showUnreadBadge && (
                    <span className="sidebar-unread-badge">{unreadCount}</span>
                  )}
                  <span className="sidebar-channel-badge">sys</span>
                </button>
              )
            })}
            {!channelsCollapsed && channels.map((ch) => {
              const isActive = currentChannel?.name === ch.name
              const isMenuOpen = openChannelMenuId === ch.id
              const unreadCount = getConversationUnread(ch.id ?? null)
              const showUnreadBadge = unreadCount > 0
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
                    onClick={() => navigate(channelPath(ch.name))}
                    title={ch.description ?? ch.name}
                  >
                    <span className="sidebar-item-hash">#</span>
                    <span className="sidebar-item-main">
                      <span className="sidebar-item-text">{ch.name}</span>
                      {ch.description && <span className="sidebar-item-meta">{ch.description}</span>}
                      {showConversationIds && ch.id && <span className="sidebar-item-meta sidebar-item-id">{ch.id}</span>}
                    </span>
                    {showUnreadBadge && (
                      <span className="sidebar-unread-badge">{unreadCount}</span>
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
            <div
              className="sidebar-section-header"
              role="button"
              tabIndex={0}
              aria-expanded={!agentsCollapsed}
              onClick={() => setAgentsCollapsed((v) => !v)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault()
                  setAgentsCollapsed((v) => !v)
                }
              }}
            >
              <span className="sidebar-section-label">Agents</span>
              <button
                className="sidebar-add-btn"
                type="button"
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
                    currentAgent?.name === agent.name ? ' active' : ''
                  }`}
                  onClick={() => navigate(dmPath(agent.name))}
                >
                  <AgentAvatar name={agent.name} status={agent.status} activity={agent.activity} />
                  <span className="sidebar-item-main">
                    <span className="sidebar-item-text">{agent.display_name ?? agent.name}</span>
                    <span className="sidebar-item-meta">:: {agent.runtime ?? 'agent'}</span>
                    {showConversationIds && conversationId && (
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
            <div
              className="sidebar-section-header"
              role="button"
              tabIndex={0}
              aria-expanded={!humansCollapsed}
              onClick={() => setHumansCollapsed((v) => !v)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault()
                  setHumansCollapsed((v) => !v)
                }
              }}
            >
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
                {h.id === currentUserId && <span className="you-badge">you</span>}
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
              background: agentColor(currentHumanInfo?.name ?? currentUser),
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
            <span className="sidebar-footer-name">{currentHumanInfo?.name ?? currentUser}</span>
            <span className="sidebar-footer-meta">[operator::active]</span>
          </div>
          <button
            className="sidebar-footer-cog"
            type="button"
            aria-label={showDecisions ? 'Close decision inbox' : 'Open decision inbox'}
            aria-pressed={showDecisions}
            onClick={() => toggleOverlay(inboxPath())}
          >
            <Inbox size={15} />
          </button>
          <button
            className="sidebar-footer-cog"
            type="button"
            aria-label={showSettings ? 'Close settings' : 'Open settings'}
            aria-pressed={showSettings}
            onClick={() => toggleOverlay(settingsPath())}
          >
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
            navigate(channelPath(created.name))
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
            if (currentChannel?.id === updated.id) {
              navigate(channelPath(updated.name), { replace: true })
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
