import type { ReactNode } from 'react'

export function PageHeader({ title, subtitle, actions }: { title: ReactNode; subtitle?: ReactNode; actions?: ReactNode }) {
  return (
    <div style={{ display: 'flex', alignItems: 'flex-end', gap: 16, marginBottom: 'var(--sp-5)' }}>
      <div>
        {/* 21px/700 per design spec, letter-spacing -.02em from h1 global style */}
        <h1 style={{ margin: 0, fontSize: 21, fontWeight: 700, letterSpacing: '-.02em' }}>{title}</h1>
        {subtitle && <div style={{ color: 'var(--muted)', fontSize: 13, marginTop: 2 }}>{subtitle}</div>}
      </div>
      {actions && <div style={{ marginLeft: 'auto', display: 'flex', gap: 8, alignItems: 'center' }}>{actions}</div>}
    </div>
  )
}
