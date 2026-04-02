import { useEffect, useMemo, useState } from 'react'
import {
  deleteAgent,
  getAgentDetail,
  restartAgent,
  startAgent,
  stopAgent,
  updateAgent,
} from '../../../api'
import type { AgentDetailResponse, RuntimeStatusInfo } from '../types'
import { useRuntimeStatuses } from '../../../hooks/useRuntimeStatuses'
import { useApp } from '../../../store'
import {
  AgentConfigForm,
  runtimeStatusSummary,
  type AgentConfigState,
} from '../AgentConfigForm'
import { Dialog, DialogContent, DialogHeader, DialogFooter, DialogTitle, DialogDescription, DialogClose } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { FormError } from '@/components/ui/form'
import './ProfilePanel.css'

function agentColor(name: string): string {
  const colors = ['#FF6B6B', '#4ECDC4', '#45B7D1', '#96CEB4', '#FFEAA7', '#DDA0DD', '#98D8C8']
  let h = 0
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) & 0xffffffff
  return colors[Math.abs(h) % colors.length]
}

function activityLabel(activity?: string, detail?: string): string {
  if (!activity || activity === 'offline') return 'Offline'
  if (detail) return detail
  return activity.charAt(0).toUpperCase() + activity.slice(1)
}

function activityDotColor(activity?: string): string {
  switch (activity) {
    case 'online':
      return 'var(--status-online)'
    case 'thinking':
    case 'working':
      return 'var(--status-sleeping)'
    default:
      return 'var(--status-inactive)'
  }
}

type RestartMode = 'restart' | 'reset_session' | 'full_reset'
type DeleteMode = 'preserve_workspace' | 'delete_workspace'

