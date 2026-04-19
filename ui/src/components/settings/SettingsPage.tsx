import { useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { X } from 'lucide-react'
import { updateHuman, channelQueryKeys, getSystemInfo, getLogs } from '../../data'
import type { SystemInfo, ConfigInfo } from '../../data'
import { useHumans } from '../../hooks/data'
import { useStore } from '../../store'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { FormField, FormError } from '@/components/ui/form'
import { Label } from '@/components/ui/label'
import './SettingsPage.css'

type SettingsSection = 'profile' | 'system' | 'logs'

const NAV_ITEMS: { id: SettingsSection; label: string }[] = [
  { id: 'profile', label: 'Profile' },
  { id: 'system', label: 'System' },
  { id: 'logs', label: 'Logs' },
]

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

function ProfileSection({ username }: { username: string }) {
  const queryClient = useQueryClient()
  const humans = useHumans()
  const currentHuman = humans.find((h) => h.name === username)

  const [displayName, setDisplayName] = useState(currentHuman?.display_name ?? '')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [saved, setSaved] = useState(false)
  const savedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    return () => {
      if (savedTimerRef.current !== null) clearTimeout(savedTimerRef.current)
    }
  }, [])

  useEffect(() => {
    setDisplayName(currentHuman?.display_name ?? '')
  }, [currentHuman?.display_name])

  async function handleSave() {
    setSaving(true)
    setError(null)
    setSaved(false)
    try {
      await updateHuman(username, { display_name: displayName.trim() || null })
      await queryClient.invalidateQueries({ queryKey: channelQueryKeys.humans })
      setSaved(true)
      savedTimerRef.current = setTimeout(() => setSaved(false), 2000)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="settings-section">
      <div className="settings-section-header">
        <h2 className="settings-section-title">Profile</h2>
        <p className="settings-section-desc">
          Logged in as <span className="font-mono">{username}</span>
        </p>
      </div>

      <FormField>
        <Label htmlFor="display-name">Display name</Label>
        <Input
          id="display-name"
          placeholder={username}
          value={displayName}
          onChange={(e) => setDisplayName(e.target.value)}
          disabled={saving}
        />
      </FormField>

      {error && <FormError>{error}</FormError>}

      <div className="settings-actions">
        <Button onClick={handleSave} disabled={saving}>
          {saving ? 'Saving…' : saved ? 'Saved' : 'Save'}
        </Button>
      </div>
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
  const { currentUser, setShowSettings } = useStore()
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
          {activeSection === 'profile' && <ProfileSection username={currentUser} />}
          {activeSection === 'system' && <SystemSection />}
          {activeSection === 'logs' && <LogsSection />}
        </div>
      </div>
    </div>
  )
}
