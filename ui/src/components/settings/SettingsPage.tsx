import { useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Check, Plus, Trash2, X } from 'lucide-react'
import {
  getSystemInfo,
  getLogs,
  createWorkspace,
  switchWorkspace,
  deleteWorkspace,
  workspaceQueryKeys,
} from '../../data'
import type { SystemInfo, ConfigInfo, WorkspaceInfo } from '../../data'
import { useRefresh, useWorkspaces } from '../../hooks/data'
import { useStore } from '../../store'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { FormField, FormError } from '@/components/ui/form'
import { Label } from '@/components/ui/label'
import './SettingsPage.css'

type SettingsSection = 'profile' | 'workspaces' | 'appearance' | 'system' | 'logs'

const NAV_ITEMS: { id: SettingsSection; label: string }[] = [
  { id: 'profile', label: 'Profile' },
  { id: 'workspaces', label: 'Workspaces' },
  { id: 'appearance', label: 'Appearance' },
  { id: 'system', label: 'System' },
  { id: 'logs', label: 'Logs' },
]

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

function formatDate(value: string): string {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return date.toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'short',
    day: '2-digit',
  })
}

function formatCount(value: number, label: string): string {
  return `${value} ${label}${value === 1 ? '' : 's'}`
}

function ProfileSection({ humanId, name }: { humanId: string; name: string }) {
  return (
    <div className="settings-section">
      <div className="settings-section-header">
        <h2 className="settings-section-title">Profile</h2>
        <p className="settings-section-desc">
          Logged in as <span className="font-mono">{name}</span>
        </p>
      </div>

      <FormField>
        <Label htmlFor="human-name">Name</Label>
        <Input id="human-name" value={name} readOnly />
      </FormField>

      <FormField>
        <Label htmlFor="human-id">Human ID</Label>
        <Input id="human-id" value={humanId} readOnly />
      </FormField>
    </div>
  )
}