export function ProfilePanel() {
  const { selectedAgent, refreshAgents, setSelectedAgent } = useApp()
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [detail, setDetail] = useState<AgentDetailResponse | null>(null)
  const [detailLoading, setDetailLoading] = useState(false)
  const { runtimeStatuses, runtimeStatusError } = useRuntimeStatuses()
  const [showEdit, setShowEdit] = useState(false)
  const [showRestart, setShowRestart] = useState(false)
  const [showDelete, setShowDelete] = useState(false)

  useEffect(() => {
    if (!selectedAgent) {
      setDetail(null)
      return
    }
    setDetailLoading(true)
    getAgentDetail(selectedAgent.name)
      .then(setDetail)
      .catch((err) => setError(String(err)))
      .finally(() => setDetailLoading(false))
  }, [selectedAgent])

  if (!selectedAgent) {
    return (
      <div className="profile-panel profile-panel-empty">
        Select an agent to view profile.
      </div>
    )
  }

  const agent = selectedAgent
  const color = agentColor(agent.name)
  const initial = agent.name[0]?.toUpperCase() ?? '?'
  const isActive = agent.status === 'active'
  const envVars = detail?.envVars ?? []
  const reasoningEffort = agent.reasoningEffort ?? detail?.agent.reasoningEffort ?? null
  const runtimeSummary = runtimeStatusSummary(agent.runtime ?? 'claude', runtimeStatuses)

  async function handleStartStop() {
    setBusy(true)
    setError(null)
    try {
      if (isActive) {
        await stopAgent(agent.name)
      } else {
        await startAgent(agent.name)
      }
      refreshAgents()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  async function reloadDetail() {
    const nextDetail = await getAgentDetail(agent.name)
    setDetail(nextDetail)
    refreshAgents()
  }

  return (
    <div className="profile-panel">
      <div className="profile-avatar-section">
        <div className="profile-avatar-row">
          <div className="profile-avatar-large" style={{ background: color }}>
            {initial}
          </div>
          <div className="profile-identity">
            <span className="profile-kicker">[agent::profile]</span>
            <div className="profile-name">{agent.display_name ?? agent.name}</div>
            <div className="profile-handle">@{agent.name}</div>
            <div className="profile-activity-status">
              <span className="profile-activity-dot" style={{ background: activityDotColor(agent.activity) }} />
              {activityLabel(agent.activity, agent.activity_detail)}
            </div>
          </div>
          <div className="profile-toolbar">
            <Button size="sm" type="button" onClick={() => setShowEdit(true)}>
              Edit
            </Button>
            <Button size="sm" type="button" onClick={() => setShowRestart(true)}>
              Restart
            </Button>
            <Button size="sm" variant="destructive" type="button" onClick={() => setShowDelete(true)}>
              Delete
            </Button>
          </div>
        </div>
      </div>

      {error && <FormError>{error}</FormError>}

      <div className="profile-controls">
        <Button variant="outline" type="button" onClick={handleStartStop} disabled={busy}>
          {busy ? '…' : isActive ? '[stop::agent]' : '[start::agent]'}
        </Button>
      </div>

      <div className="profile-section">
        <div className="profile-section-label">[role::brief]</div>
        <div className="profile-role-text">{agent.description || 'No role defined.'}</div>
      </div>

      <div className="profile-section">
        <div className="profile-section-label">[config::runtime]</div>
        <div className="profile-config-grid">
          <span className="profile-config-key">Runtime</span>
          <span className={`badge badge-${agent.runtime ?? 'claude'}`}>{agent.runtime ?? 'claude'}</span>
          <span className="profile-config-key">Model</span>
          <span className="badge">{agent.model ?? 'sonnet'}</span>
          {agent.runtime === 'codex' && (
            <>
              <span className="profile-config-key">Reasoning</span>
              <span className="badge">{reasoningEffort ?? 'default'}</span>
            </>
          )}
          <span className="profile-config-key">Status</span>
          <span className="badge" style={{ background: isActive ? 'var(--status-online)' : agent.status === 'sleeping' ? 'var(--status-sleeping)' : 'var(--status-inactive)' }}>
            {agent.status}
          </span>
        </div>
        <div className={`runtime-status-banner runtime-status-banner-${runtimeSummary.tone}`}>
          <strong>{runtimeSummary.title}</strong>
          <span>{runtimeSummary.detail}</span>
        </div>
        {runtimeStatusError && (
          <div className="profile-role-text">{runtimeStatusError}</div>
        )}
      </div>

      <div className="profile-section">
        <div className="profile-section-label">[env::vars]</div>
        {detailLoading ? (
          <div className="profile-role-text">Loading...</div>
        ) : envVars.length === 0 ? (
          <div className="profile-role-text">No environment variables configured.</div>
        ) : (
          envVars.map((envVar) => (
            <div key={envVar.key} className="env-var-row">
              <span className="env-var-key">{envVar.key}</span>
              <span className="env-var-val">{envVar.value || '(empty)'}</span>
            </div>
          ))
        )}
      </div>

      {detail && (
        <EditAgentModal
          open={showEdit}
          agentName={agent.name}
          initialState={{
            name: agent.name,
            display_name: detail.agent.display_name ?? agent.name,
            description: detail.agent.description ?? '',
            runtime: detail.agent.runtime ?? 'claude',
            model: detail.agent.model ?? 'sonnet',
            reasoningEffort:
              detail.agent.runtime === 'codex'
                ? (detail.agent.reasoningEffort ?? 'default')
                : null,
            envVars: detail.envVars,
          }}
          runtimeStatuses={runtimeStatuses}
          runtimeStatusError={runtimeStatusError}
          onOpenChange={setShowEdit}
          onSaved={async () => {
            setShowEdit(false)
            await reloadDetail()
          }}
        />
      )}

      <RestartAgentModal
        open={showRestart}
        agentName={agent.name}
        onOpenChange={setShowRestart}
        onRestarted={async () => {
          setShowRestart(false)
          await reloadDetail()
        }}
      />

      <DeleteAgentModal
        open={showDelete}
        agentName={agent.name}
        onOpenChange={setShowDelete}
        onDeleted={async () => {
          setShowDelete(false)
          setSelectedAgent(null)
          await refreshAgents()
        }}
      />
    </div>
  )
}

function EditAgentModal({
  open,
  agentName,
  initialState,
  runtimeStatuses,
  runtimeStatusError,
  onOpenChange,
  onSaved,
}: {
  open: boolean
  agentName: string
  initialState: AgentConfigState
  runtimeStatuses: RuntimeStatusInfo[]
  runtimeStatusError: string | null
  onOpenChange: (open: boolean) => void
  onSaved: () => Promise<void>
}) {
  const [state, setState] = useState<AgentConfigState>(initialState)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleSave() {
    setSaving(true)
    setError(null)
    try {
      if (!state.model.trim()) {
        throw new Error('Model is required')
      }
      await updateAgent(agentName, {
        display_name: state.display_name,
        description: state.description,
        runtime: state.runtime,
        model: state.model,
        reasoningEffort: state.runtime === 'codex' || state.runtime === 'opencode' ? state.reasoningEffort : null,
        envVars: state.envVars.filter((envVar) => envVar.key.trim() || envVar.value),
      })
      await onSaved()
    } catch (e) {
      setError(String(e))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="w-[min(720px,96vw)]">
        <DialogHeader>
          <div className="flex flex-col gap-1">
            <DialogTitle>Edit Agent</DialogTitle>
            <DialogDescription>@{agentName}</DialogDescription>
          </div>
          <DialogClose className="h-8 w-8 grid place-items-center text-muted-foreground hover:bg-secondary hover:text-foreground">×</DialogClose>
        </DialogHeader>
        <AgentConfigForm state={state} runtimeStatuses={runtimeStatuses} runtimeStatusError={runtimeStatusError} onChange={setState} />
        {error && <FormError>{error}</FormError>}
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button onClick={handleSave} disabled={saving || !state.model.trim()}>
            {saving ? 'Saving...' : 'Save'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function RestartAgentModal({
  open,
  agentName,
  onOpenChange,
  onRestarted,
}: {
  open: boolean
  agentName: string
  onOpenChange: (open: boolean) => void
  onRestarted: () => Promise<void>
}) {
  const [mode, setMode] = useState<RestartMode>('restart')
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const options = useMemo(
    () => [
      {
        id: 'restart' as const,
        title: 'Restart',
        body: 'Stop and restart the agent process. Keeps conversation state and workspace files.',
      },
      {
        id: 'reset_session' as const,
        title: 'Reset Session & Restart',
        body: 'Clear the saved conversation session, but keep workspace files such as MEMORY.md and notes/.',
      },
      {
        id: 'full_reset' as const,
        title: 'Full Reset & Restart',
        body: 'Clear the saved conversation session, delete workspace files, and start fresh.',
      },
    ],
    []
  )

  async function handleSubmit() {
    setSubmitting(true)
    setError(null)
    try {
      await restartAgent(agentName, mode)
      await onRestarted()
    } catch (e) {
      setError(String(e))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Restart {agentName}</DialogTitle>
          <DialogClose className="h-8 w-8 grid place-items-center text-muted-foreground hover:bg-secondary hover:text-foreground">×</DialogClose>
        </DialogHeader>
        <div className="modal-choice-list">
          {options.map((option) => (
            <button
              key={option.id}
              type="button"
              className={`modal-choice-card ${mode === option.id ? 'modal-choice-card-active' : ''}`}
              onClick={() => setMode(option.id)}
            >
              <span className="modal-choice-title">{option.title}</span>
              <span className="modal-choice-body">{option.body}</span>
            </button>
          ))}
        </div>
        {error && <FormError>{error}</FormError>}
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button onClick={handleSubmit} disabled={submitting}>
            {submitting ? 'Restarting...' : 'Restart'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function DeleteAgentModal({
  open,
  agentName,
  onOpenChange,
  onDeleted,
}: {
  open: boolean
  agentName: string
  onOpenChange: (open: boolean) => void
  onDeleted: () => Promise<void>
}) {
  const [mode, setMode] = useState<DeleteMode>('preserve_workspace')
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  async function handleSubmit() {
    setSubmitting(true)
    setError(null)
    try {
      await deleteAgent(agentName, mode)
      await onDeleted()
    } catch (e) {
      setError(String(e))
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Delete {agentName}</DialogTitle>
          <DialogClose className="h-8 w-8 grid place-items-center text-muted-foreground hover:bg-secondary hover:text-foreground">×</DialogClose>
        </DialogHeader>
        <div className="modal-choice-list">
          <button
            type="button"
            className={`modal-choice-card ${mode === 'preserve_workspace' ? 'modal-choice-card-active' : ''}`}
            onClick={() => setMode('preserve_workspace')}
          >
            <span className="modal-choice-title">Delete Agent Only</span>
            <span className="modal-choice-body">Remove the Chorus record and keep workspace files on disk.</span>
          </button>
          <button
            type="button"
            className={`modal-choice-card ${mode === 'delete_workspace' ? 'modal-choice-card-active' : ''}`}
            onClick={() => setMode('delete_workspace')}
          >
            <span className="modal-choice-title">Delete Agent + Workspace</span>
            <span className="modal-choice-body">Remove the Chorus record and delete the workspace directory.</span>
          </button>
        </div>
        {error && <FormError>{error}</FormError>}
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button variant="destructive" onClick={handleSubmit} disabled={submitting}>
            {submitting ? 'Deleting...' : 'Delete'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
