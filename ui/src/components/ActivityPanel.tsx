import { useState, useEffect, useCallback, useRef } from 'react'
import {
  BrainCircuit,
  MessageSquare,
  FileText,
  FilePen,
  FileOutput,
  Terminal,
  Search,
  FolderSearch,
  Globe,
  Inbox,
  History,
  ClipboardList,
  CheckSquare,
  Upload,
  Server,
  Zap,
  Circle,
  ChevronDown,
  ChevronUp,
} from 'lucide-react'
import { getAgentActivityLog } from '../api'
import type { ActivityLogEntry } from '../types'
import './ActivityPanel.css'

interface Props {
  agentName: string
}

// ── Tool icon + label lookup ──────────────────────────────────────

type ToolMeta = { icon: React.ReactNode; label: string }

function toolMeta(rawName: string): ToolMeta {
  const name = rawName ?? ''

  // MCP chat tools
  if (name.startsWith('mcp__chat__') || name.startsWith('chat__')) {
    const op = name.replace(/^(mcp__)?chat__/, '')
    const map: Record<string, ToolMeta> = {
      send_message:      { icon: <MessageSquare size={13} />, label: 'Send message' },
      receive_message:   { icon: <Inbox size={13} />,         label: 'Receive message' },
      read_history:      { icon: <History size={13} />,       label: 'Read history' },
      get_history:       { icon: <History size={13} />,       label: 'Read history' },
      list_server:       { icon: <Server size={13} />,        label: 'List server' },
      get_server_info:   { icon: <Server size={13} />,        label: 'Server info' },
      list_tasks:        { icon: <ClipboardList size={13} />, label: 'List tasks' },
      create_tasks:      { icon: <ClipboardList size={13} />, label: 'Create tasks' },
      claim_tasks:       { icon: <ClipboardList size={13} />, label: 'Claim tasks' },
      unclaim_task:      { icon: <ClipboardList size={13} />, label: 'Unclaim task' },
      update_task_status:{ icon: <CheckSquare size={13} />,   label: 'Update task' },
      upload_file:       { icon: <Upload size={13} />,        label: 'Upload file' },
      view_file:         { icon: <FileText size={13} />,      label: 'View file' },
      resolve_channel:   { icon: <Server size={13} />,        label: 'Resolve channel' },
    }
    return map[op] ?? { icon: <Zap size={13} />, label: op }
  }

  // Standard Claude Code tools
  const map: Record<string, ToolMeta> = {
    Read:        { icon: <FileText size={13} />,    label: 'Read file' },
    read_file:   { icon: <FileText size={13} />,    label: 'Read file' },
    Write:       { icon: <FileOutput size={13} />,  label: 'Write file' },
    write_file:  { icon: <FileOutput size={13} />,  label: 'Write file' },
    Edit:        { icon: <FilePen size={13} />,     label: 'Edit file' },
    edit_file:   { icon: <FilePen size={13} />,     label: 'Edit file' },
    Bash:        { icon: <Terminal size={13} />,    label: 'Run command' },
    bash:        { icon: <Terminal size={13} />,    label: 'Run command' },
    Grep:        { icon: <Search size={13} />,      label: 'Search code' },
    grep:        { icon: <Search size={13} />,      label: 'Search code' },
    Glob:        { icon: <FolderSearch size={13} />,label: 'Find files' },
    glob:        { icon: <FolderSearch size={13} />,label: 'Find files' },
    WebFetch:    { icon: <Globe size={13} />,       label: 'Fetch URL' },
    web_fetch:   { icon: <Globe size={13} />,       label: 'Fetch URL' },
    WebSearch:   { icon: <Globe size={13} />,       label: 'Web search' },
    web_search:  { icon: <Globe size={13} />,       label: 'Web search' },
    TodoWrite:   { icon: <CheckSquare size={13} />, label: 'Update todos' },
    Task:        { icon: <Zap size={13} />,         label: 'Spawn agent' },
  }

  // Use tool_display_name passed from backend (already human-readable)
  return map[name] ?? { icon: <Zap size={13} />, label: name.replace(/_/g, ' ') }
}

// ── Status dot color ──────────────────────────────────────────────

function statusColor(activity: string): string {
  switch (activity) {
    case 'online':   return 'var(--lime, #84cc16)'
    case 'thinking': return 'var(--orange, #f97316)'
    case 'working':  return 'var(--orange, #f97316)'
    case 'error':    return 'var(--pink, #ec4899)'
    default:         return 'var(--gray-400, #9ca3af)'
  }
}

// ── Time formatting ───────────────────────────────────────────────

function fmtTime(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, {
    hour: '2-digit', minute: '2-digit', second: '2-digit',
  })
}

// ── Expandable text block ─────────────────────────────────────────

const COLLAPSE_LINES = 3

