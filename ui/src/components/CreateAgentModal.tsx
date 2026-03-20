import { useState } from 'react'
import './ProfilePanel.css'  // reuses modal styles

interface Props {
  onClose: () => void
  onCreated: () => void
}

interface EnvVar {
  key: string
  value: string
}

const MODELS: Record<string, { value: string; label: string }[]> = {
  claude: [
    { value: 'sonnet', label: 'claude-sonnet-4-6' },
    { value: 'opus', label: 'claude-opus-4-6' },
    { value: 'haiku', label: 'claude-haiku-4-5' },
  ],
  codex: [
    { value: 'gpt-5.4', label: 'gpt-5.4' },
    { value: 'gpt-5.4-mini', label: 'gpt-5.4-mini' },
    { value: 'gpt-5.3-codex', label: 'gpt-5.3-codex' },
    { value: 'gpt-5.2-codex', label: 'gpt-5.2-codex' },
    { value: 'gpt-5.2', label: 'gpt-5.2' },
    { value: 'gpt-5.1-codex-max', label: 'gpt-5.1-codex-max' },
    { value: 'gpt-5.1-codex-mini', label: 'gpt-5.1-codex-mini' },
  ],
}

export function CreateAgentModal({ onClose, onCreated }: Props) {
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [runtime, setRuntime] = useState('claude')
  const [model, setModel] = useState('sonnet')
  const [showAdvanced, setShowAdvanced] = useState(false)
  const [envVars, setEnvVars] = useState<EnvVar[]>([])
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)

  function addEnvVar() {
    setEnvVars((prev) => [...prev, { key: '', value: '' }])
  }

  function updateEnvVar(i: number, field: 'key' | 'value', val: string) {
    setEnvVars((prev) => prev.map((e, j) => (j === i ? { ...e, [field]: val } : e)))
  }

  async function handleCreate() {
    if (!name.trim()) {
      setError('Name is required')
      return
    }
    setCreating(true)
    setError(null)
    try {
      const res = await fetch('/api/agents', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: name.trim(), description, runtime, model }),
      })
      if (!res.ok) {
        const body = await res.json().catch(() => ({ error: res.statusText }))
        throw new Error((body as { error?: string }).error ?? res.statusText)
      }
      onCreated()
    } catch (e) {
      setError(String(e))
    } finally {
      setCreating(false)
    }
  }

  return (
    <div className="modal-overlay" onClick={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal-box">
        <div className="modal-header">
          <span className="modal-title">Create Agent</span>
          <button className="modal-close" onClick={onClose}>×</button>
        </div>

        <div className="modal-field">
          <label>Machine</label>
          <select disabled value="local">
            <option value="local">local</option>
          </select>
        </div>

        <div className="modal-field">
          <label>Name</label>
          <input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="e.g. my-agent"
            autoFocus
          />
        </div>

        <div className="modal-field">
          <label>Description</label>
          <textarea
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            placeholder="What does this agent do?"
          />
        </div>

        <div className="modal-field">
          <label>Runtime</label>
          <select
            value={runtime}
            onChange={(e) => {
              const r = e.target.value
              setRuntime(r)
              setModel(MODELS[r][0].value)
            }}
          >
            <option value="claude">Claude Code</option>
            <option value="codex">Codex CLI</option>
          </select>
        </div>

        <div className="modal-field">
          <label>Model</label>
          <select value={model} onChange={(e) => setModel(e.target.value)}>
            {(MODELS[runtime] ?? []).map((m) => (
              <option key={m.value} value={m.value}>{m.label}</option>
            ))}
          </select>
        </div>

        <button
          className="modal-accordion-trigger"
          onClick={() => setShowAdvanced((v) => !v)}
        >
          {showAdvanced ? '▾' : '▸'} Advanced
        </button>

        {showAdvanced && (
          <div className="env-var-editor">
            {envVars.map((ev, i) => (
              <div key={i} className="env-var-editor-row">
                <input
                  placeholder="KEY"
                  value={ev.key}
                  onChange={(e) => updateEnvVar(i, 'key', e.target.value)}
                />
                <input
                  placeholder="value"
                  value={ev.value}
                  onChange={(e) => updateEnvVar(i, 'value', e.target.value)}
                />
              </div>
            ))}
            <button className="env-add-btn" onClick={addEnvVar}>
              + Add Variable
            </button>
          </div>
        )}

        {error && (
          <div style={{ color: 'var(--accent)', fontSize: 13, marginTop: 8 }}>{error}</div>
        )}

        <div className="modal-footer">
          <button className="btn-secondary" onClick={onClose}>Cancel</button>
          <button
            className="btn-primary"
            onClick={handleCreate}
            disabled={creating || !name.trim()}
          >
            {creating ? 'Creating...' : 'Create Agent'}
          </button>
        </div>
      </div>
    </div>
  )
}
