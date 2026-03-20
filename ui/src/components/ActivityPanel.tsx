import { useState, useEffect, useCallback } from 'react'
import { getAgentActivity } from '../api'
import type { ActivityMessage } from '../types'
import './ActivityPanel.css'

interface Props {
  agentName: string
}

export function ActivityPanel({ agentName }: Props) {
  const [messages, setMessages] = useState<ActivityMessage[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const res = await getAgentActivity(agentName, 100)
      setMessages(res.messages)
      setError(null)
    } catch (e) {
      setError(String(e))
    } finally {
      setLoading(false)
    }
  }, [agentName])

  useEffect(() => {
    setLoading(true)
    load()
    const interval = setInterval(load, 5000)
    return () => clearInterval(interval)
  }, [load])

  if (loading && messages.length === 0) {
    return (
      <div className="activity-panel">
        <div className="activity-header">
          <span className="activity-title">Activity</span>
        </div>
        <div className="activity-empty">Loading...</div>
      </div>
    )
  }

  if (error) {
    return (
      <div className="activity-panel">
        <div className="activity-header">
          <span className="activity-title">Activity</span>
        </div>
        <div className="activity-empty" style={{ color: 'var(--accent)' }}>{error}</div>
      </div>
    )
  }

  return (
    <div className="activity-panel">
      <div className="activity-header">
        <span className="activity-title">Activity — {agentName}</span>
        <span className="activity-count">{messages.length} messages sent</span>
      </div>
      {messages.length === 0 ? (
        <div className="activity-empty">No messages sent yet.</div>
      ) : (
        <div className="activity-list">
          {messages.map((msg) => (
            <div key={msg.id} className="activity-item">
              <div className="activity-item-meta">
                <span className="activity-channel">#{msg.channelName}</span>
                <span className="activity-time">{formatTime(msg.createdAt)}</span>
              </div>
              <div className="activity-content">{msg.content}</div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

function formatTime(iso: string): string {
  try {
    const d = new Date(iso.includes('T') ? iso : iso.replace(' ', 'T') + 'Z')
    return d.toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    })
  } catch {
    return iso
  }
}
