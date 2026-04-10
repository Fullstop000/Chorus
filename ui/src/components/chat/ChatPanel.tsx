import { Search, Settings2, Users, Activity } from "lucide-react";
import { useStore } from "../../store";
import { useChannels } from "../../hooks/data";
import { MessageList } from "./MessageList";
import { CrossAgentTimeline } from "./CrossAgentTimeline";
import type { HistoryMessage } from "./types";
import "./ChatPanel.css";

interface ChatHeaderProps {
  memberCount?: number | null;
  membersOpen: boolean;
  isTeamChannel?: boolean;
  timelineOpen?: boolean;
  onToggleTimeline?: () => void;
  onToggleMembers: () => void;
  onOpenTeamSettings?: () => void;
}

export function ChatHeader({
  memberCount,
  membersOpen,
  isTeamChannel,
  timelineOpen,
  onToggleTimeline,
  onToggleMembers,
  onOpenTeamSettings,
}: ChatHeaderProps) {
  const { currentChannel, currentAgent } = useStore();
  const { channels } = useChannels();
  const channelInfo = currentChannel
    ? channels.find((channel) => channel.name === currentChannel.name)
    : null;

  const headerName = currentChannel
    ? `#${currentChannel.name}`
    : currentAgent
      ? `@${currentAgent.display_name ?? currentAgent.name}`
      : "Select a channel";

  const headerDesc =
    channelInfo?.description ?? currentAgent?.description ?? "";
  const headerIcon = currentChannel ? "#" : currentAgent ? "@" : "?";

  return (
    <div className="chat-header">
      <div className="chat-header-copy">
        <div className="chat-header-title-row">
          <span className="chat-header-icon">{headerIcon}</span>
          <span className="chat-header-name">{headerName}</span>
          {headerDesc && <span className="chat-header-desc">{headerDesc}</span>}
        </div>
      </div>
      <div className="chat-header-actions">
        {onToggleTimeline && (
          <button
            className={`chat-header-btn${timelineOpen ? " active" : ""}`}
            type="button"
            aria-label={timelineOpen ? "Hide agent timeline" : "Show all agent activity"}
            onClick={onToggleTimeline}
          >
            <Activity size={15} />
          </button>
        )}
        {currentChannel && (
          <button
            className={`chat-header-member-btn${membersOpen ? " active" : ""}`}
            type="button"
            aria-label={membersOpen ? "Hide members list" : "Show members list"}
            onClick={onToggleMembers}
          >
            <Users size={14} />
            <span>{memberCount ?? "..."}</span>
          </button>
        )}
        <button
          className="chat-header-btn"
          type="button"
          aria-label="Search room"
        >
          <Search size={15} />
        </button>
        {isTeamChannel && onOpenTeamSettings && (
          <button
            className="chat-header-btn"
            type="button"
            aria-label="Open team settings"
            onClick={onOpenTeamSettings}
          >
            <Settings2 size={15} />
          </button>
        )}
      </div>
    </div>
  );
}

interface ChatPanelProps {
  target: string | null;
  conversationId: string | null;
  messages: HistoryMessage[];
  loading: boolean;
  lastReadSeq: number;
  timelineOpen?: boolean;
}

export function ChatPanel({
  target,
  conversationId,
  messages,
  loading,
  lastReadSeq,
  timelineOpen,
}: ChatPanelProps) {
  const { currentUser, setOpenThreadMsg } = useStore();

  if (!target) {
    return (
      <div className="chat-panel">
        <div className="chat-messages-empty">
          Select a channel or agent to start chatting.
        </div>
      </div>
    );
  }

  return (
    <div className="chat-panel">
      {timelineOpen && <CrossAgentTimeline />}
      <MessageList
        targetKey={target}
        conversationId={conversationId}
        messages={messages}
        loading={loading}
        lastReadSeq={lastReadSeq}
        currentUser={currentUser}
        onReply={setOpenThreadMsg}
      />
    </div>
  );
}
