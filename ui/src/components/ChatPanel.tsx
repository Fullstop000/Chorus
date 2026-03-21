import { useEffect, useRef } from 'react'
import { useApp, useTarget } from '../store'
import { useHistory } from '../hooks/useHistory'
import { MessageItem } from './MessageItem'
import './ChatPanel.css'

export function ChatPanel() {
  const { currentUser, selectedChannel, selectedAgent, serverInfo, setOpenThreadMsg } = useApp()
  const target = useTarget()
  const { messages, loading } = useHistory(currentUser, target)
  const bottomRef = useRef<HTMLDivElement>(null)

  // Scroll to bottom when messages change
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [messages])

  const channelInfo = selectedChannel
    ? serverInfo?.channels.find((c) => `#${c.name}` === selectedChannel)
    : null

  const headerName = selectedChannel
    ? selectedChannel
    : selectedAgent
    ? `@${selectedAgent.display_name ?? selectedAgent.name}`
    : 'Select a channel'

  const headerDesc = channelInfo?.description ?? selectedAgent?.description ?? ''
  const headerIcon = selectedChannel ? '#' : selectedAgent ? '@' : '?'

  return (
    <div className="chat-panel">
      <div className="chat-header">
        <span className="chat-header-icon">{headerIcon}</span>
        <span className="chat-header-name">{headerName}</span>
        {headerDesc && <span className="chat-header-desc">{headerDesc}</span>}
        <div className="chat-header-actions">
          <button className="chat-header-btn">🔍</button>
          <button className="chat-header-btn">⋯</button>
        </div>
      </div>

      <div className="chat-messages">
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
          <MessageItem
            key={msg.id}
            message={msg}
            currentUser={currentUser}
            prevMessage={messages[i - 1]}
            onReply={setOpenThreadMsg}
          />
        ))}
        <div ref={bottomRef} />
      </div>
    </div>
  )
}
