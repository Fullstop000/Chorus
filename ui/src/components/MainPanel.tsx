import { useEffect, useState } from 'react'
import { getChannelMembers } from '../api'
import { useApp, useTarget } from '../store'
import { TabBar } from './TabBar'
import { ChatHeader, ChatPanel } from './ChatPanel'
import { TasksPanel } from './TasksPanel'
import { ProfilePanel } from './ProfilePanel'
import { ActivityPanel } from './ActivityPanel'
import { WorkspacePanel } from './WorkspacePanel'
import { MessageInput } from './MessageInput'
import { ThreadPanel } from './ThreadPanel'
import { useHistory } from '../hooks/useHistory'
import { ChannelMembersPanel } from './ChannelMembersPanel'
import type { ChannelMemberInfo } from '../types'

export function MainPanel() {
  const {
    activeTab,
    currentUser,
    selectedChannel,
    selectedChannelId,
    selectedAgent,
    openThreadMsg,
    serverInfo,
  } = useApp()
  const target = useTarget()
  const { refresh: refreshHistory } = useHistory(currentUser, target)
  const [members, setMembers] = useState<ChannelMemberInfo[]>([])
  const [membersLoading, setMembersLoading] = useState(false)
  const [showMembersPanel, setShowMembersPanel] = useState(false)

  const showHeader = selectedChannel || selectedAgent
  const selectedUserChannel = selectedChannel
    ? serverInfo?.channels.find((channel) => `#${channel.name}` === selectedChannel) ?? null
    : null
  const selectedSystemChannel = selectedChannel
    ? serverInfo?.system_channels.find((channel) => `#${channel.name}` === selectedChannel) ?? null
    : null
  const canInviteMembers = Boolean(selectedUserChannel?.id)
  const optimisticMemberCount =
    selectedChannel && !selectedUserChannel && !selectedSystemChannel ? 1 : null

  useEffect(() => {
    if (!selectedChannelId) {
      setMembers([])
      setShowMembersPanel(false)
      return
    }

    let cancelled = false
    setMembersLoading(true)
    getChannelMembers(selectedChannelId)
      .then((response) => {
        if (!cancelled) {
          setMembers(response.members)
        }
      })
      .catch(() => {
        if (!cancelled) {
          setMembers([])
        }
      })
      .finally(() => {
        if (!cancelled) {
          setMembersLoading(false)
        }
      })

    return () => {
      cancelled = true
    }
  }, [selectedChannelId])

  useEffect(() => {
    if (!selectedChannel || activeTab !== 'chat') {
      setShowMembersPanel(false)
    }
  }, [activeTab, selectedChannel])

  return (
    <div
      style={{
        flex: 1,
        display: 'flex',
        flexDirection: 'column',
        overflow: 'hidden',
        background: 'var(--content-bg)',
        paddingTop: 10,
      }}
    >
      {showHeader && (
        <>
          <ChatHeader
            memberCount={
              selectedChannel
                ? membersLoading
                  ? optimisticMemberCount
                  : members.length
                : null
            }
            membersOpen={showMembersPanel}
            onToggleMembers={() => setShowMembersPanel((current) => !current)}
          />
          <TabBar />
        </>
      )}

      <div style={{ flex: 1, display: 'flex', overflow: 'hidden' }}>
        <div style={{ flex: 1, display: 'flex', flexDirection: 'column', overflow: 'hidden', position: 'relative' }}>
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
              <span className="empty-state-icon">[chorus::idle]</span>
              <span>Select a channel or agent to get started</span>
            </div>
          )}

          {activeTab === 'chat' && selectedChannel && selectedChannelId && showMembersPanel && (
            <ChannelMembersPanel
              channelId={selectedChannelId}
              channelName={(selectedUserChannel ?? selectedSystemChannel)?.name ?? selectedChannel.replace(/^#/, '')}
              currentUser={currentUser}
              members={members}
              agents={serverInfo?.agents ?? []}
              humans={serverInfo?.humans ?? []}
              invitable={canInviteMembers}
              onClose={() => setShowMembersPanel(false)}
              onMembersChange={setMembers}
            />
          )}
        </div>
        {activeTab === 'chat' && openThreadMsg && <ThreadPanel />}
      </div>
    </div>
  )
}
