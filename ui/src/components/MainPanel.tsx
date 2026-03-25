import { useEffect, useState } from 'react'
import { getChannelMembers, getTeam } from '../api'
import { useApp, useTarget } from '../store'
import { mergeUserAndTeamChannels } from '../channelList'
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
import type { ChannelMemberInfo, TeamResponse } from '../types'
import { TeamSettings } from './TeamSettings'

export function MainPanel() {
  const {
    activeTab,
    currentUser,
    refreshServerInfo,
    refreshTeams,
    channels,
    agents,
    selectedChannel,
    selectedChannelId,
    selectedAgent,
    openThreadMsg,
    serverInfo,
    teams,
  } = useApp()
  const target = useTarget()
  const { refresh: refreshHistory } = useHistory(currentUser, target)
  const [members, setMembers] = useState<ChannelMemberInfo[]>([])
  const [membersLoading, setMembersLoading] = useState(false)
  const [showMembersPanel, setShowMembersPanel] = useState(false)
  const [showTeamSettings, setShowTeamSettings] = useState(false)
  const [teamDetails, setTeamDetails] = useState<TeamResponse | null>(null)
  const [teamSettingsLoading, setTeamSettingsLoading] = useState(false)

  const userChannels = mergeUserAndTeamChannels(channels, teams)
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
            await Promise.all([refreshServerInfo(), refreshTeams()])
            await refreshSelectedTeam()
          }}
          onDeleted={async () => {
            await Promise.all([refreshServerInfo(), refreshTeams()])
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
