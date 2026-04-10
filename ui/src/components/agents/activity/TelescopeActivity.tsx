import { useState, useEffect, useRef, useCallback } from 'react'
import type { ReactNode } from 'react'
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
  AlertCircle,
} from 'lucide-react'
import { useTraceStore } from '../../../store/traceStore'
import { getTraceEvents } from '../../../data/chat'
import type { TraceEventRecord } from '../../../data/chat'
import type { TraceFrame } from '../../../transport/types'
import './ActivityPanel.css'

interface Props {
  agentName: string
}

// ── Tool icon + label lookup ──

type ToolMeta = { icon: ReactNode; label: string }

function toolMeta(rawName: string): ToolMeta {
  const name = rawName ?? ''

  if (name.startsWith('mcp__chat__') || name.startsWith('chat__')) {
    const op = name.replace(/^(mcp__)?chat__/, '')
    const map: Record<string, ToolMeta> = {
      send_message: { icon: <MessageSquare size={13} />, label: 'Send message' },
      receive_message: { icon: <Inbox size={13} />, label: 'Receive message' },
      read_history: { icon: <History size={13} />, label: 'Read history' },
      get_history: { icon: <History size={13} />, label: 'Read history' },
      list_server: { icon: <Server size={13} />, label: 'List server' },
      get_server_info: { icon: <Server size={13} />, label: 'Server info' },
      list_tasks: { icon: <ClipboardList size={13} />, label: 'List tasks' },
      create_tasks: { icon: <ClipboardList size={13} />, label: 'Create tasks' },
      claim_tasks: { icon: <ClipboardList size={13} />, label: 'Claim tasks' },
      unclaim_task: { icon: <ClipboardList size={13} />, label: 'Unclaim task' },
      update_task_status: { icon: <CheckSquare size={13} />, label: 'Update task' },
      upload_file: { icon: <Upload size={13} />, label: 'Upload file' },
      view_file: { icon: <FileText size={13} />, label: 'View file' },
      resolve_channel: { icon: <Server size={13} />, label: 'Resolve channel' },
    }
    return map[op] ?? { icon: <Zap size={13} />, label: op }
  }

  const map: Record<string, ToolMeta> = {
    Read: { icon: <FileText size={13} />, label: 'Read file' },
    read_file: { icon: <FileText size={13} />, label: 'Read file' },
    Write: { icon: <FileOutput size={13} />, label: 'Write file' },
    write_file: { icon: <FileOutput size={13} />, label: 'Write file' },
    Edit: { icon: <FilePen size={13} />, label: 'Edit file' },
    edit_file: { icon: <FilePen size={13} />, label: 'Edit file' },
    Bash: { icon: <Terminal size={13} />, label: 'Run command' },
    bash: { icon: <Terminal size={13} />, label: 'Run command' },
    Grep: { icon: <Search size={13} />, label: 'Search code' },
    grep: { icon: <Search size={13} />, label: 'Search code' },
    Glob: { icon: <FolderSearch size={13} />, label: 'Find files' },
    glob: { icon: <FolderSearch size={13} />, label: 'Find files' },
    WebFetch: { icon: <Globe size={13} />, label: 'Fetch URL' },
    web_fetch: { icon: <Globe size={13} />, label: 'Fetch URL' },
    WebSearch: { icon: <Globe size={13} />, label: 'Web search' },
    web_search: { icon: <Globe size={13} />, label: 'Web search' },
    TodoWrite: { icon: <CheckSquare size={13} />, label: 'Update todos' },
    Task: { icon: <Zap size={13} />, label: 'Spawn agent' },
  }

  return map[name] ?? { icon: <Zap size={13} />, label: name.replace(/_/g, ' ') }
}

// ── Helpers ──

function fmtTime(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
}

// ── Expandable text ──

