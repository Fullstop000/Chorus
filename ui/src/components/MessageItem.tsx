import React from 'react'
import ReactMarkdown from 'react-markdown'
import type { HistoryMessage } from '../types'
import { attachmentUrl } from '../api'

// Render message content with markdown + @mention pills
function renderContent(content: string) {
  return (
    <ReactMarkdown
      components={{
        // Intercept text nodes to highlight @mentions
        p({ children }) {
          return <p>{processChildren(children)}</p>
        },
        li({ children }) {
          return <li>{processChildren(children)}</li>
        },
      }}
    >
      {content}
    </ReactMarkdown>
  )
}

function processChildren(children: React.ReactNode): React.ReactNode {
  if (typeof children === 'string') return injectMentions(children)
  if (Array.isArray(children)) return children.map((c, i) => {
    if (typeof c === 'string') return <span key={i}>{injectMentions(c)}</span>
    return c
  })
  return children
}

function injectMentions(text: string): React.ReactNode {
  const parts = text.split(/(@\w+)/g)
  if (parts.length === 1) return text
  return parts.map((part, i) =>
    part.startsWith('@') ? (
      <span key={i} className="mention-pill">{part}</span>
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

interface MessageItemProps {
  message: HistoryMessage
  currentUser: string
  prevMessage?: HistoryMessage
}

export function MessageItem({ message, currentUser, prevMessage }: MessageItemProps) {
  const isMe = message.senderName === currentUser
  const initial = message.senderName[0]?.toUpperCase() ?? '?'
  const color = senderColor(message.senderName)

  // Group messages from the same sender within 5 minutes
  const isGrouped =
    prevMessage?.senderName === message.senderName &&
    Math.abs(
      new Date(message.createdAt).getTime() - new Date(prevMessage.createdAt).getTime()
    ) < 5 * 60 * 1000

  return (
    <div className={`message-item${isGrouped ? ' grouped' : ''}`}>
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
              {isMe && <span className="you-inline-badge">you</span>}
            </span>
            <span className="message-time">
              {formatDate(message.createdAt)} {formatTime(message.createdAt)}
            </span>
          </div>
        )}
        <div className="message-content">{renderContent(message.content)}</div>
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
                📎 {att.filename}
              </a>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
