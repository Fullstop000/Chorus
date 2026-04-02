import { useState, useEffect, useRef } from 'react'
import { X, Paperclip } from 'lucide-react'
import { useApp, useTarget } from '../../store'
import { useHistory } from '../../hooks/useHistory'
import { useVisibilityTracking } from '@/hooks/useVisibilityTracking'
import { MessageItem } from './MessageItem'
import { ToastRegion } from './ToastRegion'
import { MentionTextarea } from './MentionTextarea'
import type { MentionMember } from './MentionTextarea'
import { sendMessage } from '../../api'
import './ThreadPanel.css'

interface ThreadPanelProps {
  variant?: 'drawer' | 'content'
}

export function ThreadPanel({ variant = 'drawer' }: ThreadPanelProps) {
  const {
    currentUser,
    openThreadMsg,
    setOpenThreadMsg,
    serverInfo,
    agents,
    teams,
    selectedAgent,
    selectedChannelId,
    getAgentConversationId,
    applyReadCursorAck,
  } = useApp()
  const members: MentionMember[] = [
    ...agents.map((a) => ({ name: a.name, type: 'agent' as const })),
    ...(serverInfo?.humans ?? []).map((h) => ({ name: h.name, type: 'human' as const })),
    ...teams.map((team) => ({ name: team.name, type: 'team' as const })),
  ]
  const mainTarget = useTarget()
  const threadTarget = mainTarget && openThreadMsg
    ? `${mainTarget}:${openThreadMsg.id}`
    : null
  const threadConversationId =
    selectedChannelId ?? (selectedAgent ? getAgentConversationId(selectedAgent.name) : null)

  const {
    messages,
    loading,
    lastReadSeq,
    loadedTarget,
    reportVisibleSeq,
    addOptimisticMessage,
    ackOptimisticMessage,
    failOptimisticMessage,
    retryOptimisticMessage,
  } = useHistory(currentUser, threadTarget, threadConversationId, {
    threadParentId: openThreadMsg?.id ?? null,
    onReadCursorAck: applyReadCursorAck,
  })
  const [content, setContent] = useState('')
  const [sending, setSending] = useState(false)
  const [toasts, setToasts] = useState<Array<{ id: string; message: string }>>([])
  const bottomRef = useRef<HTMLDivElement>(null)
  const scrollContainerRef = useRef<HTMLDivElement>(null)
  const messageRefs = useRef<Record<string, HTMLDivElement | null>>({})
  const pendingInitialScrollTargetRef = useRef<string | null>(null)

  const { scheduleBatchVisibilityCheck, resetHighestVisibleSeq } = useVisibilityTracking(reportVisibleSeq)

  useEffect(() => {
    pendingInitialScrollTargetRef.current = threadTarget
    resetHighestVisibleSeq()
  }, [threadTarget, resetHighestVisibleSeq])

  useEffect(() => {
    const container = scrollContainerRef.current
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
      const items = messages.map((message) => ({
        seq: message.seq,
        element: messageRefs.current[message.id],
      }))
      scheduleBatchVisibilityCheck(items, container)
      pendingInitialScrollTargetRef.current = null
      return
    }

    const distFromBottom = container.scrollHeight - container.scrollTop - container.clientHeight
    if (distFromBottom < 100) {
      bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
    }
  }, [lastReadSeq, loadedTarget, loading, messages, scheduleBatchVisibilityCheck, threadTarget])

  useEffect(() => {
    const container = scrollContainerRef.current
    if (!container || !threadTarget || loadedTarget !== threadTarget || loading) return

    const handleScroll = () => {
      const items = messages.map((message) => ({
        seq: message.seq,
        element: messageRefs.current[message.id],
      }))
      scheduleBatchVisibilityCheck(items, container)
    }

    handleScroll()
    container.addEventListener('scroll', handleScroll, { passive: true })
    window.addEventListener('resize', handleScroll)
    document.addEventListener('visibilitychange', handleScroll)
    return () => {
      container.removeEventListener('scroll', handleScroll)
      window.removeEventListener('resize', handleScroll)
      document.removeEventListener('visibilitychange', handleScroll)
    }
  }, [loadedTarget, loading, messages, scheduleBatchVisibilityCheck, threadTarget])

  // Reset input when switching thread
  useEffect(() => {
    setContent('')
  }, [openThreadMsg?.id])

  useEffect(() => {
    if (toasts.length === 0) return
    const timer = window.setTimeout(() => {
      setToasts((current) => current.slice(1))
    }, 4000)
    return () => window.clearTimeout(timer)
  }, [toasts])

  async function handleSend() {
    if (!threadTarget || !currentUser || !content.trim()) return
    setSending(true)
    let optimisticHandle: ReturnType<typeof addOptimisticMessage> | null = null
    try {
      const handle = addOptimisticMessage({
        content: content.trim(),
      })
      optimisticHandle = handle
      if (!threadConversationId || !openThreadMsg) throw new Error('thread unavailable')
      const sendAck = await sendMessage(threadConversationId, content.trim(), [], {
        clientNonce: handle.clientNonce,
        threadParentId: openThreadMsg.id,
      })
      ackOptimisticMessage(handle, {
        messageId: sendAck.messageId,
        seq: sendAck.seq,
        createdAt: sendAck.createdAt,
        clientNonce: sendAck.clientNonce,
      })
      setContent('')
    } catch (e) {
      console.error('Thread send failed:', e)
      const message = e instanceof Error ? e.message : String(e)
      if (optimisticHandle) {
        failOptimisticMessage(optimisticHandle, message)
      }
      setToasts((current) => [
        ...current,
        { id: `thread-send-failed-${Date.now()}`, message: 'Message failed to send' },
      ])
    } finally {
      setSending(false)
    }
  }

  async function handleRetryMessage(message: typeof messages[number]) {
    if (!threadTarget || !currentUser) return
    const retryHandle = retryOptimisticMessage(message.id)
    if (!retryHandle) return
    try {
      if (!threadConversationId || !openThreadMsg) throw new Error('thread unavailable')
      const sendAck = await sendMessage(
        threadConversationId,
        message.content,
        message.attachments?.map((attachment) => attachment.id) ?? [],
        { clientNonce: retryHandle.clientNonce, threadParentId: openThreadMsg.id }
      )
      ackOptimisticMessage(retryHandle, {
        messageId: sendAck.messageId,
        seq: sendAck.seq,
        createdAt: sendAck.createdAt,
        clientNonce: sendAck.clientNonce,
      })
    } catch (retryError) {
      const retryMessage = retryError instanceof Error ? retryError.message : String(retryError)
      failOptimisticMessage(retryHandle, retryMessage)
      setToasts((current) => [
        ...current,
        { id: `thread-retry-failed-${Date.now()}`, message: 'Message failed to send' },
      ])
    }
  }

  if (!openThreadMsg) return null

  return (
    <div className={`thread-panel${variant === 'content' ? ' thread-panel--content' : ''}`}>
      <div className="thread-header">
        <div className="thread-header-copy">
          <span className="thread-kicker">[ctx::thread]</span>
        </div>
        <button className="thread-close-btn" type="button" onClick={() => setOpenThreadMsg(null)} title="Close thread">
          <X size={16} strokeWidth={2} />
        </button>
      </div>

      <div className="thread-body" ref={scrollContainerRef}>
        {/* no onReply — replies aren't nested */}
        <div className="thread-parent-wrapper">
          <MessageItem
            message={openThreadMsg}
            currentUser={currentUser}
          />
        </div>

        <div className="thread-replies">
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
                  onRetry={handleRetryMessage}
                />
              </div>
            ))
          )}
          <div ref={bottomRef} />
        </div>
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
        onDismiss={(id) => setToasts((current) => current.filter((toast) => toast.id !== id))}
      />
    </div>
  )
}
