import { useState, useEffect, useRef, type KeyboardEvent } from 'react'
import { useApp, useTarget } from '../store'
import { useHistory } from '../hooks/useHistory'
import { MessageItem } from './MessageItem'
import { sendMessage } from '../api'
import './ThreadPanel.css'

export function ThreadPanel() {
  const { currentUser, openThreadMsg, setOpenThreadMsg } = useApp()
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

  function handleKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      handleSend()
    }
  }

  if (!openThreadMsg) return null

  return (
    <div className="thread-panel">
      <div className="thread-header">
        <span className="thread-title">Thread</span>
        <button className="thread-close-btn" onClick={() => setOpenThreadMsg(null)} title="Close thread">
          ×
        </button>
      </div>

      <div className="thread-parent">
        <MessageItem
          message={openThreadMsg}
          currentUser={currentUser}
        />
      </div>

      <div className="thread-replies-label">
        {messages.length === 0 ? 'No replies yet' : `${messages.length} ${messages.length === 1 ? 'reply' : 'replies'}`}
      </div>

      <div className="thread-messages">
        {messages.map((msg, i) => (
          <MessageItem
            key={msg.id}
            message={msg}
            currentUser={currentUser}
            prevMessage={messages[i - 1]}
          />
        ))}
        <div ref={bottomRef} />
      </div>

      <div className="thread-input-area">
        <div className="thread-input-row">
          <textarea
            className="thread-input-textarea"
            placeholder="Reply in thread…"
            value={content}
            onChange={(e) => setContent(e.target.value)}
            onKeyDown={handleKeyDown}
            disabled={sending}
            rows={1}
          />
          <button
            className="thread-input-send"
            onClick={handleSend}
            disabled={sending || !content.trim()}
          >
            {sending ? '...' : 'Reply'}
          </button>
        </div>
      </div>
    </div>
  )
}
