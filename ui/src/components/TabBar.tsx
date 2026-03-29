import { useApp } from '../store'
import type { ActiveTab } from '../store'
import './TabBar.css'

const AGENT_TABS: { id: ActiveTab; label: string }[] = [
  { id: 'chat', label: 'Chat' },
  { id: 'tasks', label: 'Tasks' },
  { id: 'workspace', label: 'Workspace' },
  { id: 'activity', label: 'Activity' },
  { id: 'profile', label: 'Profile' },
]

export function TabBar() {
  const {
    selectedAgent,
    selectedChannelId,
    activeTab,
    getConversationThreadUnread,
    setActiveTab,
  } = useApp()
  const threadUnread = getConversationThreadUnread(selectedChannelId)
  const channelTabs: { id: ActiveTab; label: string }[] = [
    { id: 'chat', label: 'Chat' },
    { id: 'threads', label: threadUnread > 0 ? `Threads (${threadUnread})` : 'Threads' },
    { id: 'tasks', label: 'Tasks' },
  ]
  const tabs = selectedAgent ? AGENT_TABS : channelTabs

  return (
    <div className="tab-bar">
      {tabs.map((tab) => (
        <button
          key={tab.id}
          onClick={() => setActiveTab(tab.id)}
          className={`tab-bar__item${activeTab === tab.id ? ' is-active' : ''}`}
        >
          {tab.label}
        </button>
      ))}
    </div>
  )
}
