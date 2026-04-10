import { useState, useMemo } from 'react'
import { Activity, ChevronDown, ChevronUp } from 'lucide-react'
import { useTraceStore } from '../../store/traceStore'
import { classifyTool } from '../../lib/toolCategories'
import type { TraceFrame } from '../../transport/types'
import './CrossAgentTimeline.css'

// ── Agent color palette ──

const AGENT_COLORS = [
  '#C0392B', '#2980B9', '#27AE60', '#8E44AD',
  '#D35400', '#16A085', '#2C3E50', '#7D3C98',
]

function agentColor(name: string): string {
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff
  return AGENT_COLORS[Math.abs(h) % AGENT_COLORS.length]
}

// ── Helpers ──

function fmtTime(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  })
}

function truncate(s: string, max: number): string {
  return s.length <= max ? s : s.slice(0, max) + '…'
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
    case 'tool_call': return truncate(data.toolInput ?? '', 60)
    case 'tool_result': return truncate(data.content ?? '', 60)
    case 'error': return data.message ?? ''
    default: return ''
  }
}

// ── Timeline row ──

function TimelineRow({ frame, color }: { frame: TraceFrame; color: string }) {
  const { icon: Icon } = (frame.kind === 'tool_call' || frame.kind === 'tool_result')
    ? classifyTool(frame.data.toolName ?? '')
    : { icon: null }

  return (
    <div className="cat-row">
      <span className="cat-agent-dot" style={{ background: color }} />
      <span className="cat-agent-name" style={{ color }}>{frame.agentName}</span>
      {Icon && <Icon size={12} className="cat-row-icon" />}
      <span className="cat-row-label">{rowLabel(frame.kind, frame.data)}</span>
      <span className="cat-row-detail">{rowDetail(frame.kind, frame.data)}</span>
      <span className="cat-row-time">{fmtTime(frame.timestampMs)}</span>
    </div>
  )
}

// ── Summary bar when all complete ──

function CompletionSummary({ traces }: { traces: Record<string, { runId: string; events: TraceFrame[]; isActive: boolean }> }) {
  const agents = Object.entries(traces)
  const parts = agents.map(([name, trace]) => {
    const events = trace.events
    if (events.length < 2) return `${name}: 0s`
    const dur = Math.round((events[events.length - 1].timestampMs - events[0].timestampMs) / 1000)
    return `${name}: ${dur}s`
  })
  return (
    <div className="cat-summary">
      {agents.length} agent{agents.length !== 1 ? 's' : ''} processed ({parts.join(', ')})
    </div>
  )
}

// ── Main component ──

export function CrossAgentTimeline() {
  const traces = useTraceStore((s) => s.traces)
  const [expanded, setExpanded] = useState(true)

  const agentNames = Object.keys(traces)
  const hasTraces = agentNames.length > 0
  const anyActive = agentNames.some(n => traces[n].isActive)
  const allComplete = hasTraces && !anyActive

  // Interleave all events by timestamp, excluding thinking events in collapsed view
  const interleaved = useMemo(() => {
    const all: TraceFrame[] = []
    for (const name of agentNames) {
      for (const event of traces[name].events) {
        // Skip thinking events in the timeline (density management)
        if (event.kind === 'thinking') continue
        all.push(event)
      }
    }
    return all.sort((a, b) => a.timestampMs - b.timestampMs || a.seq - b.seq)
  }, [traces, agentNames])

  if (!hasTraces) {
    return (
      <div className="cat-bar cat-bar-empty">
        <Activity size={13} />
        <span>No agents processing</span>
      </div>
    )
  }

  if (allComplete && !expanded) {
    return (
      <div className="cat-bar cat-bar-summary" onClick={() => setExpanded(true)}>
        <CompletionSummary traces={traces} />
        <ChevronDown size={13} className="cat-chevron" />
      </div>
    )
  }

  return (
    <div className="cat-container">
      <div className="cat-bar" onClick={() => setExpanded(!expanded)}>
        <Activity size={13} />
        <span className="cat-title">All Agent Activity</span>
        <span className="cat-count">{agentNames.length} agent{agentNames.length !== 1 ? 's' : ''}</span>
        {expanded
          ? <ChevronUp size={13} className="cat-chevron" />
          : <ChevronDown size={13} className="cat-chevron" />
        }
      </div>
      {expanded && (
        <div className="cat-rows">
          {interleaved.map(frame => (
            <TimelineRow
              key={`${frame.agentName}-${frame.seq}`}
              frame={frame}
              color={agentColor(frame.agentName)}
            />
          ))}
          {allComplete && <CompletionSummary traces={traces} />}
        </div>
      )}
    </div>
  )
}
