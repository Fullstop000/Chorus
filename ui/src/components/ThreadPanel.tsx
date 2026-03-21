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
  const { currentUser, openThreadMsg, setOpenThreadMsg, serverInfo } = useApp()
  const members: MentionMember[] = [
    ...(serverInfo?.agents ?? []).map((a) => ({ name: a.name, type: 'agent' as const })),
    ...(serverInfo?.humans ?? []).map((h) => ({ name: h.name, type: 'human' as const })),
  ]
  const mainTarget = useTarget()
  const threadTarget = mainTarget && openThreadMsg
    ? `${mainTarget}:${openThreadMsg.id}`
    : null

  const { messages, refresh } = useHistory(currentUser, threadTarget)
  const [content, setContent] = useState('')
  const [sending, setSending] = useState(false)
  const bottomRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [messages])

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
      refresh()
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
        <span className="thread-title">Thread</span>
        <button className="thread-close-btn" onClick={() => setOpenThreadMsg(null)} title="Close thread">
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
        <div className="thread-replies">
          {messages.length === 0 ? (
            <div className="thread-empty">No replies yet</div>
          ) : (
            messages.map((msg, i) => (
              <MessageItem
                key={msg.id}
                message={msg}
                currentUser={currentUser}
                prevMessage={messages[i - 1]}
              />
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
