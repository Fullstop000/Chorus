import { useState, useEffect, useMemo } from "react";
import { X, Paperclip } from "lucide-react";
import { useStore } from "../../store";
import {
  useAgents,
  useTeams,
  useHumans,
  useInbox,
  useTarget,
  useChannelMembers,
} from "../../hooks/data";
import { useHistory } from "../../hooks/useHistory";
import { MessageItem } from "./MessageItem";
import { MessageList } from "./MessageList";
import { ToastRegion } from "./ToastRegion";
import { MentionTextarea } from "./MentionTextarea";
import type { MentionMember } from "./MentionTextarea";
import { sendMessage } from "../../data";
import "./ThreadPanel.css";

interface ThreadPanelProps {
  variant?: "drawer" | "content";
}

export function ThreadPanel({ variant = "drawer" }: ThreadPanelProps) {
  const {
    currentUser,
    currentChannel,
    currentAgent,
    openThreadMsg,
    setOpenThreadMsg,
  } = useStore();
  const agents = useAgents();
  const teams = useTeams();
  const humans = useHumans();
  const { getAgentConversationId } = useInbox();
  const channelMembers = useChannelMembers(currentChannel?.id ?? null);
  const allMembers: MentionMember[] = useMemo(
    () => [
      ...agents.map((a) => ({ name: a.name, type: "agent" as const })),
      ...humans.map((h) => ({ name: h.name, type: "human" as const })),
      ...teams.map((team) => ({ name: team.name, type: "team" as const })),
    ],
    [agents, humans, teams],
  );
  const channelMemberSet = useMemo(
    () => new Set(channelMembers.map((cm) => cm.memberName)),
    [channelMembers],
  );
  const members = useMemo(
    () =>
      currentChannel?.id
        ? allMembers.filter((m) => channelMemberSet.has(m.name))
        : allMembers,
    [allMembers, channelMemberSet, currentChannel?.id],
  );
  const mainTarget = useTarget();
  const threadTarget =
    mainTarget && openThreadMsg ? `${mainTarget}:${openThreadMsg.id}` : null;
  const threadConversationId =
    currentChannel?.id ??
    (currentAgent ? getAgentConversationId(currentAgent.name) : null);

  const { messages, loading, lastReadSeq, appendMessage } = useHistory(
    currentUser,
    threadTarget,
    threadConversationId,
    {
      threadParentId: openThreadMsg?.id ?? null,
    },
  );
  const [content, setContent] = useState("");
  const [sending, setSending] = useState(false);
  const [toasts, setToasts] = useState<Array<{ id: string; message: string }>>(
    [],
  );

  useEffect(() => {
    setContent("");
  }, [openThreadMsg?.id]);

  useEffect(() => {
    if (toasts.length === 0) return;
    const timer = window.setTimeout(() => {
      setToasts((current) => current.slice(1));
    }, 4000);
    return () => window.clearTimeout(timer);
  }, [toasts]);

  async function handleSend() {
    if (!threadTarget || !currentUser || !content.trim()) return;
    setSending(true);
    try {
      if (!threadConversationId || !openThreadMsg)
        throw new Error("thread unavailable");
      const sendAck = await sendMessage(
        threadConversationId,
        content.trim(),
        [],
        {
          threadParentId: openThreadMsg.id,
          suppressEvent: true,
        },
      );
      appendMessage({
        id: sendAck.messageId,
        seq: sendAck.seq,
        content: content.trim(),
        senderName: currentUser,
        senderType: "human",
        senderDeleted: false,
        createdAt: sendAck.createdAt,
      });
      setContent("");
    } catch (e) {
      console.error("Thread send failed:", e);
      setToasts((current) => [
        ...current,
        {
          id: `thread-send-failed-${Date.now()}`,
          message: "Message failed to send",
        },
      ]);
    } finally {
      setSending(false);
    }
  }

  if (!openThreadMsg) return null;

  return (
    <div
      className={`thread-panel${variant === "content" ? " thread-panel--content" : ""}`}
    >
      <div className="thread-header">
        <div className="thread-header-copy">
          <span className="thread-kicker">[ctx::thread]</span>
        </div>
        <button
          className="thread-close-btn"
          type="button"
          onClick={() => setOpenThreadMsg(null)}
          title="Close thread"
        >
          <X size={16} strokeWidth={2} />
        </button>
      </div>

      <div className="thread-body">
        <div className="thread-parent-wrapper">
          <MessageItem message={openThreadMsg} currentUser={currentUser} />
        </div>

        <MessageList
          targetKey={threadTarget ?? ""}
          conversationId={threadConversationId}
          messages={messages}
          loading={loading}
          lastReadSeq={lastReadSeq}
          currentUser={currentUser}
          emptyLabel="No replies yet"
          threadParentId={openThreadMsg?.id}
        />
      </div>

      <div className="thread-input-area">
        <div className="thread-input-row">
          <MentionTextarea
            className="thread-input-textarea"
            placeholder="Message thread"
            value={content}
            onChange={setContent}
            onEnter={handleSend}
            disabled={sending}
            rows={1}
            members={members}
          />
          <div className="thread-input-footer">
            <button className="thread-attach-btn" title="Attach" disabled>
              <Paperclip size={16} />
            </button>
            <button
              className="thread-send-btn"
              type="button"
              onClick={handleSend}
              disabled={sending || !content.trim()}
            >
              Send
            </button>
          </div>
        </div>
      </div>
      <ToastRegion
        toasts={toasts}
        onDismiss={(id) =>
          setToasts((current) => current.filter((toast) => toast.id !== id))
        }
      />
    </div>
  );
}
