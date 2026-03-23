import { useState } from 'react'
import { ChevronDown, Plus, Settings2, Sparkles } from 'lucide-react'
import { useApp } from '../store'
import type { AgentInfo } from '../types'
import { CreateAgentModal } from './CreateAgentModal'
import { CreateChannelModal } from './CreateChannelModal'
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
    selectedChannel,
    selectedAgent,
    setSelectedChannel,
    setSelectedAgent,
    refreshServerInfo,
  } = useApp()
  const [showCreateAgent, setShowCreateAgent] = useState(false)
  const [showCreateChannel, setShowCreateChannel] = useState(false)

  const channels = serverInfo?.channels.filter((c) => c.joined) ?? []
  const systemChannels = serverInfo?.system_channels ?? []
  const agents = serverInfo?.agents ?? []
  const humans = serverInfo?.humans ?? []

  return (
    <>
      <nav className="sidebar">
        <div className="sidebar-header">
          <div className="sidebar-server-block">
            <span className="sidebar-server-label">[chorus::workspace]</span>
            <span className="sidebar-server-name">
              Chorus
              <button type="button" aria-label="Open workspace menu">
                <ChevronDown size={14} />
              </button>
            </span>
          </div>
          <div className="sidebar-header-actions">
            <button className="sidebar-icon-btn" type="button" aria-label="Open suggestions">
              <Sparkles size={15} />
            </button>
            <button className="sidebar-icon-btn" type="button" aria-label="Create">
              <Plus size={15} />
            </button>
          </div>
        </div>

        <div className="sidebar-body">
          <div className="sidebar-section">
            <div className="sidebar-section-header">
              <span className="sidebar-section-label">Channels</span>
              <button className="sidebar-add-btn" type="button" title="Add channel" onClick={() => setShowCreateChannel(true)}>
                <Plus size={14} />
              </button>
            </div>
            {channels.map((ch) => {
              const target = `#${ch.name}`
              return (
                <button
                  key={ch.name}
                  type="button"
                  className={`sidebar-item${selectedChannel === target ? ' active' : ''}`}
                  onClick={() => setSelectedChannel(target)}
                >
                  <span className="sidebar-item-hash">#</span>
                  <span className="sidebar-item-main">
                    <span className="sidebar-item-text">{ch.name}</span>
                  </span>
                </button>
              )
            })}
          </div>

          {systemChannels.length > 0 && (
            <div className="sidebar-section">
              <div className="sidebar-section-header">
                <span className="sidebar-section-label">System Channels</span>
              </div>
              {systemChannels.map((ch) => {
                const target = `#${ch.name}`
                return (
                  <button
                    key={ch.name}
                    type="button"
                    className={`sidebar-item sidebar-item--system${selectedChannel === target ? ' active' : ''}`}
                    onClick={() => setSelectedChannel(target)}
                    title={ch.description ?? ch.name}
                  >
                    <span className="sidebar-item-hash">#</span>
                    <span className="sidebar-item-main">
                      <span className="sidebar-item-text">{ch.name}</span>
                      <span className="sidebar-item-meta">:: system</span>
                    </span>
                  </button>
                )
              })}
            </div>
          )}

          <div className="sidebar-section">
            <div className="sidebar-section-header">
              <span className="sidebar-section-label">Agents</span>
              <button
                className="sidebar-add-btn"
                type="button"
                title="Create agent"
                onClick={() => setShowCreateAgent(true)}
              >
                <Plus size={14} />
              </button>
            </div>
            {agents.map((agent) => (
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
                </span>
              </button>
            ))}
          </div>

          <div className="sidebar-section">
            <div className="sidebar-section-header">
              <span className="sidebar-section-label">Humans</span>
            </div>
            {humans.map((h) => (
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

      {showCreateAgent && (
        <CreateAgentModal
          onClose={() => setShowCreateAgent(false)}
          onCreated={() => {
            setShowCreateAgent(false)
            refreshServerInfo()
          }}
        />
      )}
      {showCreateChannel && (
        <CreateChannelModal
          onClose={() => setShowCreateChannel(false)}
          onCreated={() => {
            setShowCreateChannel(false)
            refreshServerInfo()
          }}
        />
      )}
    </>
  )
}
