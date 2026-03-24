import { useState } from 'react'
import { createChannel } from '../api'

interface Props {
  onClose: () => void
  onCreated: (channel: { id: string; name: string }) => void
}

export function CreateChannelModal({ onClose, onCreated }: Props) {
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleCreate() {
    const trimmed = name.trim().replace(/^#/, '')
    if (!trimmed) return
    setCreating(true)
    setError(null)
    try {
      const created = await createChannel({ name: trimmed, description })
      onCreated(created)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setCreating(false)
    }
  }

  return (
    <div className="modal-overlay" onClick={(e) => { if (e.target === e.currentTarget) onClose() }}>
      <div className="modal-card">
        <div className="modal-header">
          <span className="modal-title">Create Channel</span>
          <button className="modal-close" onClick={onClose}>×</button>
        </div>

        {error && <div className="error-banner">{error}</div>}

        <div className="form-group">
          <label className="form-label">Channel Name</label>
          <input
            className="form-input"
            placeholder="e.g. engineering"
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && handleCreate()}
            autoFocus
          />
        </div>

        <div className="form-group">
          <label className="form-label">Description (optional)</label>
          <input
            className="form-input"
            placeholder="What's this channel about?"
            value={description}
            onChange={(e) => setDescription(e.target.value)}
          />
        </div>

        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 20 }}>
          <button className="btn-brutal" onClick={onClose}>Cancel</button>
          <button
            className="btn-brutal btn-cyan"
            onClick={handleCreate}
            disabled={creating || !name.trim()}
          >
            {creating ? 'Creating…' : 'Create Channel'}
          </button>
        </div>
      </div>
    </div>
  )
}
