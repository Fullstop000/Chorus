import { useApp } from '../store'
import './ProfilePanel.css'

function agentColor(name: string): string {
  const colors = ['#FF6B6B','#4ECDC4','#45B7D1','#96CEB4','#FFEAA7','#DDA0DD','#98D8C8']
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff
  return colors[Math.abs(h) % colors.length]
}

export function ProfilePanel() {
  const { selectedAgent } = useApp()

  if (!selectedAgent) {
    return (
      <div className="profile-panel" style={{ display: 'flex', alignItems: 'center', justifyContent: 'center', color: 'var(--text-muted)' }}>
        Select an agent to view profile.
      </div>
    )
  }

  const color = agentColor(selectedAgent.name)
  const initial = selectedAgent.name[0]?.toUpperCase() ?? '?'

  return (
    <div className="profile-panel">
      <div className="profile-avatar-section">
        <div className="profile-avatar-large" style={{ background: color }}>
          {initial}
        </div>
        <div className="profile-name">{selectedAgent.display_name ?? selectedAgent.name}</div>
        <div className="profile-handle">@{selectedAgent.name}</div>
      </div>

      {selectedAgent.description && (
        <div className="profile-section">
          <div className="profile-section-label">
            Role <button title="Edit role">✎</button>
          </div>
          <div className="profile-role-text">{selectedAgent.description}</div>
        </div>
      )}

      <div className="profile-section">
        <div className="profile-section-label">Configuration</div>
        <div className="profile-config-grid">
          <span className="profile-config-key">Runtime</span>
          <span>
            <span className="badge badge-claude">
              {selectedAgent.runtime ?? 'Claude Code'}
            </span>
          </span>
          <span className="profile-config-key">Model</span>
          <span>
            <span className="badge badge-sonnet">
              {selectedAgent.model ?? 'Sonnet'}
            </span>
          </span>
          <span className="profile-config-key">Status</span>
          <span>
            <span
              className="badge"
              style={{
                background:
                  selectedAgent.status === 'active'
                    ? 'var(--status-online)'
                    : selectedAgent.status === 'sleeping'
                    ? 'var(--status-sleeping)'
                    : 'var(--status-inactive)',
              }}
            >
              {selectedAgent.status}
            </span>
          </span>
        </div>
      </div>
    </div>
  )
}
