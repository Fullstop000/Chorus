import { useEffect, useState } from "react";
import { getChannelMembers, getTeam, sendMessage } from "../data";
import { useStore } from "../store";
import {
  useAgents,
  useHumans,
  useInbox,
  useRefresh,
  useTarget,
} from "../hooks/data";
import { useHistory } from "../hooks/useHistory";
import { TabBar } from "./TabBar";
import { ChatHeader, ChatPanel } from "../components/chat/ChatPanel";
import { TasksPanel } from "../components/tasks/TasksPanel";
import { ProfilePanel } from "../components/agents/profile/ProfilePanel";
import { ActivityPanel } from "../components/agents/activity/ActivityPanel";
import { WorkspacePanel } from "../components/agents/WorkspacePanel";
import { MessageInput } from "../components/chat/MessageInput";
import { ThreadPanel } from "../components/chat/ThreadPanel";
import { ThreadsTab } from "../components/chat/ThreadsTab";
import { ChannelMembersPanel } from "../components/channels/ChannelMembersPanel";
import type {
  ChannelMemberInfo,
  TeamResponse,
} from "../components/channels/types";
import { TeamSettings } from "../components/channels/TeamSettings";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

export function MainPanel() {
  const {
    currentUser,
    activeTab,
    currentChannel,
    currentAgent,
    openThreadMsg,
  } = useStore();
  const agents = useAgents();
  const humans = useHumans();
  const { getAgentConversationId, applyReadCursorAck } = useInbox();
  const { refreshChannels, refreshAgents, refreshTeams } = useRefresh();
  const chatTarget = useTarget();
  const activeConversationId =
    currentChannel?.id ??
    (currentAgent ? getAgentConversationId(currentAgent.name) : null);
  const chatHistory = useHistory(
    currentUser,
    activeTab === "chat" ? chatTarget : null,
    activeConversationId,
    { onReadCursorAck: applyReadCursorAck },
  );
  const [members, setMembers] = useState<ChannelMemberInfo[]>([]);
  const [membersLoading, setMembersLoading] = useState(false);
  const [showMembersPanel, setShowMembersPanel] = useState(false);
  const [showTeamSettings, setShowTeamSettings] = useState(false);
  const [teamDetails, setTeamDetails] = useState<TeamResponse | null>(null);
  const [teamSettingsLoading, setTeamSettingsLoading] = useState(false);

  const showHeader = currentChannel || currentAgent;
  const isTeamChannel = currentChannel?.channel_type === "team";
  const isSystemChannel = currentChannel?.channel_type === "system";
  const canInviteMembers = Boolean(
    currentChannel?.id && !isTeamChannel && !isSystemChannel,
  );
  const channelId = currentChannel?.id;

  useEffect(() => {
    setShowMembersPanel(false);
    setShowTeamSettings(false);
  }, [channelId]);

  useEffect(() => {
    if (!channelId) {
      setMembers([]);
      setShowMembersPanel(false);
      return;
    }

    let cancelled = false;
    setMembersLoading(true);
    getChannelMembers(channelId)
      .then((response) => {
        if (!cancelled) {
          setMembers(response.members);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setMembers([]);
        }
      })
      .finally(() => {
        if (!cancelled) {
          setMembersLoading(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [channelId]);

  useEffect(() => {
    if (!currentChannel || activeTab !== "chat") {
      setShowMembersPanel(false);
    }
  }, [activeTab, currentChannel]);

  useEffect(() => {
    setShowTeamSettings(false);
    setTeamDetails(null);
  }, [currentChannel]);

  async function openTeamSettings() {
    if (!currentChannel || !isTeamChannel) return;
    setTeamSettingsLoading(true);
    setShowTeamSettings(true);
    try {
      setTeamDetails(await getTeam(currentChannel.name));
    } catch (error) {
      console.error("Failed to load team settings", error);
      setShowTeamSettings(false);
    } finally {
      setTeamSettingsLoading(false);
    }
  }

  async function refreshSelectedTeam() {
    if (!currentChannel || !isTeamChannel) return;
    setTeamDetails(await getTeam(currentChannel.name));
  }

  async function refreshCurrentChannelMembers() {
    if (!channelId) return;
    setMembersLoading(true);
    try {
      const response = await getChannelMembers(channelId);
      setMembers(response.members);
    } catch (err) {
      console.error("Failed to refresh channel members", err);
    } finally {
      setMembersLoading(false);
    }
  }

  async function handleRetryChatMessage(
    message: (typeof chatHistory.messages)[number],
  ) {
    if (!chatTarget || !currentUser || !activeConversationId) return;
    const retryHandle = chatHistory.retryOptimisticMessage(message.id);
    if (!retryHandle) return;
    try {
      const sendAck = await sendMessage(
        activeConversationId,
        message.content,
        message.attachments?.map((attachment) => attachment.id) ?? [],
        { clientNonce: retryHandle.clientNonce },
      );
      chatHistory.ackOptimisticMessage(retryHandle, {
        messageId: sendAck.messageId,
        seq: sendAck.seq,
        createdAt: sendAck.createdAt,
        clientNonce: sendAck.clientNonce,
      });
    } catch (retryError) {
      const retryMessage =
        retryError instanceof Error ? retryError.message : String(retryError);
      chatHistory.failOptimisticMessage(retryHandle, retryMessage);
    }
  }

  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        background: "transparent",
        paddingTop: 10,
      }}
    >
      {showHeader && (
        <>
          <ChatHeader
            memberCount={
              currentChannel
                ? membersLoading
                  ? members.length || null
                  : members.length
                : null
            }
            isTeamChannel={isTeamChannel}
            membersOpen={showMembersPanel}
            onOpenTeamSettings={isTeamChannel ? openTeamSettings : undefined}
            onToggleMembers={() => setShowMembersPanel((current) => !current)}
          />
          <TabBar />
        </>
      )}

      <div style={{ flex: 1, display: "flex", overflow: "hidden" }}>
        <div
          style={{
            flex: 1,
            display: "flex",
            flexDirection: "column",
            overflow: "hidden",
            position: "relative",
          }}
        >
          {activeTab === "chat" && (
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
          {activeTab === "threads" && <ThreadsTab />}
          {activeTab === "tasks" && <TasksPanel />}
          {activeTab === "profile" && <ProfilePanel />}
          {activeTab === "activity" && currentAgent && (
            <ActivityPanel agentName={currentAgent.name} />
          )}
          {activeTab === "workspace" && currentAgent && (
            <WorkspacePanel agentName={currentAgent.name} />
          )}
          {!showHeader && (
            <div
              style={{
                flex: 1,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                color: "var(--color-muted-foreground)",
                flexDirection: "column",
                gap: 8,
              }}
            >
              <span className="empty-state-icon">[chorus::idle]</span>
              <span>Select a channel or agent to get started</span>
            </div>
          )}

          {activeTab === "chat" &&
            currentChannel &&
            channelId &&
            showMembersPanel && (
              <ChannelMembersPanel
                channelId={channelId}
                channelName={currentChannel.name}
                currentUser={currentUser}
                members={members}
                agents={agents}
                humans={humans}
                invitable={canInviteMembers}
                onClose={() => setShowMembersPanel(false)}
                onMembersChange={setMembers}
              />
            )}
        </div>
        {activeTab === "chat" && openThreadMsg && <ThreadPanel />}
      </div>
      {teamDetails && (
        <TeamSettings
          team={teamDetails.team}
          members={teamDetails.members}
          open={showTeamSettings}
          onOpenChange={setShowTeamSettings}
          onRefresh={async () => {
            await Promise.all([
              refreshChannels(),
              refreshTeams(),
              refreshAgents(),
            ]);
            await refreshSelectedTeam();
            await refreshCurrentChannelMembers();
          }}
          onDeleted={async () => {
            await Promise.all([refreshChannels(), refreshTeams()]);
          }}
        />
      )}
      {teamSettingsLoading && (
        <Dialog open={true}>
          <DialogContent>
            <DialogHeader>
              <DialogTitle>Loading Team</DialogTitle>
            </DialogHeader>
          </DialogContent>
        </Dialog>
      )}
    </div>
  );
}
