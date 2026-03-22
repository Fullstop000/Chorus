import { useState } from 'react'
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
        {/* Header */}
        <div className="sidebar-header">
          <span className="sidebar-server-name">
            Chorus <button>▾</button>
          </span>
          <div style={{ display: 'flex', gap: 4 }}>
            <button className="sidebar-icon-btn">✦</button>
            <button className="sidebar-icon-btn">⊕</button>
          </div>
        </div>

        <div className="sidebar-body">
          {/* Channels */}
          <div className="sidebar-section">
            <div className="sidebar-section-header">
              <span className="sidebar-section-label">Channels</span>
              <button className="sidebar-add-btn" title="Add channel" onClick={() => setShowCreateChannel(true)}>+</button>
            </div>
            {channels.map((ch) => {
              const target = `#${ch.name}`
              return (
                <div
                  key={ch.name}
                  className={`sidebar-item${selectedChannel === target ? ' active' : ''}`}
                  onClick={() => setSelectedChannel(target)}
                >
                  <span className="sidebar-item-hash">#</span>
                  <span className="sidebar-item-text">{ch.name}</span>
                </div>
              )
            })}
          </div>

          {/* System Channels */}
          {systemChannels.length > 0 && (
            <div className="sidebar-section">
              <div className="sidebar-section-header">
                <span className="sidebar-section-label">System</span>
              </div>
              {systemChannels.map((ch) => {
                const target = `#${ch.name}`
                return (
                  <div
                    key={ch.name}
                    className={`sidebar-item sidebar-item--system${selectedChannel === target ? ' active' : ''}`}
                    onClick={() => setSelectedChannel(target)}
                    title={ch.description ?? ch.name}
                  >
                    <span className="sidebar-item-hash">#</span>
                    <span className="sidebar-item-text">{ch.name}</span>
                  </div>
                )
              })}
            </div>
          )}

          {/* Agents */}
          <div className="sidebar-section">
            <div className="sidebar-section-header">
              <span className="sidebar-section-label">Agents</span>
              <button
                className="sidebar-add-btn"
                title="Create agent"
                onClick={() => setShowCreateAgent(true)}
              >
                +
              </button>
            </div>
            {agents.map((agent) => (
              <div
                key={agent.name}
                className={`sidebar-item${
                  selectedAgent?.name === agent.name ? ' active' : ''
                }`}
                onClick={() => setSelectedAgent(agent as AgentInfo)}
              >
                <AgentAvatar name={agent.name} status={agent.status} activity={agent.activity} />
                <span className="sidebar-item-text">{agent.display_name ?? agent.name}</span>
              </div>
            ))}
          </div>

          {/* Humans */}
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
                <span className="sidebar-item-text">{h.name}</span>
                {h.name === currentUser && <span className="you-badge">you</span>}
              </div>
            ))}
          </div>
        </div>

        {/* Footer */}
        <div className="sidebar-footer">
          <div
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
          <span className="sidebar-footer-name">{currentUser}</span>
          <button className="sidebar-footer-cog">⚙</button>
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
