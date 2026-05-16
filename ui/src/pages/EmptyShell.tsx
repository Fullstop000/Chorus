import type { ReactNode } from 'react'

/**
 * Empty-state panel used by RootRedirect, MainPanel's "select a channel"
 * fallback, and the not-found leaf. Class `empty-state` is what QA's
 * `waitForAppReady` polls for to know the shell is interactive.
 */
export function EmptyShell({
  label,
  icon = '[chorus::idle]',
  extra,
}: {
  label: string
  icon?: string
  extra?: ReactNode
}): JSX.Element {
  return (
    <div
      className="empty-state"
      style={{
        flex: 1,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        flexDirection: 'column',
        gap: 8,
        color: 'var(--color-muted-foreground)',
      }}
    >
      <h1 className="sr-only">Chorus — {label}</h1>
      <span className="empty-state-icon">{icon}</span>
      <span>{label}</span>
      {extra}
    </div>
  )
}
