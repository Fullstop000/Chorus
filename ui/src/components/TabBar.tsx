import { useApp } from '../store'
import type { ActiveTab } from '../store'

const CHANNEL_TABS: { id: ActiveTab; label: string }[] = [
  { id: 'chat', label: 'Chat' },
  { id: 'tasks', label: 'Tasks' },
]

const AGENT_TABS: { id: ActiveTab; label: string }[] = [
  { id: 'chat', label: 'Chat' },
  { id: 'tasks', label: 'Tasks' },
  { id: 'workspace', label: 'Workspace' },
  { id: 'activity', label: 'Activity' },
  { id: 'profile', label: 'Profile' },
]

export function TabBar() {
  const { selectedAgent, activeTab, setActiveTab } = useApp()
  const tabs = selectedAgent ? AGENT_TABS : CHANNEL_TABS

  return (
    <div
      style={{
        display: 'flex',
        borderBottom: '2px solid var(--border)',
        background: 'var(--content-bg)',
        paddingLeft: 16,
        gap: 0,
        flexShrink: 0,
      }}
    >
      {tabs.map((tab) => (
        <button
          key={tab.id}
          onClick={() => setActiveTab(tab.id)}
          style={{
            padding: '10px 16px',
            fontSize: 12,
            fontWeight: 700,
            letterSpacing: '0.06em',
            textTransform: 'uppercase',
            borderBottom: activeTab === tab.id ? '2px solid var(--accent)' : '2px solid transparent',
            marginBottom: -2,
            color: activeTab === tab.id ? 'var(--accent)' : 'var(--text-muted)',
            background: 'none',
            cursor: 'pointer',
            transition: 'color 0.15s',
          }}
        >
          {tab.label}
        </button>
      ))}
    </div>
  )
}
