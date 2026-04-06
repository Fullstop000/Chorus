import { useEffect, useState } from 'react'
import { MessageSquare, ArrowRight } from 'lucide-react'
import { useStore } from '../../store'
import { useInbox, useRefresh } from '../../hooks/data'
import type { ThreadInboxEntry } from '../../data'
import type { HistoryMessage } from './types'
import { ThreadPanel } from './ThreadPanel'
import './ThreadsTab.css'

function threadRowToParentMessage(entry: ThreadInboxEntry): HistoryMessage {
  return {
    id: entry.threadParentId,
    seq: entry.parentSeq,
    content: entry.parentContent,
    senderName: entry.parentSenderName,
    senderType: entry.parentSenderType,
    senderDeleted: false,
    createdAt: entry.parentCreatedAt,
    replyCount: entry.replyCount,
  }
}

export function ThreadsTab() {
  const { currentUser, currentChannel, activeTab, openThreadMsg, setOpenThreadMsg } = useStore()
  const { getConversationThreads, getConversationThreadUnread } = useInbox()
  const { refreshConversationThreads } = useRefresh()
  const [loading, setLoading] = useState(false)
  const channelId = currentChannel?.id ?? null

  useEffect(() => {
    if (!currentUser || !currentChannel || !channelId) return
    if (activeTab !== 'threads') return
    let cancelled = false
    setLoading(true)
    refreshConversationThreads(channelId)
      .finally(() => {
        if (!cancelled) {
          setLoading(false)
        }
      })
    return () => {
      cancelled = true
    }
  }, [currentUser, activeTab, refreshConversationThreads, currentChannel, channelId])

  const threadRows = getConversationThreads(channelId)
  const unreadCount = getConversationThreadUnread(channelId)

  if (!currentChannel || !channelId) {
    return (
      <div className="threads-tab threads-tab--empty">
        <div className="threads-tab__empty-copy">Select a channel to browse threads.</div>
      </div>
    )
  }

  return (
    <div className="threads-tab">
      <div className="threads-tab__list">
        <div className="threads-tab__list-header">
          <div>
            <div className="threads-tab__kicker">Threads · Latest Activity</div>
            <div className="threads-tab__title">
              {threadRows.length > 0
                ? `${threadRows.length} active thread${threadRows.length === 1 ? '' : 's'}`
                : 'No active threads'}
            </div>
            {unreadCount > 0 && (
              <div className="threads-tab__subtitle">
                {unreadCount} unread repl{unreadCount === 1 ? 'y' : 'ies'}
              </div>
            )}
          </div>
        </div>

        {loading && threadRows.length === 0 && (
          <div className="threads-tab__empty-copy">Loading channel threads…</div>
        )}
        {!loading && threadRows.length === 0 && (
          <div className="threads-tab__empty-copy">
            No active threads in this channel yet.
          </div>
        )}

        {threadRows.length > 0 && (
          <div className="threads-tab__rows">
            {threadRows.map((thread) => {
              const isSelected = openThreadMsg?.id === thread.threadParentId
              return (
                <button
                  key={thread.threadParentId}
                  type="button"
                  className={`threads-tab__row${isSelected ? ' is-selected' : ''}`}
                  onClick={() => setOpenThreadMsg(threadRowToParentMessage(thread))}
                >
                  <div className="threads-tab__row-top">
                    <div className="threads-tab__row-heading">
                      <span className="threads-tab__parent-sender">{thread.parentSenderName}</span>
                      <span className="threads-tab__conversation-id">{thread.conversationId}</span>
                    </div>
                    <span className="threads-tab__last-at">
                      {thread.lastReplyAt
                        ? new Date(thread.lastReplyAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
                        : ''}
                    </span>
                  </div>
                  <div className="threads-tab__preview">{thread.parentContent}</div>
                  <div className="threads-tab__row-bottom">
                    <span className="threads-tab__meta">
                      <MessageSquare size={12} />
                      {thread.replyCount} repl{thread.replyCount === 1 ? 'y' : 'ies'}
                    </span>
                    <span className="threads-tab__meta">
                      {thread.participantCount} participant{thread.participantCount === 1 ? '' : 's'}
                    </span>
                    {thread.unreadCount > 0 && (
                      <span className="threads-tab__unread">
                        {thread.unreadCount} unread
                      </span>
                    )}
                    <span className="threads-tab__open">
                      Open
                      <ArrowRight size={12} />
                    </span>
                  </div>
                </button>
              )
            })}
          </div>
        )}
      </div>

      <div className="threads-tab__reader">
        {openThreadMsg ? (
          <ThreadPanel variant="content" />
        ) : (
          <div className="threads-tab__reader-empty">
            Select a thread to read replies.
          </div>
        )}
      </div>
    </div>
  )
}
