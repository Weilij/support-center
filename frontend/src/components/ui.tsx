// Small presentational primitives (Epic 0 foundation): StatCard for dashboard
// tiles, StatusPill/Badge for statuses, FilterBar to hold search + filters,
// EmptyState, and a lightweight Toast surface for action feedback.
// Restyled to clean-light tokens: solid white surfaces, --line borders, no blur.

import { useEffect } from 'react'
import type { ReactNode } from 'react'

export function StatCard({ label, value, hint }: { label: ReactNode; value: ReactNode; hint?: ReactNode }) {
  return (
    <div
      style={{
        background: 'var(--surface)',
        border: '1px solid var(--line)',
        borderRadius: 'var(--radius-lg)',
        boxShadow: 'var(--shadow-sm)',
        padding: 'var(--sp-4)',
        minWidth: 140,
      }}
    >
      <div style={{ fontSize: 13, color: 'var(--muted)', fontWeight: 500 }}>{label}</div>
      <div style={{ fontSize: 30, fontWeight: 700, letterSpacing: '-.025em', lineHeight: 1, margin: '8px 0 0' }}>{value}</div>
      {hint && <div style={{ fontSize: 12, color: 'var(--muted)', marginTop: 4 }}>{hint}</div>}
    </div>
  )
}

// StatusPill — renders as .cs-status: colored dot + label text
// dot color: ok (online/active/open/ok), warn (away/pending/busy/warn), muted-2 (offline/closed/off)
type StatusKey = 'ok' | 'warn' | 'off'

const STATUS_MAP: Record<string, StatusKey> = {
  online: 'ok', active: 'ok', open: 'ok', ok: 'ok',
  away: 'warn', pending: 'warn', busy: 'warn', warn: 'warn',
  offline: 'off', closed: 'off', inactive: 'off', off: 'off',
  failed: 'off', error: 'off',
}

const STATUS_DOT_STYLE: Record<StatusKey, React.CSSProperties> = {
  ok: {
    background: 'var(--ok)',
    boxShadow: '0 0 0 3px color-mix(in oklch, var(--ok) 22%, transparent)',
  },
  warn: {
    background: 'var(--warn)',
    boxShadow: '0 0 0 3px color-mix(in oklch, var(--warn) 22%, transparent)',
  },
  off: {
    background: 'var(--muted-2)',
  },
}

export function StatusPill({ status, label }: { status: string; label?: ReactNode }) {
  const key: StatusKey = STATUS_MAP[status?.toLowerCase()] ?? 'off'
  return (
    <span
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 6,
        fontSize: 12,
        fontWeight: 600,
        whiteSpace: 'nowrap',
      }}
    >
      <span
        style={{
          width: 7,
          height: 7,
          borderRadius: '50%',
          flexShrink: 0,
          ...STATUS_DOT_STYLE[key],
        }}
      />
      {label ?? status}
    </span>
  )
}

// Badge — renders like .cs-chip: subtle line-2 bg, ink-2 text, rounded
export function Badge({ children, color }: { children: ReactNode; color?: string }) {
  const isBlue = color === 'blue'
  return (
    <span
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 5,
        padding: '4px 10px',
        borderRadius: 7,
        fontSize: 11.5,
        fontWeight: 600,
        background: isBlue ? 'var(--blue-50)' : 'var(--line-2)',
        color: isBlue ? 'var(--blue-700)' : 'var(--ink-2)',
        whiteSpace: 'nowrap',
      }}
    >
      {children}
    </span>
  )
}

// FilterBar — light, white/--bg surface, --line border, --radius
export function FilterBar({ children }: { children: ReactNode }) {
  return (
    <div
      style={{
        display: 'flex',
        gap: 10,
        alignItems: 'center',
        flexWrap: 'wrap',
        margin: '12px 0',
        background: 'var(--surface)',
        border: '1px solid var(--line)',
        borderRadius: 'var(--radius)',
        padding: 'var(--sp-3) var(--sp-4)',
      }}
    >
      {children}
    </div>
  )
}

export function EmptyState({ message = '沒有資料' }: { message?: ReactNode }) {
  return <p style={{ color: 'var(--muted)', padding: 24, textAlign: 'center' }}>{message}</p>
}

// Toast — solid dark slate (--ink bg, white text), no blur
export function Toast({ message, onDismiss, ms = 3000 }: { message: string | null; onDismiss: () => void; ms?: number }) {
  useEffect(() => {
    if (!message) return
    const id = setTimeout(onDismiss, ms)
    return () => clearTimeout(id)
  }, [message, ms, onDismiss])
  if (!message) return null
  return (
    <div
      role="status"
      style={{
        position: 'fixed',
        bottom: 24,
        left: '50%',
        transform: 'translateX(-50%)',
        background: 'var(--ink)',
        color: 'white',
        padding: '10px 18px',
        borderRadius: 8,
        boxShadow: 'var(--shadow)',
        zIndex: 2000,
        fontSize: 14,
        fontWeight: 500,
      }}
    >
      {message}
    </div>
  )
}
