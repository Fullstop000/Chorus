import { useRef, useEffect, useState, useCallback } from 'react'
import { ChevronDown } from 'lucide-react'
import { classifyTool } from '../../lib/toolCategories'
import { getTraceEvents } from '../../data/chat'
import { useTraceStore } from '../../store/traceStore'
import type { TraceSummary, TraceEventRecord } from '../../data/chat'
import './Telescope.css'

// ── Trace event types (canonical source: transport/types.ts) ──

export interface TraceEvent {
  runId: string
  agentName: string
  seq: number
  timestampMs: number
  kind: string
  data: Record<string, string>
}

// ── Props ──

interface TelescopeProps {
  agentName: string
  runId: string
  events: TraceEvent[]
  isActive: boolean
  isError: boolean
  onToggleExpand?: () => void
  isExpanded?: boolean
  traceSummary?: TraceSummary
}

// ── Helpers ──

function relativeTime(timestampMs: number): string {
  const delta = Date.now() - timestampMs
  if (delta < 1000) return 'now'
  if (delta < 60_000) return `${Math.floor(delta / 1000)}s`
  return `${Math.floor(delta / 60_000)}m`
}

function rowLabel(kind: string, data: Record<string, string>): string {
  switch (kind) {
    case 'tool_call': return data.toolName ?? 'tool'
    case 'tool_result': return `${data.toolName ?? 'tool'} ✓`
    case 'thinking': return 'thinking'
    case 'text': return 'response'
    case 'turn_end': return 'done'
    case 'error': return 'error'
    default: return kind
  }
}

function rowDetail(kind: string, data: Record<string, string>): string {
  switch (kind) {
    case 'tool_call': return truncate(data.toolInput ?? '', 80)
    case 'tool_result': return truncate(data.content ?? '', 80)
    case 'thinking': return truncate(data.text ?? '', 80)
    case 'text': return truncate(data.text ?? '', 80)
    case 'error': return data.message ?? ''
    default: return ''
  }
}

function truncate(s: string, max: number): string {
  if (s.length <= max) return s
  return s.slice(0, max) + '…'
}

function summaryText(events: TraceEvent[]): string {
  const toolCalls = events.filter(e => e.kind === 'tool_call').length
  if (toolCalls === 0) return 'no tool calls'
  return toolCalls === 1 ? '1 tool call' : `${toolCalls} tool calls`
}

function historySummaryText(ts: TraceSummary): string {
  const n = ts.toolCalls
  const label = n === 0 ? 'no tool calls' : n === 1 ? '1 tool call' : `${n} tool calls`
  if (ts.duration > 0) {
    const sec = Math.round(ts.duration / 1000)
    return sec > 0 ? `${label} · ${sec}s` : label
  }
  return label
}

// ── Row ──

function TelescopeRow({ event }: { event: TraceEvent }) {
  const { icon: Icon } = event.kind === 'tool_call' || event.kind === 'tool_result'
    ? classifyTool(event.data.toolName ?? '')
    : { icon: null }

  return (
    <div className="tele-row">
      {Icon && <Icon size={13} className="tele-row-icon" />}
      {!Icon && <span className="tele-row-icon" style={{ width: 13 }} />}
      <span className="tele-row-label">{rowLabel(event.kind, event.data)}</span>
      <span className="tele-row-detail">{rowDetail(event.kind, event.data)}</span>
      <span className="tele-row-time">{relativeTime(event.timestampMs)}</span>
    </div>
  )
}

function HistoryRow({ event }: { event: TraceEventRecord }) {
  const data = typeof event.data === 'string' ? JSON.parse(event.data) : event.data
  const { icon: Icon } = event.kind === 'tool_call' || event.kind === 'tool_result'
    ? classifyTool(data.toolName ?? '')
    : { icon: null }

  return (
    <div className="tele-row">
      {Icon && <Icon size={13} className="tele-row-icon" />}
      {!Icon && <span className="tele-row-icon" style={{ width: 13 }} />}
      <span className="tele-row-label">{rowLabel(event.kind, data)}</span>
      <span className="tele-row-detail">{rowDetail(event.kind, data)}</span>
      <span className="tele-row-time">{relativeTime(event.timestampMs)}</span>
    </div>
  )
}

// ── Main component ──

export function Telescope({
  agentName,
  runId,
  events,
  isActive,
  isError,
  onToggleExpand,
  isExpanded = true,
  traceSummary,
}: TelescopeProps) {
  const rowsRef = useRef<HTMLDivElement>(null)
  const isHistorical = !!traceSummary && events.length === 0
  const [histExpanded, setHistExpanded] = useState(false)
  const [histEvents, setHistEvents] = useState<TraceEventRecord[] | null>(null)
  const [histLoading, setHistLoading] = useState(false)
  const isFlashing = useTraceStore((s) => s.completionFlash[agentName] ?? false)

  // Auto-scroll to bottom when new events arrive while active
  useEffect(() => {
    if (isExpanded && isActive && rowsRef.current) {
      rowsRef.current.scrollTop = rowsRef.current.scrollHeight
    }
  }, [events.length, isExpanded, isActive])

  const handleHistToggle = useCallback(() => {
    const next = !histExpanded
    setHistExpanded(next)
    if (next && histEvents === null && !histLoading) {
      setHistLoading(true)
      getTraceEvents(runId)
        .then(res => setHistEvents(res.events))
        .catch(() => setHistEvents([]))
        .finally(() => setHistLoading(false))
    }
  }, [histExpanded, histEvents, histLoading, runId])

  // ── Historical mode ──
  if (isHistorical) {
    const dotClass = isError ? 'tele-dot error' : 'tele-dot'
    return (
      <div className={`telescope${isError ? ' error' : ''}`}>
        <div className="tele-header" onClick={handleHistToggle}>
          <span className={dotClass} />
          <span className="tele-agent-name">{agentName}</span>
          <span className="tele-summary">{historySummaryText(traceSummary)}</span>
          <ChevronDown
            size={13}
            className={`tele-chevron${histExpanded ? ' expanded' : ''}`}
          />
        </div>
        {histExpanded && (
          <div className="tele-rows" ref={rowsRef}>
            {histLoading && <div className="tele-row"><span className="tele-row-detail">Loading…</span></div>}
            {histEvents?.map(e => <HistoryRow key={e.seq} event={e} />)}
          </div>
        )}
      </div>
    )
  }

  // ── Live mode ──
  if (events.length === 0 && isActive) {
    return (
      <div className="telescope">
        <div className="tele-loading">
          <span className="tele-dot active" />
          <span>{agentName} connecting…</span>
        </div>
      </div>
    )
  }

  if (events.length === 0) return null

  const dotClass = isError ? 'tele-dot error' : isActive ? 'tele-dot active' : 'tele-dot'
  const wrapperClass = `telescope${isError ? ' error' : ''}${isFlashing ? ' completion-flash' : ''}`

  return (
    <div className={wrapperClass}>
      <div className="tele-header" onClick={onToggleExpand}>
        <span className={dotClass} />
        <span className="tele-agent-name">{agentName}</span>
        <span className="tele-summary">{summaryText(events)}</span>
        <ChevronDown
          size={13}
          className={`tele-chevron${isExpanded ? ' expanded' : ''}`}
        />
      </div>
      {isExpanded && (
        <div className="tele-rows" ref={rowsRef}>
          {events.map(e => (
            <TelescopeRow key={e.seq} event={e} />
          ))}
        </div>
      )}
    </div>
  )
}