function WorkspaceSection() {
  const queryClient = useQueryClient()
  const { refreshServerInfo } = useRefresh()
  const { workspaces, isLoading, error } = useWorkspaces()
  const setCurrentAgent = useStore((s) => s.setCurrentAgent)
  const setCurrentChannel = useStore((s) => s.setCurrentChannel)
  const setCurrentTaskDetail = useStore((s) => s.setCurrentTaskDetail)
  const setActiveTab = useStore((s) => s.setActiveTab)
  const pushToast = useStore((s) => s.pushToast)
  const [name, setName] = useState('')
  const [pendingAction, setPendingAction] = useState<string | null>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null)

  const activeWorkspace = workspaces.find((workspace) => workspace.active) ?? null

  async function refreshWorkspaceShell() {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: workspaceQueryKeys.workspaces }),
      queryClient.invalidateQueries({ queryKey: workspaceQueryKeys.current }),
      refreshServerInfo(),
    ])
  }

  function clearWorkspaceScopedSelection() {
    setCurrentAgent(null)
    setCurrentChannel(null)
    setCurrentTaskDetail(null)
    setActiveTab('chat')
  }

  async function runWorkspaceAction(
    actionId: string,
    action: () => Promise<WorkspaceInfo | null>,
    options: { clearSelection?: boolean } = {},
  ) {
    setPendingAction(actionId)
    setActionError(null)
    try {
      const active = await action()
      if (options.clearSelection ?? true) {
        clearWorkspaceScopedSelection()
      }
      await refreshWorkspaceShell()
      if (active) {
        pushToast({
          id: crypto.randomUUID(),
          message: `Workspace active: ${active.name}`,
          level: 'info',
        })
      }
    } catch (err) {
      setActionError(err instanceof Error ? err.message : String(err))
    } finally {
      setPendingAction(null)
    }
  }

  async function handleCreate() {
    const nextName = name.trim()
    if (!nextName) return
    await runWorkspaceAction(
      'create',
      async () => {
        await createWorkspace(nextName)
        setName('')
        return null
      },
      { clearSelection: false },
    )
  }

  async function handleSwitch(workspace: WorkspaceInfo) {
    await runWorkspaceAction(`switch:${workspace.id}`, () => switchWorkspace(workspace.id))
  }

  async function handleDelete(workspace: WorkspaceInfo) {
    if (deleteConfirmId !== workspace.id) {
      setDeleteConfirmId(workspace.id)
      return
    }
    await runWorkspaceAction(
      `delete:${workspace.id}`,
      async () => {
        const response = await deleteWorkspace(workspace.id)
        setDeleteConfirmId(null)
        return workspace.active ? response.active_workspace : null
      },
      { clearSelection: workspace.active },
    )
  }

  return (
    <div className="settings-section settings-section-wide">
      <div className="settings-section-header">
        <h2 className="settings-section-title">Workspaces</h2>
        <p className="settings-section-desc">
          Active workspace: <span className="font-mono">{activeWorkspace?.name ?? 'none'}</span>
        </p>
      </div>

      <div className="settings-workspace-create">
        <FormField className="settings-workspace-name-field">
          <Label htmlFor="workspace-name">New workspace</Label>
          <Input
            id="workspace-name"
            placeholder="Chorus Local"
            value={name}
            onChange={(event) => setName(event.target.value)}
            disabled={pendingAction === 'create'}
            onKeyDown={(event) => {
              if (event.key === 'Enter') {
                event.preventDefault()
                void handleCreate()
              }
            }}
          />
        </FormField>
        <Button
          type="button"
          onClick={handleCreate}
          disabled={!name.trim() || pendingAction === 'create'}
        >
          <Plus size={14} />
          {pendingAction === 'create' ? 'Creating' : 'Create'}
        </Button>
      </div>

      {actionError && <FormError>{actionError}</FormError>}
      {error && <FormError>Failed to load workspaces: {error.message}</FormError>}

      <div className="settings-workspace-list">
        {isLoading && <div className="settings-workspace-empty">Loading workspaces…</div>}
        {!isLoading && workspaces.length === 0 && (
          <div className="settings-workspace-empty">No workspaces.</div>
        )}
        {workspaces.map((workspace) => (
          <WorkspaceRow
            key={workspace.id}
            workspace={workspace}
            pendingAction={pendingAction}
            confirmingDelete={deleteConfirmId === workspace.id}
            onSwitch={handleSwitch}
            onDelete={handleDelete}
            onCancelDelete={() => setDeleteConfirmId(null)}
          />
        ))}
      </div>
    </div>
  )
}

function WorkspaceRow({
  workspace,
  pendingAction,
  confirmingDelete,
  onSwitch,
  onDelete,
  onCancelDelete,
}: {
  workspace: WorkspaceInfo
  pendingAction: string | null
  confirmingDelete: boolean
  onSwitch: (workspace: WorkspaceInfo) => Promise<void>
  onDelete: (workspace: WorkspaceInfo) => Promise<void>
  onCancelDelete: () => void
}) {
  const isPending =
    pendingAction === `switch:${workspace.id}` || pendingAction === `delete:${workspace.id}`
  return (
    <div className={`settings-workspace-row${workspace.active ? ' is-active' : ''}`}>
      <div className="settings-workspace-main">
        <div className="settings-workspace-title-row">
          <span className="settings-workspace-name">{workspace.name}</span>
          {workspace.active && (
            <span className="settings-workspace-active">
              <Check size={12} />
              active
            </span>
          )}
        </div>
        <div className="settings-workspace-meta">
          <span>{workspace.slug}</span>
          <span>{workspace.mode}</span>
          <span>{formatCount(workspace.channel_count, 'channel')}</span>
          <span>{formatCount(workspace.agent_count, 'agent')}</span>
          <span>{formatCount(workspace.human_count, 'human')}</span>
          <span>{workspace.created_by_human ?? 'local'}</span>
          <span>{formatDate(workspace.created_at)}</span>
        </div>
      </div>
      <div className="settings-workspace-actions">
        {!workspace.active && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={isPending}
            onClick={() => void onSwitch(workspace)}
          >
            {pendingAction === `switch:${workspace.id}` ? 'Switching' : 'Switch'}
          </Button>
        )}
        {confirmingDelete ? (
          <>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={isPending}
              onClick={onCancelDelete}
            >
              Cancel
            </Button>
            <Button
              type="button"
              variant="destructive"
              size="sm"
              disabled={isPending}
              onClick={() => void onDelete(workspace)}
            >
              Confirm
            </Button>
          </>
        ) : (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={isPending}
            onClick={() => void onDelete(workspace)}
          >
            <Trash2 size={13} />
            Delete
          </Button>
        )}
      </div>
    </div>
  )
}