function ExpandableText({ text, maxLines = COLLAPSE_LINES }: { text: string; maxLines?: number }) {
  const [expanded, setExpanded] = useState(false)
  const lines = text.split('\n')
  const needsExpand = lines.length > maxLines || text.length > 300
  const display = expanded
    ? text
    : lines.slice(0, maxLines).join('\n').slice(0, 300)

  return (
    <span className="activity-expandable">
      <span className="activity-expandable-text">
        {display}
        {!expanded && needsExpand && '…'}
      </span>
      {needsExpand && (
        <button
          className="activity-expand-btn"
          onClick={() => setExpanded((x) => !x)}
          title={expanded ? 'Collapse' : 'Expand'}
        >
          {expanded
            ? <ChevronUp size={11} />
            : <ChevronDown size={11} />}
          {expanded ? 'less' : 'more'}
        </button>
      )}
    </span>
  )
}

// ── Individual entry renderers ────────────────────────────────────

function StatusRow({ item }: { item: ActivityLogEntry }) {
  const { entry, timestamp_ms } = item
  const color = statusColor(entry.activity ?? '')
  return (
    <div className="activity-item activity-item-status">
      <span className="activity-status-dot" style={{ color }}>
        <Circle size={8} fill="currentColor" />
      </span>
      <span className="activity-item-text activity-item-muted">
        {entry.activity}
        {entry.detail ? <span className="activity-status-detail"> · {entry.detail}</span> : null}
      </span>
      <span className="activity-item-time">{fmtTime(timestamp_ms)}</span>
    </div>
  )
}

function ThinkingRow({ item }: { item: ActivityLogEntry }) {
  const { entry, timestamp_ms } = item
  return (
    <div className="activity-item activity-item-thinking">
      <span className="activity-item-icon activity-icon-think">
        <BrainCircuit size={13} />
      </span>
      <span className="activity-item-text">
        <ExpandableText text={entry.text ?? ''} maxLines={2} />
      </span>
      <span className="activity-item-time">{fmtTime(timestamp_ms)}</span>
    </div>
  )
}

function ToolRow({ item }: { item: ActivityLogEntry }) {
  const { entry, timestamp_ms } = item
  // entry.tool_name is already the human-readable display name from backend
  // We look up a matching icon by trying the raw name first, fallback to display name
  const meta = toolMeta(entry.tool_name ?? '')
  const input = entry.tool_input ?? ''

  return (
    <div className="activity-item activity-item-tool">
      <span className="activity-item-icon activity-icon-tool">
        {meta.icon}
      </span>
      <span className="activity-item-text">
        <span className="activity-tool-label">
          {entry.tool_name || meta.label}
        </span>
        {input && (
          <span className="activity-tool-input">
            <ExpandableText text={input} maxLines={1} />
          </span>
        )}
      </span>
      <span className="activity-item-time">{fmtTime(timestamp_ms)}</span>
    </div>
  )
}

function TextRow({ item }: { item: ActivityLogEntry }) {
  const { entry, timestamp_ms } = item
  return (
    <div className="activity-item activity-item-text-entry">
      <span className="activity-item-icon activity-icon-text">
        <MessageSquare size={13} />
      </span>
      <span className="activity-item-text">
        <ExpandableText text={entry.text ?? ''} maxLines={3} />
      </span>
      <span className="activity-item-time">{fmtTime(timestamp_ms)}</span>
    </div>
  )
}

function ActivityRow({ item }: { item: ActivityLogEntry }) {
  switch (item.entry.kind) {
    case 'status':     return <StatusRow item={item} />
    case 'thinking':   return <ThinkingRow item={item} />
    case 'tool_start': return <ToolRow item={item} />
    case 'text':       return <TextRow item={item} />
    default:           return null
  }
}

// ── Main panel ────────────────────────────────────────────────────

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

  useEffect(() => {
    if (listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight
    }
  }, [entries])

  const dotColor = statusColor(agentActivity)

  if (loading && entries.length === 0) {
    return (
      <div className="activity-panel">
        <ActivityHeader agentName={agentName} dotColor={dotColor} activity={agentActivity} detail={agentDetail} />
        <div className="activity-empty">Loading…</div>
      </div>
    )
  }

  if (error && entries.length === 0) {
    return (
      <div className="activity-panel">
        <ActivityHeader agentName={agentName} dotColor={dotColor} activity={agentActivity} detail={agentDetail} />
        <div className="activity-empty" style={{ color: 'var(--accent)' }}>{error}</div>
      </div>
    )
  }

  return (
    <div className="activity-panel">
      <ActivityHeader agentName={agentName} dotColor={dotColor} activity={agentActivity} detail={agentDetail} />
      {entries.length === 0 ? (
        <div className="activity-empty">No activity yet.</div>
      ) : (
        <div className="activity-list" ref={listRef}>
          {entries.map((item) => (
            <ActivityRow key={item.seq} item={item} />
          ))}
        </div>
      )}
    </div>
  )
}

function ActivityHeader({
  agentName, dotColor, activity, detail,
}: {
  agentName: string; dotColor: string; activity: string; detail: string
}) {
  return (
    <div className="activity-header">
      <span className="activity-title">ACTIVITY LOG — {agentName.toUpperCase()}</span>
      <span className="activity-status" style={{ color: dotColor }}>
        <Circle size={8} fill="currentColor" />
        <span>{activity}</span>
        {detail && <span className="activity-status-detail"> · {detail}</span>}
      </span>
    </div>
  )
}
