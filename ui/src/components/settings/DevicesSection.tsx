import { useCallback, useEffect, useState } from 'react'
import { Trash2 } from 'lucide-react'
import {
  forgetDevice,
  kickDevice,
  listDevices,
  mintDevice,
  rotateDevice,
  type Device,
  type MintResponse,
} from '../../data'
import { ApiError } from '../../data/client'
import { Button } from '@/components/ui/button'

function formatRelative(value: string | null): string {
  if (!value) return '—'
  const d = new Date(value)
  if (Number.isNaN(d.getTime())) return value
  const secs = Math.max(0, Math.floor((Date.now() - d.getTime()) / 1000))
  if (secs < 60) return `${secs}s ago`
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`
  return `${Math.floor(secs / 86400)}d ago`
}

function deviceStatus(d: Device): { label: string; tone: 'active' | 'offline' | 'kicked' } {
  if (d.kicked_at) return { label: 'kicked', tone: 'kicked' }
  if (d.active) return { label: 'active', tone: 'active' }
  return { label: 'offline', tone: 'offline' }
}

export function DevicesSection() {
  const [devices, setDevices] = useState<Device[]>([])
  const [hasToken, setHasToken] = useState(false)
  const [loading, setLoading] = useState(true)
  const [refreshError, setRefreshError] = useState<string | null>(null)
  const [revealed, setRevealed] = useState<MintResponse | null>(null)
  const [revealLabel, setRevealLabel] = useState<'mint' | 'rotate' | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const [confirmRotate, setConfirmRotate] = useState(false)

  const refresh = useCallback(async () => {
    setLoading(true)
    try {
      const resp = await listDevices()
      setDevices(resp.devices)
      setHasToken(resp.has_token)
      setRefreshError(null)
    } catch (err) {
      setRefreshError(err instanceof Error ? err.message : 'failed to load devices')
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  async function handleMint() {
    setActionError(null)
    setBusy('mint')
    try {
      const resp = await mintDevice()
      setRevealed(resp)
      setRevealLabel('mint')
      // Token now exists server-side; reflect locally so the CTA flips
      // to "Rotate" once the user dismisses the reveal panel.
      setHasToken(true)
    } catch (err) {
      if (err instanceof ApiError && err.status === 410) {
        setActionError('A bridge token already exists. Use Rotate to mint a new one.')
      } else {
        setActionError(err instanceof Error ? err.message : 'failed to mint')
      }
    } finally {
      setBusy(null)
    }
  }

  async function handleRotate() {
    setActionError(null)
    setBusy('rotate')
    try {
      const resp = await rotateDevice()
      setRevealed(resp)
      setRevealLabel('rotate')
      setConfirmRotate(false)
      await refresh()
    } catch (err) {
      setActionError(err instanceof Error ? err.message : 'failed to rotate')
    } finally {
      setBusy(null)
    }
  }

  async function handleKick(machineId: string) {
    setActionError(null)
    setBusy(`kick:${machineId}`)
    try {
      await kickDevice(machineId)
      await refresh()
    } catch (err) {
      setActionError(err instanceof Error ? err.message : `failed to kick ${machineId}`)
    } finally {
      setBusy(null)
    }
  }

  async function handleForget(machineId: string) {
    setActionError(null)
    setBusy(`forget:${machineId}`)
    try {
      await forgetDevice(machineId)
      await refresh()
    } catch (err) {
      setActionError(err instanceof Error ? err.message : `failed to forget ${machineId}`)
    } finally {
      setBusy(null)
    }
  }

  async function copyScript() {
    if (!revealed) return
    try {
      await navigator.clipboard.writeText(revealed.script)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    } catch {
      setActionError('Could not copy to clipboard. Select and copy manually.')
    }
  }

  function dismissReveal() {
    setRevealed(null)
    setRevealLabel(null)
    setCopied(false)
    void refresh()
  }

  const hasAnyToken = hasToken

  return (
    <div className="settings-section">
      <div className="settings-section-header">
        <h2 className="settings-section-title">Devices</h2>
        <p className="settings-section-desc">
          Onboard a laptop, homelab box, or other machine to run agents on your
          behalf. Each machine connects via <code>chorus bridge</code> using a
          shared bearer token; the token is shown <strong>once</strong> at
          first-mint, so save it in a password manager.
        </p>
      </div>

      {actionError && (
        <div className="settings-banner settings-banner-error" role="alert">
          {actionError}
        </div>
      )}

      {revealed && (
        <div className="devices-reveal" role="dialog" aria-modal="false">
          <div className="devices-reveal-header">
            <h3 className="devices-reveal-title">
              {revealLabel === 'rotate' ? 'New token minted' : 'Save this script'}
            </h3>
            <p className="devices-reveal-warning">
              ⚠ You'll only see this once. Save it somewhere safe (password
              manager, snippet store) — there's no way to retrieve it later.
            </p>
          </div>
          <pre className="devices-reveal-script">{revealed.script}</pre>
          <div className="devices-reveal-actions">
            <Button onClick={copyScript} variant="default">
              {copied ? 'Copied!' : 'Copy script'}
            </Button>
            <Button onClick={dismissReveal} variant="outline">
              I've saved it — close
            </Button>
          </div>
        </div>
      )}

      <div className="devices-list-header">
        <h3 className="devices-list-title">Connected devices</h3>
        {!hasAnyToken && !revealed && (
          <Button onClick={handleMint} disabled={busy === 'mint'}>
            {busy === 'mint' ? 'Minting…' : 'Onboard a device'}
          </Button>
        )}
        {hasAnyToken && !revealed && !confirmRotate && (
          <Button variant="outline" onClick={() => setConfirmRotate(true)}>
            Rotate token
          </Button>
        )}
        {confirmRotate && !revealed && (
          <div className="devices-rotate-confirm">
            <span>Rotate disconnects every active device. Continue?</span>
            <Button onClick={handleRotate} disabled={busy === 'rotate'}>
              {busy === 'rotate' ? 'Rotating…' : 'Yes, rotate'}
            </Button>
            <Button variant="outline" onClick={() => setConfirmRotate(false)}>
              Cancel
            </Button>
          </div>
        )}
      </div>

      {loading && <p className="devices-empty">Loading…</p>}
      {!loading && refreshError && (
        <div className="settings-banner settings-banner-error" role="alert">
          {refreshError}
        </div>
      )}
      {!loading && !refreshError && devices.length === 0 && (
        <p className="devices-empty">
          No devices onboarded yet. Click <strong>Onboard a device</strong> to
          mint a bridge token and get the one-time setup script.
        </p>
      )}
      {!loading && devices.length > 0 && (
        <ul className="devices-list">
          {devices.map((d) => {
            const status = deviceStatus(d)
            return (
              <li key={d.machine_id} className="devices-row">
                <div className="devices-row-id">
                  <strong>{d.machine_id}</strong>
                  {d.hostname_hint && d.hostname_hint !== d.machine_id && (
                    <span className="devices-row-hint">({d.hostname_hint})</span>
                  )}
                </div>
                <span className={`devices-row-status devices-row-status-${status.tone}`}>
                  {status.label}
                </span>
                <span className="devices-row-seen">
                  last seen {formatRelative(d.last_seen_at)}
                </span>
                <div className="devices-row-actions">
                  {!d.kicked_at && (
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => handleKick(d.machine_id)}
                      disabled={busy === `kick:${d.machine_id}`}
                    >
                      Kick
                    </Button>
                  )}
                  {(d.kicked_at || !d.active) && (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => handleForget(d.machine_id)}
                      disabled={busy === `forget:${d.machine_id}`}
                      aria-label={`Forget ${d.machine_id}`}
                    >
                      <Trash2 size={14} />
                      Forget
                    </Button>
                  )}
                </div>
              </li>
            )
          })}
        </ul>
      )}
    </div>
  )
}
