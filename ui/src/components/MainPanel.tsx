import { useApp, useTarget } from '../store'
import { TabBar } from './TabBar'
import { ChatPanel } from './ChatPanel'
import { TasksPanel } from './TasksPanel'
import { ProfilePanel } from './ProfilePanel'
import { ActivityPanel } from './ActivityPanel'
import { WorkspacePanel } from './WorkspacePanel'
import { MessageInput } from './MessageInput'
import { ThreadPanel } from './ThreadPanel'
import { useHistory } from '../hooks/useHistory'

export function MainPanel() {
  const { activeTab, currentUser, selectedChannel, selectedAgent, openThreadMsg } = useApp()
  const target = useTarget()
  const { refresh: refreshHistory } = useHistory(currentUser, target)

  const showHeader = selectedChannel || selectedAgent

  return (
    <div
      style={{
        flex: 1,
        display: 'flex',
        flexDirection: 'column',
        overflow: 'hidden',
        background: 'var(--content-bg)',
      }}
    >
      {showHeader && <TabBar />}

      <div style={{ flex: 1, display: 'flex', overflow: 'hidden' }}>
        <div style={{ flex: 1, display: 'flex', flexDirection: 'column', overflow: 'hidden' }}>
          {activeTab === 'chat' && (
            <>
              <ChatPanel />
              <MessageInput onMessageSent={refreshHistory} />
            </>
          )}
          {activeTab === 'tasks' && <TasksPanel />}
          {activeTab === 'profile' && <ProfilePanel />}
          {activeTab === 'activity' && selectedAgent && <ActivityPanel agentName={selectedAgent.name} />}
          {activeTab === 'workspace' && selectedAgent && <WorkspacePanel agentName={selectedAgent.name} />}
          {!showHeader && (
            <div
              style={{
                flex: 1,
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                color: 'var(--text-muted)',
                flexDirection: 'column',
                gap: 8,
              }}
            >
              <span style={{ fontSize: 32 }}>🎵</span>
              <span>Select a channel or agent to get started</span>
            </div>
          )}
        </div>

        {activeTab === 'chat' && openThreadMsg && <ThreadPanel />}
      </div>
    </div>
  )
}
