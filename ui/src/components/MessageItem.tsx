import React from 'react'
import ReactMarkdown from 'react-markdown'
import { MessageSquare, Copy, Paperclip, LoaderCircle, CircleAlert, RotateCcw } from 'lucide-react'
import type { HistoryMessage, AgentInfo } from '../types'
import { attachmentUrl } from '../api'
import { useApp } from '../store'

function replyLabel(n: number) {
  return n === 1 ? '1 reply' : `${n} replies`
}

// Render message content with markdown + @mention pills
function renderContent(content: string, agents: AgentInfo[], onSelectAgent: (agent: AgentInfo) => void) {
  return (
    <ReactMarkdown
      components={{
        // Intercept text nodes to highlight @mentions
        p({ children }) {
          return <p>{processChildren(children, agents, onSelectAgent)}</p>
        },
        li({ children }) {
          return <li>{processChildren(children, agents, onSelectAgent)}</li>
        },
      }}
    >
      {content}
    </ReactMarkdown>
  )
}

function processChildren(children: React.ReactNode, agents: AgentInfo[], onSelectAgent: (agent: AgentInfo) => void): React.ReactNode {
  if (typeof children === 'string') return injectMentions(children, agents, onSelectAgent)
  if (Array.isArray(children)) return children.map((c, i) => {
    if (typeof c === 'string') return <span key={i}>{injectMentions(c, agents, onSelectAgent)}</span>
    return c
  })
  return children
}

function MentionPill({ mention, agents, onSelectAgent }: MentionPillProps) {
  const name = mention.slice(1) // remove @
  const agent = agents.find((a) => a.name === name)
  
  if (!agent) {
    return <span className="mention-pill">{mention}</span>
  }
  
  return (
    <span 
      className="mention-pill mention-pill-clickable" 
      onClick={() => onSelectAgent(agent)}
      title={`View @${name} profile`}
    >
      {mention}
    </span>
  )
}

function injectMentions(text: string, agents: AgentInfo[], onSelectAgent: (agent: AgentInfo) => void): React.ReactNode {
  const parts = text.split(/(@[\w-]+)/g)
  if (parts.length === 1) return text
  return parts.map((part, i) =>
    part.startsWith('@') ? (
      <MentionPill key={i} mention={part} agents={agents} onSelectAgent={onSelectAgent} />
    ) : part
  )
}

function formatTime(iso: string): string {
  try {
    return new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  } catch {
    return iso
  }
}

function formatDate(iso: string): string {
  try {
    return new Date(iso).toLocaleDateString([], {
      month: 'short',
      day: 'numeric',
      year: 'numeric',
    })
  } catch {
    return iso
  }
}

function senderColor(name: string): string {
  const colors = [
    '#C0392B','#2980B9','#27AE60','#8E44AD','#D35400','#16A085','#2C3E50',
  ]
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff
  return colors[Math.abs(h) % colors.length]
}

interface MentionPillProps {
  mention: string
  agents: AgentInfo[]
  onSelectAgent: (agent: AgentInfo) => void
}

interface MessageItemProps {
  message: HistoryMessage
  currentUser: string
  prevMessage?: HistoryMessage
  onReply?: (msg: HistoryMessage) => void
  onRetry?: (msg: HistoryMessage) => void
}

export function MessageItem({ message, currentUser, prevMessage, onReply, onRetry }: MessageItemProps) {
  const { agents, setSelectedAgent, setActiveTab } = useApp()
  
  const handleSelectAgent = (agent: AgentInfo) => {
    setSelectedAgent(agent)
    setActiveTab('profile')
  }
  
  const isMe = message.senderName === currentUser
  const initial = message.senderName[0]?.toUpperCase() ?? '?'
  const color = senderColor(message.senderName)
  const deletedClass = message.senderDeleted ? ' message-deleted' : ''

  // Group messages from the same sender within 5 minutes
  const isGrouped =
    !message.clientStatus &&
    !prevMessage?.clientStatus &&
    prevMessage?.senderName === message.senderName &&
    Math.abs(
      new Date(message.createdAt).getTime() - new Date(prevMessage.createdAt).getTime()
    ) < 5 * 60 * 1000

  function handleCopy() {
    navigator.clipboard.writeText(message.content).catch(() => {})
  }

  return (
    <div className={`message-item${isGrouped ? ' grouped' : ''}${deletedClass} message-group`}>
      {!isGrouped && (
        <div
          className="message-avatar"
          style={{
            background: color,
          }}
        >
          {message.senderType === 'agent' ? (
            <span style={{ fontFamily: 'var(--font-mono)', fontSize: 11, fontWeight: 700 }}>
              {initial}
            </span>
          ) : (
            <span style={{ fontSize: 12, fontWeight: 700 }}>{initial}</span>
          )}
        </div>
      )}
      {isGrouped && <div className="message-avatar-spacer" />}
      <div className="message-body">
        {!isGrouped && (
          <div className="message-header">
            <span className="message-sender" style={{ color }}>
              {message.senderName}
              {message.senderType === 'agent' && (
                <span className="agent-badge">BOT</span>
              )}
              {message.senderDeleted && <span className="deleted-inline-badge">deleted</span>}
              {isMe && <span className="you-inline-badge">you</span>}
            </span>
            <span className="message-time">
              {formatDate(message.createdAt)} {formatTime(message.createdAt)}
            </span>
            {message.clientStatus === 'sending' && (
              <span className="message-status message-status-sending" aria-label="Sending">
                <LoaderCircle size={12} className="message-status-spin" />
                Sending
              </span>
            )}
            {message.clientStatus === 'failed' && (
              <>
                <span className="message-status message-status-failed" aria-label="Failed to send">
                  <CircleAlert size={12} />
                  Failed
                </span>
                {onRetry && (
                  <button
                    className="message-action-btn"
                    type="button"
                    aria-label="Retry send"
                    title="Retry send"
                    onClick={() => onRetry(message)}
                  >
                    <RotateCcw size={13} />
                  </button>
                )}
              </>
            )}
          </div>
        )}
        <div className="message-content">{renderContent(message.content, agents, handleSelectAgent)}</div>
        <div className="message-actions">
          {onReply && (
            <button className="message-action-btn" title="Reply in thread" onClick={() => onReply(message)}>
              <MessageSquare size={13} />
            </button>
          )}
          <button className="message-action-btn" title="Copy markdown" onClick={handleCopy}>
            <Copy size={13} />
          </button>
        </div>
        {message.attachments && message.attachments.length > 0 && (
          <div className="message-attachments">
            {message.attachments.map((att) => (
              <a
                key={att.id}
                href={attachmentUrl(att.id)}
                target="_blank"
                rel="noreferrer"
                className="attachment-link"
              >
                <Paperclip size={12} />
                {att.filename}
              </a>
            ))}
          </div>
        )}
        {onReply && (message.replyCount ?? 0) > 0 && (
          <button
            className="message-reply-count"
            onClick={() => onReply(message)}
          >
            <MessageSquare size={12} />
            {replyLabel(message.replyCount!)}
          </button>
        )}
      </div>
    </div>
  )
}
