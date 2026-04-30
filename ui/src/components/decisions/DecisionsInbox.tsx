// Decisions Inbox — minimum v1 UI per r7.
//
// List of open decisions emitted by agents via chorus_create_decision.
// Click a card to focus it. Click an option to resolve it. Polling every
// 5s. NO keyboard shortcuts, NO auto-advance, NO confidence/reversibility
// gates, NO H2-section parsing — those are post-dogfood enhancements.
// Markdown is rendered plainly (whitespace-pre wrap); a sanitizing
// renderer can be swapped in if Day-5 dogfood reveals the agents
// emit content that needs styling.
//
// See docs/DECISIONS.md and the design doc lineage in
// chorus-design-reviews/explorations/2026-04-30-pr-review-vertical-slice/.

import { useCallback, useEffect, useState } from 'react'
import {
  type DecisionView,
  listDecisions,
  resolveDecision,
} from '../../data/decisions'
import { useStore } from '../../store/uiStore'

const POLL_INTERVAL_MS = 5000

export function DecisionsInbox() {
  const setShowDecisions = useStore((s) => s.setShowDecisions)
  const [decisions, setDecisions] = useState<DecisionView[]>([])
  const [focusedId, setFocusedId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [resolving, setResolving] = useState<string | null>(null)
  const [note, setNote] = useState('')

  const refresh = useCallback(async () => {
    try {
      const resp = await listDecisions('open')
      setDecisions(resp.decisions)
      setError(null)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [])

  useEffect(() => {
    refresh()
    const handle = window.setInterval(refresh, POLL_INTERVAL_MS)
    return () => window.clearInterval(handle)
  }, [refresh])

  // Auto-focus the first decision when none is selected and the list
  // has at least one row. Lightweight enough to be useful without
  // claiming "auto-advance" semantics.
  useEffect(() => {
    if (focusedId === null && decisions.length > 0) {
      setFocusedId(decisions[0].id)
    } else if (focusedId !== null && !decisions.some((d) => d.id === focusedId)) {
      // Currently-focused decision was removed (resolved by us or by
      // another tab). Pick the next one if any, else clear focus.
      setFocusedId(decisions[0]?.id ?? null)
    }
  }, [decisions, focusedId])

  const onPick = async (decisionId: string, pickedKey: string) => {
    setResolving(decisionId)
    try {
      await resolveDecision(decisionId, {
        picked_key: pickedKey,
        note: note.trim() || null,
      })
      setNote('')
      await refresh()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setResolving(null)
    }
  }

  const focused = decisions.find((d) => d.id === focusedId) ?? null

  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        height: '100%',
        background: 'var(--bg, #fff)',
        color: 'var(--fg, #111)',
      }}
    >
      {/* Header */}
      <div
        style={{
          padding: '12px 16px',
          borderBottom: '1px solid var(--border, #ddd)',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
        }}
      >
        <h2 style={{ margin: 0, fontSize: 16, fontWeight: 600 }}>
          Decisions{' '}
          <span style={{ opacity: 0.6, fontWeight: 400 }}>
            {decisions.length} open
          </span>
        </h2>
        <button
          onClick={() => setShowDecisions(false)}
          style={{
            background: 'none',
            border: '1px solid var(--border, #ddd)',
            padding: '4px 12px',
            cursor: 'pointer',
            fontSize: 13,
          }}
        >
          Close
        </button>
      </div>

      {error && (
        <div
          style={{
            padding: '8px 16px',
            background: '#fdd',
            color: '#900',
            fontSize: 13,
          }}
        >
          {error}
        </div>
      )}

      {/* Two-pane layout: list on left, focused on right */}
      <div style={{ flex: 1, display: 'flex', overflow: 'hidden' }}>
        {/* List */}
        <div
          style={{
            width: 320,
            borderRight: '1px solid var(--border, #ddd)',
            overflow: 'auto',
          }}
        >
          {decisions.length === 0 ? (
            <div
              style={{
                padding: '40px 16px',
                opacity: 0.6,
                fontSize: 13,
                fontFamily: 'monospace',
                textAlign: 'center',
              }}
            >
              No decisions waiting. Agents are running.
            </div>
          ) : (
            decisions.map((d) => (
              <button
                key={d.id}
                onClick={() => setFocusedId(d.id)}
                style={{
                  display: 'block',
                  width: '100%',
                  textAlign: 'left',
                  padding: '12px 16px',
                  border: 'none',
                  borderBottom: '1px solid var(--border, #eee)',
                  background:
                    d.id === focusedId
                      ? 'var(--accent-bg, #f4f4f7)'
                      : 'transparent',
                  cursor: 'pointer',
                  fontFamily: 'inherit',
                  fontSize: 13,
                }}
              >
                <div style={{ fontWeight: 600, marginBottom: 4 }}>
                  {d.payload.headline}
                </div>
                <div style={{ opacity: 0.7, fontSize: 12 }}>
                  recommended:{' '}
                  {d.payload.options.find(
                    (o) => o.key === d.payload.recommended_key,
                  )?.label ?? '?'}
                </div>
                <div
                  style={{
                    opacity: 0.5,
                    fontSize: 11,
                    marginTop: 4,
                    fontFamily: 'monospace',
                  }}
                >
                  {d.agent_id}
                </div>
              </button>
            ))
          )}
        </div>

        {/* Focused */}
        <div style={{ flex: 1, overflow: 'auto', padding: 24 }}>
          {focused ? (
            <FocusedDecision
              decision={focused}
              note={note}
              setNote={setNote}
              resolving={resolving === focused.id}
              onPick={(key) => onPick(focused.id, key)}
            />
          ) : (
            <div style={{ opacity: 0.6, fontSize: 13 }}>
              Select a decision on the left.
            </div>
          )}
        </div>
      </div>
    </div>
  )
}

