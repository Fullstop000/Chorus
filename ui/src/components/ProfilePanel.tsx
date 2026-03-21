import { useState } from 'react'
import { useApp } from '../store'
import { startAgent, stopAgent } from '../api'
import './ProfilePanel.css'

function agentColor(name: string): string {
  const colors = ['#FF6B6B','#4ECDC4','#45B7D1','#96CEB4','#FFEAA7','#DDA0DD','#98D8C8']
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff
  return colors[Math.abs(h) % colors.length]
}

function activityLabel(activity?: string, detail?: string): string {
  if (!activity || activity === 'offline') return 'Offline'
  if (detail) return detail
  return activity.charAt(0).toUpperCase() + activity.slice(1)
}

function activityDotColor(activity?: string): string {
  switch (activity) {
    case 'online': return 'var(--lime)'
    case 'thinking': return 'var(--orange)'
    case 'working': return 'var(--orange)'
    default: return 'var(--gray-400)'
  }
}

export function ProfilePanel() {
  const { selectedAgent, refreshServerInfo } = useApp()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  if (!selectedAgent) {
    return (
      <div className="profile-panel" style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', color: 'var(--text-muted)' }}>
        Select an agent to view profile.
      </div>
    )
  }

  const color = agentColor(selectedAgent.name)
  const initial = selectedAgent.name[0]?.toUpperCase() ?? '?'
  const isActive = selectedAgent.status === 'active'

  async function handleStart() {
    setBusy(true); setError(null)
    try {
      await startAgent(selectedAgent!.name)
      refreshServerInfo()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  async function handleStop() {
    setBusy(true); setError(null)
    try {
      await stopAgent(selectedAgent!.name)
      refreshServerInfo()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="profile-panel">
      <div className="profile-avatar-section">
        <div className="profile-avatar-large" style={{ background: color }}>
          {initial}
        </div>
        <div className="profile-name">{selectedAgent.display_name ?? selectedAgent.name}</div>
        <div className="profile-handle">@{selectedAgent.name}</div>
        <div className="profile-activity-status">
          <span className="profile-activity-dot" style={{ background: activityDotColor(selectedAgent.activity) }} />
          {activityLabel(selectedAgent.activity, selectedAgent.activity_detail)}
        </div>
      </div>

      {error && <div className="error-banner">{error}</div>}

      <div className="profile-controls">
        {isActive ? (
          <button className="btn-brutal btn-orange" onClick={handleStop} disabled={busy}>
            {busy ? '…' : '⏹ Stop'}
          </button>
        ) : (
          <button className="btn-brutal btn-lime" onClick={handleStart} disabled={busy}>
            {busy ? '…' : '▶ Start'}
          </button>
        )}
      </div>

      {selectedAgent.description && (
        <div className="profile-section">
          <div className="profile-section-label">Role</div>
          <div className="profile-role-text">{selectedAgent.description}</div>
        </div>
      )}

      <div className="profile-section">
        <div className="profile-section-label">Configuration</div>
        <div className="profile-config-grid">
          <span className="profile-config-key">Runtime</span>
          <span className="badge badge-claude">{selectedAgent.runtime ?? 'claude'}</span>
          <span className="profile-config-key">Model</span>
          <span className="badge">{selectedAgent.model ?? 'sonnet'}</span>
          <span className="profile-config-key">Status</span>
          <span
            className="badge"
            style={{
              background:
                selectedAgent.status === 'active'
                  ? 'var(--lime)'
                  : selectedAgent.status === 'sleeping'
                  ? 'var(--orange)'
                  : 'var(--gray-400)',
            }}
          >
            {selectedAgent.status}
          </span>
        </div>
      </div>
    </div>
  )
}
