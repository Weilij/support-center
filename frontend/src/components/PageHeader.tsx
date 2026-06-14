import type { ReactNode } from 'react'

export function PageHeader({ title, subtitle, actions }: { title: ReactNode; subtitle?: ReactNode; actions?: ReactNode }) {
  return (
    <div style={{ display: 'flex', alignItems: 'flex-end', gap: 16, marginBottom: 'var(--sp-5)' }}>
      <div>
        <h1 style={{ margin: 0 }}>{title}</h1>
        {subtitle && <div style={{ color: 'var(--muted)', fontSize: 14, marginTop: 4 }}>{subtitle}</div>}
      </div>
      {actions && <div style={{ marginLeft: 'auto', display: 'flex', gap: 8, alignItems: 'center' }}>{actions}</div>}
    </div>
  )
}
