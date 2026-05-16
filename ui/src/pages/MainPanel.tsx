import { useEffect, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { getChannelMembers, getTeam } from "../data";
import { useStore } from "../store";
import {
  useAgents,
  useHumans,
  useInbox,
  useRefresh,
  useTeams,
} from "../hooks/data";
import { useTarget } from "../hooks/useTarget";
import {
  useCurrentAgent,
  useCurrentChannel,
  useCurrentTaskDetail,
  useRouteSubject,
} from "../hooks/useRouteSubject";
import { inboxPath, rootPath, settingsPath } from "../lib/routes";
import type { ActiveTab } from "../store/uiStore";
import { useHistory } from "../hooks/useHistory";
import { useTraceSubscription } from "../hooks/useTraceSubscription";
import { TabBar } from "./TabBar";
import { EmptyShell } from "./EmptyShell";
import { ChatHeader, ChatPanel } from "../components/chat/ChatPanel";
import { TasksPanel } from "../components/tasks/TasksPanel";
import { TaskDetail } from "../components/tasks/TaskDetail";
import { ProfilePanel } from "../components/agents/profile/ProfilePanel";
import { TelescopeActivity } from "../components/agents/activity/TelescopeActivity";
import { WorkspacePanel } from "../components/agents/WorkspacePanel";
import { MessageInput } from "../components/chat/MessageInput";
import { ChannelMembersPanel } from "../components/channels/ChannelMembersPanel";
import type {
  ChannelMemberInfo,
  TeamResponse,
} from "../components/channels/types";
import { TeamSettings } from "../components/channels/TeamSettings";
import { SettingsPage } from "../components/settings/SettingsPage";
import { DecisionsInbox } from "../components/decisions/DecisionsInbox";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

/** Rendered when the URL doesn't match any known route. */
function NotFoundView() {
  return (
    <EmptyShell
      label="This view is not available."
      icon="[chorus::404]"
      extra={
        <Link to={rootPath()} style={{ color: "var(--color-foreground)" }}>
          Back to start
        </Link>
      }
    />
  );
}

export function MainPanel() {
  const { currentUser, currentUserId } = useStore();
  const subject = useRouteSubject();
  const currentChannel = useCurrentChannel();
  const currentAgent = useCurrentAgent();
  const location = useLocation();

  // Derive the rest of the legacy view-state from the URL. With the URL
  // schema in place these are no longer in the store.
  const showSettings = location.pathname.startsWith(settingsPath());
  const showDecisions = location.pathname === inboxPath();
  const currentTaskDetail = useCurrentTaskDetail();
  const activeTab: ActiveTab = (() => {
    switch (subject.kind) {
      case "task":
        return "tasks";
      case "channel":
        return subject.view === "tasks" ? "tasks" : "chat";
      case "agent-tab":
        return subject.tab;
      case "dm":
        return "chat";
      default:
        return "chat";
    }
  })();
  const agents = useAgents();
  const humans = useHumans();
  const teams = useTeams();
  const { getAgentConversationId } = useInbox();
  const { refreshChannels, refreshAgents, refreshTeams } = useRefresh();
  const chatTarget = useTarget();
  const activeConversationId =
    currentChannel?.id ??
    (currentAgent ? getAgentConversationId(currentAgent.name) : null);
  const chatHistory = useHistory(
    currentUserId,
    activeTab === "chat" ? chatTarget : null,
    activeConversationId,
  );
  useTraceSubscription(currentUserId || null);
  const [members, setMembers] = useState<ChannelMemberInfo[]>([]);
  const [membersLoading, setMembersLoading] = useState(false);
  const [showMembersPanel, setShowMembersPanel] = useState(false);
  const [showTeamSettings, setShowTeamSettings] = useState(false);
  const chatAgentNames = currentAgent
    ? [currentAgent.name]
    : currentChannel
      ? members
          .filter((member) => member.memberType === "agent")
          .map((member) => member.memberName)
      : [];

  const [teamDetails, setTeamDetails] = useState<TeamResponse | null>(null);
  const [teamSettingsLoading, setTeamSettingsLoading] = useState(false);

  const showHeader = currentChannel || currentAgent;
  const isTeamChannel = currentChannel?.channel_type === "team";
  const isSystemChannel = currentChannel?.channel_type === "system";
  const canInviteMembers = Boolean(
    currentChannel?.id && !isTeamChannel && !isSystemChannel,
  );
  const channelId = currentChannel?.id;
  const selectedTeam =
    isTeamChannel && channelId
      ? teams.find((team) => team.channel_id === channelId) ?? null
      : null;

  useEffect(() => {
    setShowMembersPanel(false);
    setShowTeamSettings(false);

  }, [channelId]);

  // Close transient panels when settings page opens to avoid them reappearing on close.
  useEffect(() => {
    if (showSettings) {
      setShowMembersPanel(false);
      setShowTeamSettings(false);
    }
  }, [showSettings]);

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
    if (!selectedTeam) {
      console.error("Failed to resolve team for current channel", {
        channelId: currentChannel.id,
        channelName: currentChannel.name,
      });
      return;
    }
    setTeamSettingsLoading(true);
    setShowTeamSettings(true);
    try {
      setTeamDetails(await getTeam(selectedTeam.id));
    } catch (error) {
      console.error("Failed to load team settings", error);
      setShowTeamSettings(false);
    } finally {
      setTeamSettingsLoading(false);
    }
  }

  async function refreshSelectedTeam() {
    if (!teamDetails) return;
    setTeamDetails(await getTeam(teamDetails.team.id));
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

  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        overflow: "hidden",
        background: "transparent",
        paddingTop: showSettings ? 0 : 10,
      }}
    >
      {showSettings ? (
        <SettingsPage />
      ) : showDecisions ? (
        <DecisionsInbox />
      ) : currentTaskDetail ? (
        <TaskDetail />
      ) : subject.kind === "unknown" ? (
        <NotFoundView />
      ) : (
        <>
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
              <h1 className="sr-only">
                {currentAgent
                  ? `Chat with ${currentAgent.display_name ?? currentAgent.name}`
                  : currentChannel
                    ? `Chat in ${currentChannel.name}`
                    : "Chat"}
              </h1>
              <ChatPanel
                target={chatTarget}
                conversationId={activeConversationId}
                conversationAgentNames={chatAgentNames}
                messages={chatHistory.messages}
                loading={chatHistory.loading}
                lastReadSeq={chatHistory.lastReadSeq}

              />
              <MessageInput
                target={chatTarget}
                conversationId={activeConversationId}
                history={chatHistory}
              />
            </>
          )}
          {activeTab === "tasks" && (
            <>
              <h1 className="sr-only">Tasks</h1>
              <TasksPanel />
            </>
          )}
          {activeTab === "profile" && (
            <>
              <h1 className="sr-only">
                {currentAgent
                  ? `Profile: ${currentAgent.display_name ?? currentAgent.name}`
                  : "Profile"}
              </h1>
              <ProfilePanel />
            </>
          )}
          {activeTab === "activity" && currentAgent && (
            <>
              <h1 className="sr-only">
                Activity: {currentAgent.display_name ?? currentAgent.name}
              </h1>
              <TelescopeActivity
                agentId={currentAgent.id}
                agentName={currentAgent.name}
              />
            </>
          )}
          {activeTab === "workspace" && currentAgent && (
            <>
              <h1 className="sr-only">
                Workspace: {currentAgent.display_name ?? currentAgent.name}
              </h1>
              <WorkspacePanel agentId={currentAgent.id} />
            </>
          )}
          {!showHeader && (
            <EmptyShell label="Select a channel or agent to get started" />
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
        </>
      )}
    </div>
  );
}