function FocusedDecision({
  decision,
  note,
  setNote,
  resolving,
  onPick,
}: {
  decision: DecisionView
  note: string
  setNote: (v: string) => void
  resolving: boolean
  onPick: (key: string) => void
}) {
  const { payload } = decision
  return (
    <div>
      <div
        style={{
          fontSize: 11,
          opacity: 0.5,
          fontFamily: 'monospace',
          marginBottom: 8,
        }}
      >
        {decision.id} · {decision.agent_id} ·{' '}
        {new Date(decision.created_at).toLocaleTimeString()}
      </div>
      <h3 style={{ margin: '0 0 4px 0', fontSize: 18, fontWeight: 600 }}>
        {payload.headline}
      </h3>
      <p
        style={{
          margin: '0 0 16px 0',
          fontStyle: 'italic',
          opacity: 0.85,
        }}
      >
        {payload.question}
      </p>

      <div style={{ marginBottom: 16 }}>
        {payload.options.map((opt) => {
          const isRecommended = opt.key === payload.recommended_key
          return (
            <button
              key={opt.key}
              disabled={resolving}
              onClick={() => onPick(opt.key)}
              style={{
                display: 'block',
                width: '100%',
                textAlign: 'left',
                padding: '12px 16px',
                marginBottom: 8,
                border: isRecommended
                  ? '2px solid var(--accent, #2962ff)'
                  : '1px solid var(--border, #ccc)',
                background: 'transparent',
                cursor: resolving ? 'wait' : 'pointer',
                fontFamily: 'inherit',
                fontSize: 13,
                opacity: resolving ? 0.5 : 1,
              }}
            >
              <div style={{ fontWeight: 600, marginBottom: 4 }}>
                <span
                  style={{
                    fontFamily: 'monospace',
                    marginRight: 8,
                    opacity: 0.7,
                  }}
                >
                  [{opt.key}]
                </span>
                {opt.label}
                {isRecommended && (
                  <span
                    style={{
                      marginLeft: 8,
                      fontSize: 11,
                      color: 'var(--accent, #2962ff)',
                      fontWeight: 400,
                    }}
                  >
                    recommended
                  </span>
                )}
              </div>
              <div
                style={{
                  opacity: 0.85,
                  whiteSpace: 'pre-wrap',
                  fontSize: 12,
                }}
              >
                {opt.body}
              </div>
            </button>
          )
        })}
      </div>

      <label
        style={{
          display: 'block',
          fontSize: 12,
          opacity: 0.7,
          marginBottom: 4,
        }}
      >
        Optional note (sent to agent in the envelope)
      </label>
      <textarea
        value={note}
        onChange={(e) => setNote(e.target.value)}
        rows={2}
        disabled={resolving}
        style={{
          width: '100%',
          padding: 8,
          border: '1px solid var(--border, #ccc)',
          fontFamily: 'inherit',
          fontSize: 13,
          marginBottom: 16,
          background: 'transparent',
          color: 'inherit',
        }}
      />

      {payload.context && (
        <details>
          <summary style={{ cursor: 'pointer', opacity: 0.7, fontSize: 12 }}>
            context
          </summary>
          <pre
            style={{
              whiteSpace: 'pre-wrap',
              fontSize: 12,
              fontFamily: 'monospace',
              padding: 12,
              background: 'var(--code-bg, #f6f8fa)',
              marginTop: 8,
              borderRadius: 0,
            }}
          >
            {payload.context}
          </pre>
        </details>
      )}
    </div>
  )
}
