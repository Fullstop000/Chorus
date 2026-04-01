import { useState } from 'react'
import { useRuntimeStatuses } from '../hooks/useRuntimeStatuses'
import './ProfilePanel.css'  // reuses modal styles
import { AgentConfigForm, type AgentConfigState } from './AgentConfigForm'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'

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
    reasoningEffort: null,
    envVars: [],
  })
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const { runtimeStatuses, runtimeStatusError } = useRuntimeStatuses()

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
          reasoningEffort: config.runtime === 'codex' || config.runtime === 'opencode' ? config.reasoningEffort : null,
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
      <div className="modal-box modal-box-agent">
        <div className="modal-header">
          <div className="modal-title-block">
            <span className="modal-title">Create Agent</span>
            <span className="modal-subtitle">[agent::new]</span>
          </div>
          <button className="modal-close" onClick={onClose}>×</button>
        </div>

        <div className="modal-field">
          <label className="form-label">Machine</label>
          <Select value="local" disabled>
            <SelectTrigger className="form-select" aria-label="Machine">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="local">local</SelectItem>
            </SelectContent>
          </Select>
        </div>

        <AgentConfigForm
          state={config}
          runtimeStatuses={runtimeStatuses}
          runtimeStatusError={runtimeStatusError}
          editableName
          onChange={setConfig}
        />

        {error && <div className="error-banner">{error}</div>}

        <div className="modal-footer">
          <button className="btn-brutal" onClick={onClose}>Cancel</button>
          <button
            className="btn-brutal btn-cyan"
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