function ExpandableText({ text, maxLines = 3 }: { text: string; maxLines?: number }) {
  const [expanded, setExpanded] = useState(false)
  const lines = text.split('\n')
  const needsExpand = lines.length > maxLines || text.length > 300
  const display = expanded ? text : lines.slice(0, maxLines).join('\n').slice(0, 300)

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
          {expanded ? <ChevronUp size={11} /> : <ChevronDown size={11} />}
          {expanded ? 'less' : 'more'}
        </button>
      )}
    </span>
  )
}

// ── Row renderers ──

function TraceRow({ kind, data, timestampMs }: { kind: string; data: Record<string, string>; timestampMs: number }) {
  switch (kind) {
    case 'thinking':
      return (
        <div className="activity-item activity-item-thinking">
          <span className="activity-item-icon activity-icon-think">
            <BrainCircuit size={13} />
          </span>
          <div className="activity-item-main">
            <div className="activity-item-heading">
              <span className="activity-item-label">Thinking</span>
            </div>
            <div className="activity-item-body">
              <ExpandableText text={data.text ?? ''} maxLines={2} />
            </div>
          </div>
          <span className="activity-item-time">{fmtTime(timestampMs)}</span>
        </div>
      )

    case 'tool_call': {
      const meta = toolMeta(data.toolName ?? '')
      return (
        <div className="activity-item activity-item-tool">
          <span className="activity-item-icon activity-icon-tool">{meta.icon}</span>
          <div className="activity-item-main">
            <div className="activity-item-heading">
              <span className="activity-item-label">Tool</span>
              <span className="activity-item-meta">{data.toolName}</span>
            </div>
            {data.toolInput && (
              <div className="activity-item-body activity-tool-input">
                <ExpandableText text={data.toolInput} maxLines={1} />
              </div>
            )}
          </div>
          <span className="activity-item-time">{fmtTime(timestampMs)}</span>
        </div>
      )
    }

    case 'tool_result': {
      const meta = toolMeta(data.toolName ?? '')
      return (
        <div className="activity-item activity-item-tool-result">
          <span className="activity-item-icon activity-icon-tool-result">{meta.icon}</span>
          <div className="activity-item-main">
            <div className="activity-item-heading">
              <span className="activity-item-label">Result</span>
              {data.toolName && <span className="activity-item-meta">{data.toolName}</span>}
            </div>
            <div className="activity-item-body activity-item-muted">
              <ExpandableText text={data.content ?? ''} maxLines={2} />
            </div>
          </div>
          <span className="activity-item-time">{fmtTime(timestampMs)}</span>
        </div>
      )
    }

    case 'text':
      return (
        <div className="activity-item activity-item-text-entry">
          <span className="activity-item-icon activity-icon-text">
            <MessageSquare size={13} />
          </span>
          <div className="activity-item-main">
            <div className="activity-item-heading">
              <span className="activity-item-label">Output</span>
            </div>
            <div className="activity-item-body">
              <ExpandableText text={data.text ?? ''} maxLines={3} />
            </div>
          </div>
          <span className="activity-item-time">{fmtTime(timestampMs)}</span>
        </div>
      )

    case 'turn_end':
      return (
        <div className="activity-item activity-item-start">
          <span className="activity-item-icon activity-icon-start">
            <CheckSquare size={13} />
          </span>
          <div className="activity-item-main">
            <div className="activity-item-heading">
              <span className="activity-item-label">Completed</span>
            </div>
          </div>
          <span className="activity-item-time">{fmtTime(timestampMs)}</span>
        </div>
      )

    case 'error':
      return (
        <div className="activity-item" style={{ borderColor: 'var(--color-destructive)' }}>
          <span className="activity-item-icon" style={{ color: 'var(--color-destructive)', background: 'color-mix(in srgb, var(--color-destructive) 12%, transparent)' }}>
            <AlertCircle size={13} />
          </span>
          <div className="activity-item-main">
            <div className="activity-item-heading">
              <span className="activity-item-label">Error</span>
            </div>
            <div className="activity-item-body">
              <ExpandableText text={data.message ?? ''} maxLines={3} />
            </div>
          </div>
          <span className="activity-item-time">{fmtTime(timestampMs)}</span>
        </div>
      )

    default:
      return null
  }
}

