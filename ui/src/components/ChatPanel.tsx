import { useEffect, useRef } from 'react'
import { Search, Settings2, Users } from 'lucide-react'
import { useApp } from '../store'
import { MessageItem } from './MessageItem'
import type { HistoryMessage } from '../types'
import './ChatPanel.css'

interface ChatHeaderProps {
  memberCount?: number | null
  membersOpen: boolean
  isTeamChannel?: boolean
  onToggleMembers: () => void
  onOpenTeamSettings?: () => void
}

export function ChatHeader({
  memberCount,
  membersOpen,
  isTeamChannel,
  onToggleMembers,
  onOpenTeamSettings,
}: ChatHeaderProps) {
  const { selectedChannel, selectedAgent, channels } = useApp()
  const channelInfo = selectedChannel
    ? channels.find((channel) => `#${channel.name}` === selectedChannel)
    : null

  const headerName = selectedChannel
    ? selectedChannel
    : selectedAgent
    ? `@${selectedAgent.display_name ?? selectedAgent.name}`
    : 'Select a channel'

  const headerDesc = channelInfo?.description ?? selectedAgent?.description ?? ''
  const headerIcon = selectedChannel ? '#' : selectedAgent ? '@' : '?'

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
        {selectedChannel && (
          <button
            className={`chat-header-member-btn${membersOpen ? ' active' : ''}`}
            type="button"
            aria-label={membersOpen ? 'Hide members list' : 'Show members list'}
            onClick={onToggleMembers}
          >
            <Users size={14} />
            <span>{memberCount ?? '...'}</span>
          </button>
        )}
        <button className="chat-header-btn" type="button" aria-label="Search room">
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
  )
}

interface ChatPanelProps {
  target: string | null
  messages: HistoryMessage[]
  loading: boolean
  lastReadSeq: number
  loadedTarget: string | null
  reportVisibleSeq: (seq: number) => void
  onRetryMessage?: (message: HistoryMessage) => void
}

export function ChatPanel({
  target,
  messages,
  loading,
  lastReadSeq,
  loadedTarget,
  reportVisibleSeq,
  onRetryMessage,
}: ChatPanelProps) {
  const { currentUser, setOpenThreadMsg } = useApp()
  const bottomRef = useRef<HTMLDivElement>(null)
  const scrollContainerRef = useRef<HTMLDivElement>(null)
  const messageRefs = useRef<Record<string, HTMLDivElement | null>>({})
  const pendingInitialScrollTargetRef = useRef<string | null>(null)

  useEffect(() => {
    pendingInitialScrollTargetRef.current = target
  }, [target])

  useEffect(() => {
    const container = scrollContainerRef.current
    if (!container) return

    const collectHighestVisibleSeq = () => {
      if (document.visibilityState !== 'visible') return 0
      let highestVisibleSeq = 0
      for (const message of messages) {
        const node = messageRefs.current[message.id]
        if (!node) continue
        const top = node.offsetTop
        const bottom = top + node.offsetHeight
        const visibleTop = container.scrollTop
        const visibleBottom = visibleTop + container.clientHeight
        if (bottom > visibleTop && top < visibleBottom) {
          highestVisibleSeq = Math.max(highestVisibleSeq, message.seq)
        }
      }
      return highestVisibleSeq
    }

    const scheduleInitialVisibilityRead = (attempt = 0) => {
      requestAnimationFrame(() => {
        const highestVisibleSeq = collectHighestVisibleSeq()
        if (highestVisibleSeq > 0) {
          reportVisibleSeq(highestVisibleSeq)
          return
        }
        if (attempt >= 4) return
        window.setTimeout(() => scheduleInitialVisibilityRead(attempt + 1), 50)
      })
    }

    const firstUnreadMessage = messages.find((message) => message.seq > lastReadSeq)

    if (
      pendingInitialScrollTargetRef.current === target &&
      loadedTarget === target &&
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
      scheduleInitialVisibilityRead()
      pendingInitialScrollTargetRef.current = null
      return
    }

    const distFromBottom = container.scrollHeight - container.scrollTop - container.clientHeight
    if (distFromBottom < 100) {
      bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
    }
  }, [lastReadSeq, loadedTarget, loading, messages, reportVisibleSeq, target])

  useEffect(() => {
    const container = scrollContainerRef.current
    if (!container || !target || loadedTarget !== target || loading) return

    let rafId = 0
    const scheduleVisibilityRead = () => {
      cancelAnimationFrame(rafId)
      rafId = requestAnimationFrame(() => {
        if (document.visibilityState !== 'visible') return
        let highestVisibleSeq = 0
        for (const message of messages) {
          const node = messageRefs.current[message.id]
          if (!node) continue
          const top = node.offsetTop
          const bottom = top + node.offsetHeight
          const visibleTop = container.scrollTop
          const visibleBottom = visibleTop + container.clientHeight
          if (bottom > visibleTop && top < visibleBottom) {
            highestVisibleSeq = Math.max(highestVisibleSeq, message.seq)
          }
        }
        if (highestVisibleSeq > 0) {
          reportVisibleSeq(highestVisibleSeq)
        }
      })
    }

    scheduleVisibilityRead()
    container.addEventListener('scroll', scheduleVisibilityRead, { passive: true })
    window.addEventListener('resize', scheduleVisibilityRead)
    document.addEventListener('visibilitychange', scheduleVisibilityRead)
    return () => {
      cancelAnimationFrame(rafId)
      container.removeEventListener('scroll', scheduleVisibilityRead)
      window.removeEventListener('resize', scheduleVisibilityRead)
      document.removeEventListener('visibilitychange', scheduleVisibilityRead)
    }
  }, [loadedTarget, loading, messages, reportVisibleSeq, target])

  return (
    <div className="chat-panel">
      <div className="chat-messages" ref={scrollContainerRef}>
        {loading && messages.length === 0 && (
          <div className="chat-messages-empty">Loading messages...</div>
        )}
        {!loading && messages.length === 0 && target && (
          <div className="chat-messages-empty">
            No messages yet. Be the first to say something!
          </div>
        )}
        {!target && (
          <div className="chat-messages-empty">Select a channel or agent to start chatting.</div>
        )}
        {messages.map((msg, i) => (
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
              onReply={setOpenThreadMsg}
              onRetry={onRetryMessage}
            />
          </div>
        ))}
        <div ref={bottomRef} />
      </div>
    </div>
  )
}
