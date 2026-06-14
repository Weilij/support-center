// Small presentational primitives (Epic 0 foundation): StatCard for dashboard
// tiles, StatusPill/Badge for statuses, FilterBar to hold search + filters,
// EmptyState, and a lightweight Toast surface for action feedback.

import { useEffect } from 'react'
import type { ReactNode } from 'react'

export function StatCard({ label, value, hint }: { label: ReactNode; value: ReactNode; hint?: ReactNode }) {
  return (
    <div
      style={{
        background: 'var(--surface)',
        backdropFilter: 'blur(var(--blur))',
        WebkitBackdropFilter: 'blur(var(--blur))',
        border: '1px solid var(--surface-border)',
        borderRadius: 'var(--radius)',
        boxShadow: 'var(--shadow)',
        padding: 'var(--sp-4)',
        minWidth: 140,
      }}
    >
      <div style={{ fontSize: 13, color: 'var(--muted)' }}>{label}</div>
      <div style={{ fontSize: 26, fontWeight: 700, margin: '4px 0' }}>{value}</div>
      {hint && <div style={{ fontSize: 12, color: 'var(--muted)' }}>{hint}</div>}
    </div>
  )
}

const STATUS_COLORS: Record<string, string> = {
  online: '#16A34A',
  active: '#16A34A',
  open: '#16A34A',
  away: '#F59E0B',
  pending: '#F59E0B',
  busy: '#F59E0B',
  offline: '#9CA3AF',
  closed: '#9CA3AF',
  inactive: '#9CA3AF',
  failed: '#DC2626',
  error: '#DC2626',
}

export function StatusPill({ status, label }: { status: string; label?: ReactNode }) {
  const color = STATUS_COLORS[status?.toLowerCase()] ?? '#3B82F6'
  return (
    <span
      style={{
        background: color,
        color: 'white',
        borderRadius: 999,
        padding: '2px 10px',
        fontSize: 12,
        whiteSpace: 'nowrap',
      }}
    >
      {label ?? status}
    </span>
  )
}

export function Badge({ children, color = '#3B82F6' }: { children: ReactNode; color?: string }) {
  return (
    <span style={{ background: color, color: 'white', borderRadius: 8, padding: '2px 10px', fontSize: 12 }}>
      {children}
    </span>
  )
}

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
        backdropFilter: 'blur(var(--blur))',
        WebkitBackdropFilter: 'blur(var(--blur))',
        border: '1px solid var(--surface-border)',
        borderRadius: 'var(--radius)',
        padding: 'var(--sp-3) var(--sp-4)',
      }}
    >
      {children}
    </div>
  )
}

export function EmptyState({ message = '沒有資料' }: { message?: ReactNode }) {
  return <p style={{ color: '#888', padding: 24, textAlign: 'center' }}>{message}</p>
}

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
        background: 'rgba(17,17,17,.82)',
        backdropFilter: 'blur(12px)',
        WebkitBackdropFilter: 'blur(12px)',
        color: 'white',
        padding: '10px 18px',
        borderRadius: 8,
        boxShadow: '0 4px 16px rgba(0,0,0,0.25)',
        zIndex: 2000,
      }}
    >
      {message}
    </div>
  )
}