// Coalesce consecutive thinking/text frames into single rows
function coalesceFrames<T extends { kind: string; data: Record<string, string> }>(frames: T[]): T[] {
  const result: T[] = []
  for (const frame of frames) {
    const last = result[result.length - 1]
    if (
      last &&
      last.kind === frame.kind &&
      (frame.kind === 'thinking' || frame.kind === 'text')
    ) {
      result[result.length - 1] = {
        ...last,
        data: { ...last.data, text: (last.data.text ?? '') + (frame.data.text ?? '') },
      }
    } else {
      result.push(frame)
    }
  }
  return result
}

// ── Main component ──

export function TelescopeActivity({ agentName }: Props) {
  const trace = useTraceStore((s) => s.traces[agentName])
  const listRef = useRef<HTMLDivElement>(null)

  // Historical events fetched from API when no live trace is available
  const [histEvents, setHistEvents] = useState<TraceEventRecord[] | null>(null)
  const [histLoading, setHistLoading] = useState(false)
  const lastFetchedRunRef = useRef<string | null>(null)

  // When trace completes and we don't have a live trace, fetch history for the last run
  const fetchHistory = useCallback((runId: string) => {
    if (lastFetchedRunRef.current === runId) return
    lastFetchedRunRef.current = runId
    setHistLoading(true)
    getTraceEvents(runId)
      .then((res) => setHistEvents(res.events))
      .catch(() => setHistEvents(null))
      .finally(() => setHistLoading(false))
  }, [])

  // If trace exists but is not active (completed run), fetch from history
  useEffect(() => {
    if (trace && !trace.isActive && trace.runId) {
      fetchHistory(trace.runId)
    }
  }, [trace, fetchHistory])

  // Auto-scroll
  useEffect(() => {
    if (listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight
    }
  }, [trace?.events.length, histEvents?.length])

  const isActive = trace?.isActive ?? false
  const isError = trace?.isError ?? false
  const statusLabel = isError ? 'Error' : isActive ? 'Active' : trace ? 'Completed' : 'Idle'
  const dotColor = isError
    ? 'var(--color-destructive)'
    : isActive
      ? 'var(--status-online)'
      : trace
        ? 'var(--status-sleeping)'
        : 'var(--status-inactive)'

  // Decide which events to render: live trace events take priority
  const hasLiveEvents = trace && trace.events.length > 0
  const liveRows = hasLiveEvents ? coalesceFrames(trace.events) : []

  // For historical, parse the data field since it comes as a JSON string from the API
  const histRows = !hasLiveEvents && histEvents
    ? coalesceFrames(histEvents.map(e => ({
        ...e,
        data: typeof e.data === 'string' ? JSON.parse(e.data) : e.data,
      })))
    : []

  const hasRows = liveRows.length > 0 || histRows.length > 0

  return (
    <div className="activity-panel">
      <div className="activity-header">
        <span className="activity-title">TELESCOPE — {agentName.toUpperCase()}</span>
        <span className="activity-status" style={{ color: dotColor }}>
          <Circle size={8} fill="currentColor" />
          <span>{statusLabel}</span>
        </span>
      </div>
      {!hasRows && !histLoading && (
        <div className="activity-empty">No trace events yet.</div>
      )}
      {histLoading && !hasRows && (
        <div className="activity-empty">Loading trace…</div>
      )}
      {hasRows && (
        <div className="activity-list" ref={listRef}>
          {liveRows.map((frame: TraceFrame) => (
            <TraceRow key={frame.seq} kind={frame.kind} data={frame.data} timestampMs={frame.timestampMs} />
          ))}
          {histRows.map((e) => (
            <TraceRow key={e.seq} kind={e.kind} data={e.data} timestampMs={e.timestampMs} />
          ))}
        </div>
      )}
    </div>
  )
}
