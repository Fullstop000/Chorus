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
      alert(
        `To create agent, run:\nchorus agent create ${name.trim()} --runtime ${runtime} --model ${model}`
      )
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
          <select value={runtime} onChange={(e) => setRuntime(e.target.value)}>
            <option value="claude">Claude Code</option>
            <option value="codex">Codex CLI</option>
          </select>
        </div>

        <div className="modal-field">
          <label>Model</label>
          <select value={model} onChange={(e) => setModel(e.target.value)}>
            <option value="sonnet">Sonnet</option>
            <option value="opus">Opus</option>
            <option value="haiku">Haiku</option>
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
