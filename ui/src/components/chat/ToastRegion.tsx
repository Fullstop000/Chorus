interface ToastRegionProps {
  toasts: Array<{
    id: string
    message: string
    level?: 'error' | 'warning' | 'info'
  }>
  onDismiss: (id: string) => void
}

function levelBorder(level?: 'error' | 'warning' | 'info'): string | undefined {
  if (level === 'error') return '3px solid var(--color-destructive)'
  if (level === 'warning') return '3px solid var(--color-destructive)'
  return undefined
}

export function ToastRegion({ toasts, onDismiss }: ToastRegionProps) {
  if (toasts.length === 0) return null

  return (
    <div
      style={{
        position: 'fixed',
        right: 18,
        bottom: 18,
        display: 'flex',
        flexDirection: 'column',
        gap: 8,
        zIndex: 40,
      }}
    >
      {toasts.map((toast) => (
        <div
          key={toast.id}
          className="toast-card"
          style={{
            minWidth: 240,
            maxWidth: 320,
            padding: '10px 12px',
            border: '1px solid var(--color-border)',
            borderLeft: levelBorder(toast.level),
            background: 'var(--color-card)',
            color: 'var(--color-foreground)',
            boxShadow: '0 6px 16px rgba(0, 0, 0, 0.08)',
            display: 'flex',
            alignItems: 'flex-start',
            justifyContent: 'space-between',
            gap: 12,
          }}
        >
          <span style={{ fontSize: 13, lineHeight: 1.4 }}>{toast.message}</span>
          <button
            type="button"
            onClick={() => onDismiss(toast.id)}
            style={{
              border: 'none',
              background: 'transparent',
              color: 'var(--color-muted-foreground)',
              cursor: 'pointer',
              fontSize: 16,
              lineHeight: 1,
            }}
            aria-label="Dismiss toast"
          >
            ×
          </button>
        </div>
      ))}
    </div>
  )
}
