import { useState, useEffect, useRef } from 'react'
import { X, Paperclip } from 'lucide-react'
import { useApp, useTarget } from '../store'
import { useHistory } from '../hooks/useHistory'
import { MessageItem } from './MessageItem'
import { MentionTextarea } from './MentionTextarea'
import type { MentionMember } from './MentionTextarea'
import { sendMessage } from '../api'
import './ThreadPanel.css'

export function ThreadPanel() {
  const { currentUser, openThreadMsg, setOpenThreadMsg, serverInfo, agents, teams } = useApp()
  const members: MentionMember[] = [
    ...agents.map((a) => ({ name: a.name, type: 'agent' as const })),
    ...(serverInfo?.humans ?? []).map((h) => ({ name: h.name, type: 'human' as const })),
    ...teams.map((team) => ({ name: team.name, type: 'team' as const })),
  ]
  const mainTarget = useTarget()
  const threadTarget = mainTarget && openThreadMsg
    ? `${mainTarget}:${openThreadMsg.id}`
    : null

  const { messages, loading, lastReadSeq, loadedTarget } = useHistory(currentUser, threadTarget)
  const [content, setContent] = useState('')
  const [sending, setSending] = useState(false)
  const bottomRef = useRef<HTMLDivElement>(null)
  const repliesContainerRef = useRef<HTMLDivElement>(null)
  const messageRefs = useRef<Record<string, HTMLDivElement | null>>({})
  const pendingInitialScrollTargetRef = useRef<string | null>(null)

  useEffect(() => {
    pendingInitialScrollTargetRef.current = threadTarget
  }, [threadTarget])

  useEffect(() => {
    const container = repliesContainerRef.current
    if (!container) return

    const firstUnreadMessage = messages.find((message) => message.seq > lastReadSeq)

    if (
      pendingInitialScrollTargetRef.current === threadTarget &&
      loadedTarget === threadTarget &&
      !loading
    ) {
      const unreadAnchor = firstUnreadMessage
        ? messageRefs.current[firstUnreadMessage.id]
        : null
      if (unreadAnchor) {
        container.scrollTop = Math.max(unreadAnchor.offsetTop - 96, 0)
      } else {
        bottomRef.current?.scrollIntoView({ behavior: 'auto' })
      }
      pendingInitialScrollTargetRef.current = null
      return
    }

    const distFromBottom = container.scrollHeight - container.scrollTop - container.clientHeight
    if (distFromBottom < 100) {
      bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
    }
  }, [lastReadSeq, loadedTarget, loading, messages, threadTarget])

  // Reset input when switching thread
  useEffect(() => {
    setContent('')
  }, [openThreadMsg?.id])

  async function handleSend() {
    if (!threadTarget || !currentUser || !content.trim()) return
    setSending(true)
    try {
      await sendMessage(currentUser, threadTarget, content.trim())
      setContent('')
    } catch (e) {
      console.error('Thread send failed:', e)
    } finally {
      setSending(false)
    }
  }

  if (!openThreadMsg) return null

  return (
    <div className="thread-panel">
      {/* Header */}
      <div className="thread-header">
        <div className="thread-header-copy">
          <span className="thread-kicker">[ctx::thread]</span>
          <span className="thread-title">Thread</span>
        </div>
        <button className="thread-close-btn" type="button" onClick={() => setOpenThreadMsg(null)} title="Close thread">
          <X size={16} strokeWidth={2} />
        </button>
      </div>

      <div className="thread-body">
        {/* Parent message (copy only, no reply) */}
        <div className="thread-parent-wrapper">
          <MessageItem
            message={openThreadMsg}
            currentUser={currentUser}
          />
        </div>

        {/* Replies */}
        <div className="thread-replies" ref={repliesContainerRef}>
          {messages.length === 0 ? (
            <div className="thread-empty">No replies yet</div>
          ) : (
            messages.map((msg, i) => (
              <div
                key={msg.id}
                ref={(node) => {
                  messageRefs.current[msg.id] = node
                }}
                style={{ scrollMarginTop: 96 }}
              >
                <MessageItem
                  message={msg}
                  currentUser={currentUser}
                  prevMessage={messages[i - 1]}
                />
              </div>
            ))
          )}
          <div ref={bottomRef} />
        </div>
      </div>

      {/* Input */}
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
    </div>
  )
}
