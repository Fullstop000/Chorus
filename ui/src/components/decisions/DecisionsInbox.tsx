import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  type DecisionStatusFilter,
  type DecisionView,
  listDecisions,
  resolveDecision,
} from '../../data/decisions'

const POLL_INTERVAL_MS = 5_000

export function DecisionsInbox(): JSX.Element {
  const [filter, setFilter] = useState<DecisionStatusFilter>('open')
  const [decisions, setDecisions] = useState<DecisionView[]>([])
  const [focused, setFocused] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [picking, setPicking] = useState<string | null>(null)
  const [note, setNote] = useState('')

  const refresh = useCallback(async () => {
    try {
      const r = await listDecisions(filter)
      setDecisions(r.decisions)
      setError(null)
      // If the focused decision dropped off the list (e.g., resolved by
      // someone else), clear the focus so the right pane goes back to its
      // empty state.
      if (focused && !r.decisions.find((d) => d.id === focused)) {
        setFocused(null)
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [filter, focused])

  useEffect(() => {
    void refresh()
    const id = setInterval(() => void refresh(), POLL_INTERVAL_MS)
    return () => clearInterval(id)
  }, [refresh])

  const focusedDecision = useMemo(
    () => decisions.find((d) => d.id === focused) ?? null,
    [decisions, focused],
  )

  async function pickOption(decisionId: string, optionKey: string) {
    setPicking(decisionId + ':' + optionKey)
    try {
      await resolveDecision(decisionId, optionKey, note.trim() || undefined)
      setNote('')
      await refresh()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setPicking(null)
    }
  }

  return (
    <div style={styles.root}>
      <div style={styles.header}>
        <h2 style={styles.title}>Decision Inbox</h2>
        <div style={styles.filterRow}>
          {(['open', 'resolved', 'all'] as DecisionStatusFilter[]).map((s) => (
            <button
              key={s}
              type="button"
              onClick={() => setFilter(s)}
              style={{
                ...styles.filterBtn,
                ...(filter === s ? styles.filterBtnActive : {}),
              }}
            >
              {s}
            </button>
          ))}
        </div>
      </div>

      {error && <div style={styles.error}>{error}</div>}

      <div style={styles.body}>
        <ul style={styles.list}>
          {decisions.length === 0 && (
            <li style={styles.empty}>No {filter === 'all' ? '' : filter} decisions.</li>
          )}
          {decisions.map((d) => (
            <li
              key={d.id}
              onClick={() => setFocused(d.id)}
              style={{
                ...styles.row,
                ...(focused === d.id ? styles.rowActive : {}),
              }}
            >
              <div style={styles.rowHeadline}>{d.payload.headline || '(no headline)'}</div>
              <div style={styles.rowMeta}>
                {d.agent_name} · #{d.channel_name} · {d.status}
              </div>
            </li>
          ))}
        </ul>

        <section style={styles.detail}>
          {!focusedDecision ? (
            <div style={styles.placeholder}>Select a decision on the left.</div>
          ) : (
            <DecisionDetail
              decision={focusedDecision}
              note={note}
              onChangeNote={setNote}
              onPick={(key) => void pickOption(focusedDecision.id, key)}
              picking={picking}
            />
          )}
        </section>
      </div>
    </div>
  )
}

interface DecisionDetailProps {
  decision: DecisionView
  note: string
  onChangeNote: (s: string) => void
  onPick: (key: string) => void
  picking: string | null
}

function DecisionDetail(props: DecisionDetailProps) {
  const { decision, note, onChangeNote, onPick, picking } = props
  const { payload } = decision
  const isResolved = decision.status === 'resolved'

  return (
    <div style={styles.detailInner}>
      <h3 style={styles.detailHeadline}>{payload.headline}</h3>
      <p style={styles.detailQuestion}>{payload.question}</p>
      <div style={styles.detailMeta}>
        From <strong>{decision.agent_name}</strong> in #{decision.channel_name} ·{' '}
        {decision.created_at}
      </div>

      <div style={styles.optionsRow}>
        {payload.options.map((opt) => {
          const recommended = opt.key === payload.recommended_key
          const pickedThisOne = decision.picked_key === opt.key
          const id = decision.id + ':' + opt.key
          return (
            <article
              key={opt.key}
              style={{
                ...styles.option,
                ...(recommended ? styles.optionRecommended : {}),
                ...(pickedThisOne ? styles.optionPicked : {}),
              }}
            >
              <header style={styles.optionHeader}>
                <span style={styles.optionKey}>{opt.key}</span>
                <span>{opt.label}</span>
                {recommended && <span style={styles.recBadge}>recommended</span>}
              </header>
              <pre style={styles.optionBody}>{opt.body}</pre>
              {!isResolved && (
                <button
                  type="button"
                  onClick={() => onPick(opt.key)}
                  disabled={picking !== null}
                  style={{
                    ...styles.pickBtn,
                    ...(recommended ? styles.pickBtnRecommended : {}),
                  }}
                >
                  {picking === id ? 'Picking…' : `Pick ${opt.key}`}
                </button>
              )}
            </article>
          )
        })}
      </div>

      {!isResolved && (
        <div style={styles.noteRow}>
          <label style={styles.noteLabel} htmlFor="decision-note">
            Optional note for the agent
          </label>
          <textarea
            id="decision-note"
            value={note}
            onChange={(e) => onChangeNote(e.target.value)}
            placeholder="e.g., 'go with B but skip step 3 — we already did that'"
            style={styles.noteInput}
            rows={2}
          />
        </div>
      )}

      {payload.context && (
        <details style={styles.contextWrap}>
          <summary style={styles.contextSummary}>Context</summary>
          <pre style={styles.contextBody}>{payload.context}</pre>
        </details>
      )}

      {isResolved && (
        <div style={styles.resolvedNote}>
          Picked <strong>{decision.picked_key}</strong>
          {decision.picked_note ? ` — ${decision.picked_note}` : ''}
        </div>
      )}
    </div>
  )
}

const styles: Record<string, React.CSSProperties> = {
  root: { display: 'flex', flexDirection: 'column', height: '100%' },
  header: {
    padding: '12px 16px',
    borderBottom: '1px solid #2a2a2a',
    display: 'flex',
    justifyContent: 'space-between',
    alignItems: 'center',
  },
  title: { margin: 0, fontSize: 16, fontWeight: 600 },
  filterRow: { display: 'flex', gap: 4 },
  filterBtn: {
    padding: '4px 10px',
    background: 'transparent',
    border: '1px solid #444',
    color: '#bbb',
    cursor: 'pointer',
    fontSize: 12,
  },
  filterBtnActive: { background: '#2a2a2a', color: '#fff' },
  error: { padding: 12, color: '#f88', borderBottom: '1px solid #2a2a2a' },
  body: { display: 'flex', flex: 1, minHeight: 0 },
  list: {
    width: 280,
    margin: 0,
    padding: 0,
    listStyle: 'none',
    borderRight: '1px solid #2a2a2a',
    overflowY: 'auto',
  },
  empty: { padding: 16, color: '#888', fontSize: 13 },
  row: {
    padding: '10px 12px',
    borderBottom: '1px solid #2a2a2a',
    cursor: 'pointer',
  },
  rowActive: { background: '#1d1d1d' },
  rowHeadline: { fontSize: 13, fontWeight: 500 },
  rowMeta: { fontSize: 11, color: '#888', marginTop: 2 },
  detail: { flex: 1, overflowY: 'auto' },
  placeholder: { padding: 24, color: '#888' },
  detailInner: { padding: 16 },
  detailHeadline: { margin: '0 0 4px 0', fontSize: 15 },
  detailQuestion: { margin: '0 0 8px 0', color: '#ccc' },
  detailMeta: { fontSize: 11, color: '#888', marginBottom: 16 },
  optionsRow: { display: 'flex', flexDirection: 'column', gap: 12 },
  option: {
    border: '1px solid #333',
    padding: 12,
    background: '#181818',
  },
  optionRecommended: { borderColor: '#3b6' },
  optionPicked: { borderColor: '#fc3', background: '#221d10' },
  optionHeader: {
    display: 'flex',
    alignItems: 'center',
    gap: 8,
    marginBottom: 8,
    fontSize: 13,
    fontWeight: 600,
  },
  optionKey: {
    background: '#333',
    color: '#fff',
    padding: '2px 6px',
    fontSize: 11,
    fontFamily: 'monospace',
  },
  recBadge: {
    marginLeft: 'auto',
    color: '#3b6',
    fontSize: 11,
    fontWeight: 400,
  },
  optionBody: {
    margin: 0,
    fontFamily:
      'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace',
    fontSize: 12,
    whiteSpace: 'pre-wrap',
    color: '#bbb',
  },
  pickBtn: {
    marginTop: 8,
    padding: '6px 14px',
    background: '#2a2a2a',
    border: '1px solid #444',
    color: '#fff',
    cursor: 'pointer',
    fontSize: 12,
  },
  pickBtnRecommended: { borderColor: '#3b6' },
  noteRow: { marginTop: 12 },
  noteLabel: {
    display: 'block',
    fontSize: 11,
    color: '#888',
    marginBottom: 4,
  },
  noteInput: {
    width: '100%',
    background: '#101010',
    color: '#ddd',
    border: '1px solid #333',
    padding: 6,
    fontSize: 12,
    fontFamily: 'inherit',
  },
  contextWrap: {
    marginTop: 16,
    border: '1px solid #2a2a2a',
    padding: 8,
  },
  contextSummary: {
    cursor: 'pointer',
    fontSize: 12,
    color: '#888',
  },
  contextBody: {
    margin: '8px 0 0 0',
    fontFamily:
      'ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace',
    fontSize: 11,
    whiteSpace: 'pre-wrap',
    color: '#aaa',
  },
  resolvedNote: {
    marginTop: 16,
    padding: 8,
    fontSize: 12,
    background: '#1d1d10',
    color: '#fc3',
  },
}
