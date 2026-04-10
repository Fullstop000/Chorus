import { useState, useEffect, useMemo, useCallback } from 'react'
import { GitCompare, Loader2 } from 'lucide-react'
import { getAgentRuns, getTraceEvents } from '../../../data'
import type { AgentRunInfo, TraceEventRecord } from '../../../data'
import { classifyTool } from '../../../lib/toolCategories'
import './RunComparison.css'

interface Props {
  agentName: string
}

// ── Matching algorithm ──
// Align events by (tool_name, ordinal_within_tool_name).
// Only tool_call events are compared.

interface MatchedRow {
  left?: TraceEventRecord
  right?: TraceEventRecord
  toolName: string
  ordinal: number
}

function extractToolCalls(events: TraceEventRecord[]): { toolName: string; event: TraceEventRecord }[] {
  return events
    .filter(e => e.kind === 'tool_call')
    .map(e => {
      const data = typeof e.data === 'string' ? JSON.parse(e.data) : e.data
      return { toolName: data.toolName ?? 'unknown', event: e }
    })
}

function matchRuns(leftEvents: TraceEventRecord[], rightEvents: TraceEventRecord[]): MatchedRow[] {
  const leftTools = extractToolCalls(leftEvents)
  const rightTools = extractToolCalls(rightEvents)

  // Group by tool name with per-tool ordinal
  const leftByTool = new Map<string, { ordinal: number; event: TraceEventRecord }[]>()
  const rightByTool = new Map<string, { ordinal: number; event: TraceEventRecord }[]>()

  for (const t of leftTools) {
    const arr = leftByTool.get(t.toolName) ?? []
    arr.push({ ordinal: arr.length, event: t.event })
    leftByTool.set(t.toolName, arr)
  }
  for (const t of rightTools) {
    const arr = rightByTool.get(t.toolName) ?? []
    arr.push({ ordinal: arr.length, event: t.event })
    rightByTool.set(t.toolName, arr)
  }

  // Collect all tool names
  const allTools = new Set([...leftByTool.keys(), ...rightByTool.keys()])
  const rows: MatchedRow[] = []

  for (const toolName of allTools) {
    const lefts = leftByTool.get(toolName) ?? []
    const rights = rightByTool.get(toolName) ?? []
    const maxLen = Math.max(lefts.length, rights.length)
    for (let i = 0; i < maxLen; i++) {
      rows.push({
        left: lefts[i]?.event,
        right: rights[i]?.event,
        toolName,
        ordinal: i,
      })
    }
  }

  return rows
}

