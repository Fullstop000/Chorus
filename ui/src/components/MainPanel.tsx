import { useEffect, useState } from 'react'
import { getChannelMembers, getTeam, sendMessage } from '../api'
import { useApp, useTarget } from '../store'
import { useHistory } from '../hooks/useHistory'
import { TabBar } from './TabBar'
import { ChatHeader, ChatPanel } from './ChatPanel'
import { TasksPanel } from './TasksPanel'
import { ProfilePanel } from './ProfilePanel'
import { ActivityPanel } from './ActivityPanel'
import { WorkspacePanel } from './WorkspacePanel'
import { MessageInput } from './MessageInput'
import { ThreadPanel } from './ThreadPanel'
import { ThreadsTab } from './ThreadsTab'
import { ChannelMembersPanel } from './ChannelMembersPanel'
import type { ChannelMemberInfo, TeamResponse } from '../types'
import { TeamSettings } from './TeamSettings'

export function MainPanel() {
  const {
    activeTab,
    currentUser,
    getAgentConversationId,
    markConversationRead,
    refreshChannels,
    refreshAgents,
    refreshTeams,
    channels,
    agents,
    selectedChannel,
    selectedChannelId,
    selectedAgent,
    openThreadMsg,
    serverInfo,
  } = useApp()
  const chatTarget = useTarget()
  const activeConversationId =
    selectedChannelId ?? (selectedAgent ? getAgentConversationId(selectedAgent.name) : null)
  const chatHistory = useHistory(
    currentUser,
    activeTab === 'chat' ? chatTarget : null,
    activeConversationId
  )
  const [members, setMembers] = useState<ChannelMemberInfo[]>([])
  const [membersLoading, setMembersLoading] = useState(false)
  const [showMembersPanel, setShowMembersPanel] = useState(false)
  const [showTeamSettings, setShowTeamSettings] = useState(false)
  const [teamDetails, setTeamDetails] = useState<TeamResponse | null>(null)
  const [teamSettingsLoading, setTeamSettingsLoading] = useState(false)

  const userChannels = channels
  const showHeader = selectedChannel || selectedAgent
  const selectedUserChannel = selectedChannel
    ? userChannels.find((channel) => `#${channel.name}` === selectedChannel) ?? null
    : null
  const selectedSystemChannel = selectedChannel
    ? serverInfo?.system_channels.find((channel) => `#${channel.name}` === selectedChannel) ?? null
    : null
  const selectedTeamChannel = selectedUserChannel?.channel_type === 'team' ? selectedUserChannel : null
  const canInviteMembers = Boolean(selectedUserChannel?.id && selectedUserChannel.channel_type !== 'team')
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

  useEffect(() => {
    if (activeTab !== 'chat' || !activeConversationId || chatHistory.lastReadSeq <= 0) {
      return
    }
    markConversationRead(activeConversationId, chatHistory.lastReadSeq)
  }, [activeConversationId, activeTab, chatHistory.lastReadSeq, markConversationRead])

  useEffect(() => {
    setShowTeamSettings(false)
    setTeamDetails(null)
  }, [selectedChannel])

  async function openTeamSettings() {
    if (!selectedTeamChannel) return
    setTeamSettingsLoading(true)
    setShowTeamSettings(true)
    try {
      setTeamDetails(await getTeam(selectedTeamChannel.name))
    } catch (error) {
      console.error('Failed to load team settings', error)
      setShowTeamSettings(false)
    } finally {
      setTeamSettingsLoading(false)
    }
  }

  async function refreshSelectedTeam() {
    if (!selectedTeamChannel) return
    setTeamDetails(await getTeam(selectedTeamChannel.name))
  }

  async function refreshCurrentChannelMembers() {
    if (!selectedChannelId) return
    setMembersLoading(true)
    try {
      const response = await getChannelMembers(selectedChannelId)
      setMembers(response.members)
    } catch (err) {
      console.error('Failed to refresh channel members', err)
    } finally {
      setMembersLoading(false)
    }
  }

  async function handleRetryChatMessage(message: typeof chatHistory.messages[number]) {
    if (!chatTarget || !currentUser || !activeConversationId) return
    const retryHandle = chatHistory.retryOptimisticMessage(message.id)
    if (!retryHandle) return
    try {
      const sendAck = await sendMessage(
        activeConversationId,
        message.content,
        message.attachments?.map((attachment) => attachment.id) ?? [],
        { clientNonce: retryHandle.clientNonce }
      )
      chatHistory.ackOptimisticMessage(retryHandle, {
        messageId: sendAck.messageId,
        seq: sendAck.seq,
        createdAt: sendAck.createdAt,
        clientNonce: sendAck.clientNonce,
      })
    } catch (retryError) {
      const retryMessage = retryError instanceof Error ? retryError.message : String(retryError)
      chatHistory.failOptimisticMessage(retryHandle, retryMessage)
    }
  }

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
            isTeamChannel={Boolean(selectedTeamChannel)}
            membersOpen={showMembersPanel}
            onOpenTeamSettings={selectedTeamChannel ? openTeamSettings : undefined}
            onToggleMembers={() => setShowMembersPanel((current) => !current)}
          />
          <TabBar />
        </>
      )}

      <div style={{ flex: 1, display: 'flex', overflow: 'hidden' }}>
        <div style={{ flex: 1, display: 'flex', flexDirection: 'column', overflow: 'hidden', position: 'relative' }}>
          {activeTab === 'chat' && (
            <>
              <ChatPanel
                target={chatTarget}
                messages={chatHistory.messages}
                loading={chatHistory.loading}
                lastReadSeq={chatHistory.lastReadSeq}
                loadedTarget={chatHistory.loadedTarget}
                reportVisibleSeq={chatHistory.reportVisibleSeq}
                onRetryMessage={handleRetryChatMessage}
              />
              <MessageInput
                target={chatTarget}
                conversationId={activeConversationId}
                history={chatHistory}
              />
            </>
          )}
          {activeTab === 'threads' && <ThreadsTab />}
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
              agents={agents}
              humans={serverInfo?.humans ?? []}
              invitable={canInviteMembers}
              onClose={() => setShowMembersPanel(false)}
              onMembersChange={setMembers}
            />
          )}
        </div>
        {activeTab === 'chat' && openThreadMsg && <ThreadPanel />}
      </div>
      {showTeamSettings && teamDetails && (
        <TeamSettings
          team={teamDetails.team}
          members={teamDetails.members}
          onClose={() => setShowTeamSettings(false)}
          onRefresh={async () => {
            await Promise.all([refreshChannels(), refreshTeams(), refreshAgents()])
            await refreshSelectedTeam()
            await refreshCurrentChannelMembers()
          }}
          onDeleted={async () => {
            await Promise.all([refreshChannels(), refreshTeams()])
          }}
        />
      )}
      {showTeamSettings && teamSettingsLoading && (
        <div className="modal-overlay">
          <div className="modal-card">
            <div className="modal-header">
              <span className="modal-title">Loading Team</span>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
