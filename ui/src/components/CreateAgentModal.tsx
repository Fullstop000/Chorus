import { useState } from 'react'
import './ProfilePanel.css'  // reuses modal styles
import { AgentConfigForm, type AgentConfigState } from './AgentConfigForm'

interface Props {
  onClose: () => void
  onCreated: () => void
}

export function CreateAgentModal({ onClose, onCreated }: Props) {
  const [config, setConfig] = useState<AgentConfigState>({
    name: '',
    display_name: '',
    description: '',
    runtime: 'claude',
    model: 'sonnet',
    envVars: [],
  })
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleCreate() {
    if (!config.name.trim()) {
      setError('Name is required')
      return
    }
    setCreating(true)
    setError(null)
    try {
      const res = await fetch('/api/agents', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          name: config.name.trim(),
          display_name: config.display_name.trim(),
          description: config.description,
          runtime: config.runtime,
          model: config.model,
          envVars: config.envVars,
        }),
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

        <AgentConfigForm state={config} editableName onChange={setConfig} />

        {error && (
          <div style={{ color: 'var(--accent)', fontSize: 13, marginTop: 8 }}>{error}</div>
        )}

        <div className="modal-footer">
          <button className="btn-secondary" onClick={onClose}>Cancel</button>
          <button
            className="btn-primary"
            onClick={handleCreate}
            disabled={creating || !config.name.trim()}
          >
            {creating ? 'Creating...' : 'Create Agent'}
          </button>
        </div>
      </div>
    </div>
  )
}