function getDuration(event: TraceEventRecord, allEvents: TraceEventRecord[]): number | null {
  // Find matching tool_result for this tool_call
  const data = typeof event.data === 'string' ? JSON.parse(event.data) : event.data
  const toolName = data.toolName
  const idx = allEvents.indexOf(event)
  for (let i = idx + 1; i < allEvents.length; i++) {
    const e = allEvents[i]
    if (e.kind === 'tool_result') {
      const rd = typeof e.data === 'string' ? JSON.parse(e.data) : e.data
      if (rd.toolName === toolName) {
        return e.timestampMs - event.timestampMs
      }
    }
  }
  return null
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`
  return `${(ms / 1000).toFixed(1)}s`
}

// ── Run selection UI ──

function RunSelector({
  runs,
  selected,
  onToggle,
  onCompare,
  loading,
}: {
  runs: AgentRunInfo[]
  selected: Set<string>
  onToggle: (runId: string) => void
  onCompare: () => void
  loading: boolean
}) {
  return (
    <div className="rc-selector">
      <div className="rc-selector-header">
        <span className="rc-selector-title">Select 2 runs to compare</span>
        <button
          className="rc-compare-btn"
          disabled={selected.size !== 2}
          onClick={onCompare}
        >
          <GitCompare size={13} />
          Compare
        </button>
      </div>
      {loading && <div className="rc-loading"><Loader2 size={14} className="rc-spinner" /> Loading runs…</div>}
      {!loading && runs.length === 0 && (
        <div className="rc-empty">No completed runs with trace data.</div>
      )}
      <div className="rc-run-list">
        {runs.map(run => {
          const checked = selected.has(run.runId)
          const ts = run.traceSummary
          const dur = ts.duration > 0 ? `${Math.round(ts.duration / 1000)}s` : '—'
          return (
            <label key={run.runId} className={`rc-run-item${checked ? ' selected' : ''}`}>
              <input
                type="checkbox"
                checked={checked}
                disabled={!checked && selected.size >= 2}
                onChange={() => onToggle(run.runId)}
              />
              <span className="rc-run-time">
                {new Date(run.createdAt).toLocaleString([], {
                  month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit'
                })}
              </span>
              <span className={`rc-run-status rc-status-${ts.status}`}>{ts.status}</span>
              <span className="rc-run-tools">{ts.toolCalls} tools</span>
              <span className="rc-run-dur">{dur}</span>
            </label>
          )
        })}
      </div>
    </div>
  )
}

// ── Comparison view ──

function ComparisonView({
  leftRun,
  rightRun,
  leftEvents,
  rightEvents,
  onBack,
}: {
  leftRun: AgentRunInfo
  rightRun: AgentRunInfo
  leftEvents: TraceEventRecord[]
  rightEvents: TraceEventRecord[]
  onBack: () => void
}) {
  const matched = useMemo(() => matchRuns(leftEvents, rightEvents), [leftEvents, rightEvents])

  return (
    <div className="rc-comparison">
      <div className="rc-comparison-header">
        <button className="rc-back-btn" onClick={onBack}>← Back</button>
        <span className="rc-comparison-title">Run Comparison</span>
      </div>
      <div className="rc-columns-header">
        <div className="rc-col-label">
          {new Date(leftRun.createdAt).toLocaleString([], { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })}
        </div>
        <div className="rc-col-diff-label">Diff</div>
        <div className="rc-col-label">
          {new Date(rightRun.createdAt).toLocaleString([], { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })}
        </div>
      </div>
      <div className="rc-rows">
        {matched.map((row, i) => {
          const leftDur = row.left ? getDuration(row.left, leftEvents) : null
          const rightDur = row.right ? getDuration(row.right, rightEvents) : null
          const diffMs = leftDur != null && rightDur != null ? rightDur - leftDur : null
          const { icon: Icon } = classifyTool(row.toolName)

          return (
            <div key={i} className="rc-row">
              <div className={`rc-cell${row.left ? '' : ' rc-cell-empty'}`}>
                {row.left && (
                  <>
                    {Icon && <Icon size={12} className="rc-cell-icon" />}
                    <span className="rc-cell-tool">{row.toolName}</span>
                    {leftDur != null && <span className="rc-cell-dur">{formatDuration(leftDur)}</span>}
                  </>
                )}
                {!row.left && <span className="rc-cell-added">removed</span>}
              </div>
              <div className="rc-diff">
                {diffMs != null && (
                  <span className={diffMs > 0 ? 'rc-diff-slower' : diffMs < 0 ? 'rc-diff-faster' : 'rc-diff-same'}>
                    {diffMs > 0 ? '+' : ''}{formatDuration(Math.abs(diffMs))}
                  </span>
                )}
                {row.left && !row.right && <span className="rc-diff-removed">−</span>}
                {!row.left && row.right && <span className="rc-diff-added">+</span>}
              </div>
              <div className={`rc-cell${row.right ? '' : ' rc-cell-empty'}`}>
                {row.right && (
                  <>
                    {Icon && <Icon size={12} className="rc-cell-icon" />}
                    <span className="rc-cell-tool">{row.toolName}</span>
                    {rightDur != null && <span className="rc-cell-dur">{formatDuration(rightDur)}</span>}
                  </>
                )}
                {!row.right && <span className="rc-cell-added">added</span>}
              </div>
            </div>
          )
        })}
      </div>
      <div className="rc-summary-bar">
        <span>Left: {leftRun.traceSummary.toolCalls} tools, {Math.round(leftRun.traceSummary.duration / 1000)}s</span>
        <span>Right: {rightRun.traceSummary.toolCalls} tools, {Math.round(rightRun.traceSummary.duration / 1000)}s</span>
      </div>
    </div>
  )
}

// ── Main component ──

export function RunComparison({ agentName }: Props) {
  const [runs, setRuns] = useState<AgentRunInfo[]>([])
  const [runsLoading, setRunsLoading] = useState(true)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [comparing, setComparing] = useState(false)
  const [leftEvents, setLeftEvents] = useState<TraceEventRecord[]>([])
  const [rightEvents, setRightEvents] = useState<TraceEventRecord[]>([])
  const [eventsLoading, setEventsLoading] = useState(false)

  useEffect(() => {
    setRunsLoading(true)
    setSelected(new Set())
    setComparing(false)
    getAgentRuns(agentName)
      .then(res => setRuns(res.runs))
      .catch(() => setRuns([]))
      .finally(() => setRunsLoading(false))
  }, [agentName])

  const handleToggle = useCallback((runId: string) => {
    setSelected(prev => {
      const next = new Set(prev)
      if (next.has(runId)) next.delete(runId)
      else if (next.size < 2) next.add(runId)
      return next
    })
  }, [])

  const handleCompare = useCallback(() => {
    const ids = Array.from(selected)
    if (ids.length !== 2) return
    setEventsLoading(true)
    Promise.all([getTraceEvents(ids[0]), getTraceEvents(ids[1])])
      .then(([a, b]) => {
        setLeftEvents(a.events)
        setRightEvents(b.events)
        setComparing(true)
      })
      .catch(() => {})
      .finally(() => setEventsLoading(false))
  }, [selected])

  const selectedArr = Array.from(selected)
  const leftRun = runs.find(r => r.runId === selectedArr[0])
  const rightRun = runs.find(r => r.runId === selectedArr[1])

  if (eventsLoading) {
    return (
      <div className="rc-container">
        <div className="rc-loading"><Loader2 size={14} className="rc-spinner" /> Loading trace events…</div>
      </div>
    )
  }

  if (comparing && leftRun && rightRun) {
    return (
      <div className="rc-container">
        <ComparisonView
          leftRun={leftRun}
          rightRun={rightRun}
          leftEvents={leftEvents}
          rightEvents={rightEvents}
          onBack={() => setComparing(false)}
        />
      </div>
    )
  }

  return (
    <div className="rc-container">
      <RunSelector
        runs={runs}
        selected={selected}
        onToggle={handleToggle}
        onCompare={handleCompare}
        loading={runsLoading}
      />
    </div>
  )
}
