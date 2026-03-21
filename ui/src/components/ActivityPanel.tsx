import { useState, useEffect, useCallback, useRef } from 'react'
import { getAgentActivityLog } from '../api'
import type { ActivityLogEntry } from '../types'
import './ActivityPanel.css'

interface Props {
  agentName: string
}

// Map raw tool names to human-readable labels (mirrors server rt() function)
function toolLabel(name: string): string {
  if (name.startsWith('mcp__chat__')) {
    const op = name.replace('mcp__chat__', '')
    const labels: Record<string, string> = {
      send_message: 'Send message',
      receive_message: 'Receive message',
      get_history: 'Read history',
      get_server_info: 'Get server info',
      list_tasks: 'List tasks',
      create_tasks: 'Create tasks',
      claim_tasks: 'Claim tasks',
      unclaim_task: 'Unclaim task',
      update_task_status: 'Update task',
    }
    return labels[op] ?? op
  }
  const map: Record<string, string> = {
    Read: 'Read file',
    Write: 'Write file',
    Edit: 'Edit file',
    Bash: 'Run command',
    Glob: 'Find files',
    Grep: 'Search files',
    WebFetch: 'Fetch URL',
    WebSearch: 'Web search',
    TodoWrite: 'Update todos',
    Task: 'Spawn agent',
  }
  return map[name] ?? name
}

function entryIcon(kind: string): string {
  switch (kind) {
    case 'thinking': return '💭'
    case 'tool_start': return '⚙'
    case 'text': return '💬'
    case 'status': return '●'
    default: return '·'
  }
}

function activityColor(activity: string): string {
  switch (activity) {
    case 'online': return 'var(--lime)'
    case 'thinking': return 'var(--orange)'
    case 'working': return 'var(--orange)'
    case 'offline': return 'var(--gray-400)'
    case 'error': return 'var(--pink)'
    default: return 'var(--gray-400)'
  }
}

export function ActivityPanel({ agentName }: Props) {
  const [entries, setEntries] = useState<ActivityLogEntry[]>([])
  const [agentActivity, setAgentActivity] = useState('offline')
  const [agentDetail, setAgentDetail] = useState('')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const lastSeqRef = useRef<number | undefined>(undefined)
  const listRef = useRef<HTMLDivElement>(null)

  const load = useCallback(async () => {
    try {
      const res = await getAgentActivityLog(agentName, lastSeqRef.current)
      if (res.entries.length > 0) {
        setEntries((prev) => {
          const combined = [...prev, ...res.entries]
          // Keep last 500
          return combined.slice(-500)
        })
        lastSeqRef.current = res.entries[res.entries.length - 1].seq
      }
      setAgentActivity(res.agent_activity)
      setAgentDetail(res.agent_detail)
      setError(null)
    } catch (e) {
      setError(String(e))
    } finally {
      setLoading(false)
    }
  }, [agentName])

  // Reset when agent changes
  useEffect(() => {
    setEntries([])
    setAgentActivity('offline')
    setAgentDetail('')
    setLoading(true)
    lastSeqRef.current = undefined
    load()
    const interval = setInterval(load, 2000)
    return () => clearInterval(interval)
  }, [load])

  // Auto-scroll to bottom
  useEffect(() => {
    if (listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight
    }
  }, [entries])

  const statusColor = activityColor(agentActivity)

  if (loading && entries.length === 0) {
    return (
      <div className="activity-panel">
        <div className="activity-header">
          <span className="activity-title">Activity Log</span>
        </div>
        <div className="activity-empty">Loading…</div>
      </div>
    )
  }

  if (error && entries.length === 0) {
    return (
      <div className="activity-panel">
        <div className="activity-header">
          <span className="activity-title">Activity Log</span>
        </div>
        <div className="activity-empty" style={{ color: 'var(--accent)' }}>{error}</div>
      </div>
    )
  }

  return (
    <div className="activity-panel">
      <div className="activity-header">
        <span className="activity-title">Activity Log — {agentName}</span>
        <span className="activity-status" style={{ color: statusColor }}>
          ● {agentActivity}{agentDetail ? ` · ${agentDetail}` : ''}
        </span>
      </div>
      {entries.length === 0 ? (
        <div className="activity-empty">No activity yet.</div>
      ) : (
        <div className="activity-list" ref={listRef}>
          {entries.map((item) => (
            <ActivityEntry key={item.seq} item={item} />
          ))}
        </div>
      )}
    </div>
  )
}

function ActivityEntry({ item }: { item: ActivityLogEntry }) {
  const { entry, timestamp_ms } = item
  const time = new Date(timestamp_ms).toLocaleTimeString(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })

  if (entry.kind === 'status') {
    return (
      <div className="activity-item activity-item-status">
        <span className="activity-item-icon" style={{ color: activityColor(entry.activity ?? '') }}>
          {entryIcon('status')}
        </span>
        <span className="activity-item-text activity-item-muted">
          {entry.activity}{entry.detail ? `: ${entry.detail}` : ''}
        </span>
        <span className="activity-item-time">{time}</span>
      </div>
    )
  }

  if (entry.kind === 'thinking') {
    return (
      <div className="activity-item activity-item-thinking">
        <span className="activity-item-icon">💭</span>
        <span className="activity-item-text activity-item-thinking-text">
          {entry.text}
        </span>
        <span className="activity-item-time">{time}</span>
      </div>
    )
  }

  if (entry.kind === 'tool_start') {
    return (
      <div className="activity-item activity-item-tool">
        <span className="activity-item-icon">⚙</span>
        <span className="activity-item-text">
          <strong>{toolLabel(entry.tool_name ?? '')}</strong>
          {entry.tool_input && (
            <span className="activity-tool-input"> {entry.tool_input.slice(0, 80)}{entry.tool_input.length > 80 ? '…' : ''}</span>
          )}
        </span>
        <span className="activity-item-time">{time}</span>
      </div>
    )
  }

  if (entry.kind === 'text') {
    return (
      <div className="activity-item activity-item-text-entry">
        <span className="activity-item-icon">💬</span>
        <span className="activity-item-text">
          {(entry.text ?? '').slice(0, 200)}{(entry.text ?? '').length > 200 ? '…' : ''}
        </span>
        <span className="activity-item-time">{time}</span>
      </div>
    )
  }

  return null
}
