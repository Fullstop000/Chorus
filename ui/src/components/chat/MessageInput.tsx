import { useState, useRef, useMemo } from "react";
import { Paperclip, Plus } from "lucide-react";
import { useStore } from "../../store";
import {
  useAgents,
  useTeams,
  useHumans,
  useChannels,
  useChannelMembers,
} from "../../hooks/data";
import { useCurrentChannel } from "../../hooks/useRouteSubject";
import { useHistory } from "../../hooks/useHistory";
import { sendMessage, createTasks, uploadFile } from "../../data";
import { MentionTextarea } from "./MentionTextarea";
import type { MentionMember } from "./MentionTextarea";
import { FormError } from "@/components/ui/form";

interface Props {
  target: string | null;
  conversationId: string | null;
  history: ReturnType<typeof useHistory>;
  /**
   * Hide the "also create as task" affordance. The checkbox reads
   * `currentChannel` from the store, which is the parent channel even when
   * MessageInput is embedded inside TaskDetail's sub-channel view. Toggling
   * it there would silently create a peer task on the parent. Sub-tasks of
   * tasks are not a supported primitive, so the caller (TaskDetail) hides
   * the checkbox entirely.
   */
  hideCreateTaskCheckbox?: boolean;
  /**
   * Override the default "Message {target}" placeholder. Used by TaskDetail
   * so the user sees "Message task #7" instead of the internal sub-channel
   * slug `pr-72-polish__task-7`, which leaks the backfill naming scheme.
   */
  placeholder?: string;
}

export function MessageInput({
  target,
  conversationId,
  history,
  hideCreateTaskCheckbox = false,
  placeholder: placeholderOverride,
}: Props) {
  const { currentUser, currentUserId } = useStore();
  const currentChannel = useCurrentChannel();
  const pushToast = useStore((s) => s.pushToast);
  const agents = useAgents();
  const teams = useTeams();
  const humans = useHumans();
  const { systemChannels } = useChannels();
  const channelMembers = useChannelMembers(currentChannel?.id ?? null);
  const [content, setContent] = useState("");
  const [alsoTask, setAlsoTask] = useState(false);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pendingFiles, setPendingFiles] = useState<File[]>([]);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const allMembers: MentionMember[] = useMemo(
    () => [
      ...agents.map((a) => ({ name: a.name, displayName: a.display_name, type: "agent" as const })),
      ...humans.map((h) => ({ name: h.name, type: "human" as const })),
      ...teams.map((team) => ({ name: team.name, type: "team" as const })),
    ],
    [agents, humans, teams],
  );

  const channelMemberSet = useMemo(
    () => new Set(channelMembers.map((cm) => cm.memberName)),
    [channelMembers],
  );

  // In a channel context, only suggest members who belong to the channel.
  // In DM or no-channel context, show all members.
  const members = useMemo(
    () =>
      currentChannel?.id
        ? allMembers.filter((m) => channelMemberSet.has(m.name))
        : allMembers,
    [allMembers, channelMemberSet, currentChannel?.id],
  );

  const isReadOnlySystem = !!(
    currentChannel &&
    systemChannels.some((c) => c.name === currentChannel.name && c.read_only)
  );

  const placeholder = isReadOnlySystem
    ? `${target} is read-only — agent breadcrumbs only`
    : placeholderOverride
      ? placeholderOverride
      : target
        ? `Message ${target}`
        : "Select a channel to message";

  async function handleSend() {
    if (
      !target ||
      !currentUser ||
      (!content.trim() && pendingFiles.length === 0)
    )
      return;
    setSending(true);
    setError(null);
    const trimmedContent = content.trim();
    try {
      const attachmentIds: string[] = [];
      for (const file of pendingFiles) {
        const res = await uploadFile(file);
        attachmentIds.push(res.id);
      }

      if (!conversationId) throw new Error("conversation unavailable");
      const sendAck = await sendMessage(
        conversationId,
        trimmedContent,
        attachmentIds,
        {
          suppressAgentDelivery: alsoTask && !!currentChannel,
          suppressEvent: true,
        },
      );
      history.appendMessage({
        id: sendAck.messageId,
        seq: sendAck.seq,
        content: trimmedContent,
        senderId: currentUserId,
        senderName: currentUser,
        senderType: "human",
        senderDeleted: false,
        createdAt: sendAck.createdAt,
        attachments: attachmentIds.map((id, index) => ({
          id,
          filename: pendingFiles[index]?.name ?? "attachment",
        })),
      });
      setContent("");
      setPendingFiles([]);
      setAlsoTask(false);
    } catch (e) {
      console.error("Send failed:", e);
      const message = e instanceof Error ? e.message : String(e);
      setError(message);
      pushToast({ id: crypto.randomUUID(), message: "Message failed to send", level: "error" });
    } finally {
      setSending(false);
    }

    if (alsoTask && currentChannel && trimmedContent) {
      try {
        if (!currentChannel.id) throw new Error("channel unavailable");
        await createTasks(currentChannel.id, [trimmedContent]);
      } catch (taskError) {
        const message =
          taskError instanceof Error ? taskError.message : String(taskError);
        setError(message);
        pushToast({ id: crypto.randomUUID(), message: "Task creation failed", level: "error" });
      }
    }
  }

  function handleFileChange(e: React.ChangeEvent<HTMLInputElement>) {
    const files = Array.from(e.target.files ?? []);
    setError(null);
    setPendingFiles((prev) => [...prev, ...files]);
    if (fileInputRef.current) fileInputRef.current.value = "";
  }

  return (
    <div className="message-input-area">
      {error && <FormError>{error}</FormError>}
      {pendingFiles.length > 0 && (
        <div className="message-input-files">
          {pendingFiles.map((f, i) => (
            <span key={i} className="file-chip">
              <Paperclip size={12} />
              {f.name}
              <button
                onClick={() => {
                  setError(null);
                  setPendingFiles((prev) => prev.filter((_, j) => j !== i));
                }}
              >
                ×
              </button>
            </span>
          ))}
        </div>
      )}
      <div className="message-input-row">
        <button
          className="message-input-btn attach-btn"
          onClick={() => fileInputRef.current?.click()}
          disabled={!target || isReadOnlySystem}
          title="Attach file"
        >
          <Plus size={16} />
        </button>
        <input
          ref={fileInputRef}
          type="file"
          multiple
          style={{ display: "none" }}
          onChange={handleFileChange}
        />
        <MentionTextarea
          className="message-input-textarea"
          placeholder={placeholder}
          value={content}
          onChange={(value) => {
            setError(null);
            setContent(value);
          }}
          onEnter={handleSend}
          disabled={!target || sending || isReadOnlySystem}
          rows={1}
          members={members}
        />
        <button
          className="message-input-send"
          onClick={handleSend}
          disabled={
            !target ||
            sending ||
            isReadOnlySystem ||
            (!content.trim() && pendingFiles.length === 0)
          }
        >
          {sending ? "..." : "Send"}
        </button>
      </div>
      {currentChannel && !isReadOnlySystem && !hideCreateTaskCheckbox && (
        <div className="message-input-footer">
          <label className="task-checkbox-label">
            <input
              type="checkbox"
              checked={alsoTask}
              onChange={(e) => setAlsoTask(e.target.checked)}
            />
            Also create as a task
          </label>
        </div>
      )}
    </div>
  );
}