function AppearanceSection() {
  const showConversationIds = useStore((s) => s.showConversationIds)
  const setShowConversationIds = useStore((s) => s.setShowConversationIds)

  return (
    <div className="settings-section">
      <div className="settings-section-header">
        <h2 className="settings-section-title">Appearance</h2>
        <p className="settings-section-desc">Display preferences for the sidebar and chat surface.</p>
      </div>

      <label className="settings-toggle-row">
        <input
          type="checkbox"
          className="settings-toggle-input"
          checked={showConversationIds}
          onChange={(e) => setShowConversationIds(e.target.checked)}
        />
        <span className="settings-toggle-copy">
          <span className="settings-toggle-label">Show conversation IDs</span>
          <span className="settings-toggle-desc">
            Render the underlying UUID below each channel and agent row. Useful for debugging; off by default.
          </span>
        </span>
      </label>
    </div>
  )
}

function ConfigSection({ config }: { config: ConfigInfo }) {
  return (
    <div className="settings-config">
      <div className="settings-config-group">
        <h3 className="settings-config-heading">General</h3>
        <div className="settings-info-grid">
          <span className="settings-info-label">Machine ID</span>
          <span className="settings-info-value">{config.machine_id ?? '—'}</span>
          <span className="settings-info-label">Local human</span>
          <span className="settings-info-value">{config.local_human?.name ?? '—'}</span>
        </div>
      </div>

      <div className="settings-config-group">
        <h3 className="settings-config-heading">Templates</h3>
        <div className="settings-info-grid">
          <span className="settings-info-label">Directory</span>
          <span className="settings-info-value">{config.agent_template.dir ?? '(default)'}</span>
          <span className="settings-info-label">Default</span>
          <span className="settings-info-value">
            {config.agent_template.default || '(none)'}
          </span>
        </div>
      </div>

      <div className="settings-config-group">
        <h3 className="settings-config-heading">Logging</h3>
        <div className="settings-info-grid">
          <span className="settings-info-label">Level</span>
          <span className="settings-info-value">{config.logs.level}</span>
          <span className="settings-info-label">Rotation</span>
          <span className="settings-info-value">{config.logs.rotation}</span>
          <span className="settings-info-label">Retention</span>
          <span className="settings-info-value">{config.logs.retention} files</span>
        </div>
      </div>

      {config.runtimes.length > 0 && (
        <div className="settings-config-group">
          <h3 className="settings-config-heading">Runtimes</h3>
          {config.runtimes.map((rt) => (
            <div key={rt.name} className="settings-runtime-entry">
              <span className="settings-runtime-name">{rt.name}</span>
              <div className="settings-info-grid">
                {rt.binary_path && (
                  <>
                    <span className="settings-info-label">Binary</span>
                    <span className="settings-info-value">{rt.binary_path}</span>
                  </>
                )}
                {rt.acp_adaptor && (
                  <>
                    <span className="settings-info-label">ACP adaptor</span>
                    <span className="settings-info-value">{rt.acp_adaptor}</span>
                  </>
                )}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

function SystemSection() {
  const [systemInfo, setSystemInfo] = useState<SystemInfo | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    setLoading(true)
    setError(null)
    getSystemInfo()
      .then(setSystemInfo)
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false))
  }, [])

  if (loading) {
    return (
      <div className="settings-section">
        <div className="settings-section-header">
          <h2 className="settings-section-title">System</h2>
        </div>
        <p className="text-muted-foreground font-mono text-xs">Loading…</p>
      </div>
    )
  }

  if (error) {
    return (
      <div className="settings-section">
        <div className="settings-section-header">
          <h2 className="settings-section-title">System</h2>
        </div>
        <FormError>Failed to load system info: {error}</FormError>
      </div>
    )
  }

  return (
    <div className="settings-section">
      <div className="settings-section-header">
        <h2 className="settings-section-title">System</h2>
        <p className="settings-section-desc">Runtime information</p>
      </div>

      <div className="settings-info-grid">
        <span className="settings-info-label">Data directory</span>
        <span className="settings-info-value">{systemInfo?.data_dir ?? '—'}</span>
        <span className="settings-info-label">Store size</span>
        <span className="settings-info-value">
          {systemInfo?.db_size_bytes != null ? formatBytes(systemInfo.db_size_bytes) : '—'}
        </span>
      </div>

      {systemInfo?.config ? (
        <ConfigSection config={systemInfo.config} />
      ) : (
        <p className="settings-config-missing">
          No config file found. Run <code>chorus setup</code> to generate one.
        </p>
      )}
    </div>
  )
}

const LOG_LEVEL_RE = /\b(TRACE|DEBUG|INFO|WARN|ERROR)\b/

function logLevelClass(line: string): string {
  const match = LOG_LEVEL_RE.exec(line)
  if (!match) return ''
  return `log-${match[1].toLowerCase()}`
}

function LogsSection() {
  const [logs, setLogs] = useState<string[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const logContainerRef = useRef<HTMLDivElement>(null)

  function fetchLogs() {
    setLoading(true)
    setError(null)
    getLogs(500)
      .then((r) => setLogs(r.lines))
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false))
  }

  useEffect(() => {
    fetchLogs()
  }, [])

  useEffect(() => {
    if (logContainerRef.current) {
      logContainerRef.current.scrollTop = logContainerRef.current.scrollHeight
    }
  }, [logs])

  return (
    <div className="settings-section">
      <div className="settings-section-header">
        <h2 className="settings-section-title">Logs</h2>
        <Button variant="ghost" size="sm" onClick={fetchLogs} disabled={loading}>
          {loading ? 'Loading…' : 'Refresh'}
        </Button>
      </div>

      <div className="settings-log-container" ref={logContainerRef}>
        {error && <div className="settings-log-empty log-error">Failed to load logs: {error}</div>}
        {!error && logs.length === 0 && !loading && (
          <div className="settings-log-empty">No logs available</div>
        )}
        {logs.map((line, i) => (
          <div key={i} className={`settings-log-line ${logLevelClass(line)}`}>
            {line}
          </div>
        ))}
      </div>
    </div>
  )
}

export function SettingsPage() {
  const { currentUser, currentUserId, setShowSettings } = useStore()
  const [activeSection, setActiveSection] = useState<SettingsSection>('profile')

  return (
    <div className="settings-page">
      <div className="settings-page-header">
        <h1 className="settings-page-title">Settings</h1>
        <button
          className="settings-close"
          type="button"
          aria-label="Close settings"
          onClick={() => setShowSettings(false)}
        >
          <X size={16} />
        </button>
      </div>

      <div className="settings-layout">
        <nav className="settings-nav" aria-label="Settings sections">
          {NAV_ITEMS.map((item) => (
            <button
              key={item.id}
              type="button"
              className={`settings-nav-item${activeSection === item.id ? ' is-active' : ''}`}
              onClick={() => setActiveSection(item.id)}
            >
              {item.label}
            </button>
          ))}
        </nav>

        <div className="settings-content">
          {activeSection === 'profile' && <ProfileSection humanId={currentUserId} name={currentUser} />}
          {activeSection === 'workspaces' && <WorkspaceSection />}
          {activeSection === 'appearance' && <AppearanceSection />}
          {activeSection === 'system' && <SystemSection />}
          {activeSection === 'logs' && <LogsSection />}
        </div>
      </div>
    </div>
  )
}
