import { useEffect, useRef, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { X } from 'lucide-react'
import { updateHuman, channelQueryKeys, getSystemInfo, getLogs } from '../../data'
import type { SystemInfo } from '../../data'
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
  { id: 'logs', label: 'Server Logs' },
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
      setTimeout(() => setSaved(false), 2000)
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

function SystemSection() {
  const [systemInfo, setSystemInfo] = useState<SystemInfo | null>(null)
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    setLoading(true)
    getSystemInfo()
      .then(setSystemInfo)
      .catch(() => {})
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
    </div>
  )
}

function LogsSection() {
  const [logs, setLogs] = useState<string[]>([])
  const [loading, setLoading] = useState(true)
  const logContainerRef = useRef<HTMLDivElement>(null)

  function fetchLogs() {
    setLoading(true)
    getLogs(500)
      .then((r) => setLogs(r.lines))
      .catch(() => {})
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
        <h2 className="settings-section-title">Server Logs</h2>
        <Button variant="ghost" size="sm" onClick={fetchLogs} disabled={loading}>
          {loading ? 'Loading…' : 'Refresh'}
        </Button>
      </div>

      <div className="settings-log-container" ref={logContainerRef}>
        {logs.length === 0 && !loading && (
          <div className="settings-log-empty">No logs available</div>
        )}
        {logs.map((line, i) => (
          <div key={i} className="settings-log-line">
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
