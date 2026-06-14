import type { ReactNode } from 'react'

const glass: React.CSSProperties = {
  background: 'var(--surface)',
  backdropFilter: 'blur(var(--blur))',
  WebkitBackdropFilter: 'blur(var(--blur))',
  border: '1px solid var(--surface-border)',
  borderRadius: 'var(--radius)',
  boxShadow: 'var(--shadow)',
}

export function Card({ title, actions, children, style }: { title?: ReactNode; actions?: ReactNode; children: ReactNode; style?: React.CSSProperties }) {
  return (
    <section style={{ ...glass, padding: 'var(--sp-5)', ...style }}>
      {(title || actions) && (
        <div style={{ display: 'flex', alignItems: 'center', marginBottom: 'var(--sp-3)' }}>
          {title && <h3 style={{ margin: 0 }}>{title}</h3>}
          {actions && <div style={{ marginLeft: 'auto' }}>{actions}</div>}
        </div>
      )}
      {children}
    </section>
  )
}

export const Panel = Card

export function StatGrid({ children, min = 160 }: { children: ReactNode; min?: number }) {
  return (
    <div style={{ display: 'grid', gridTemplateColumns: `repeat(auto-fill, minmax(${min}px, 1fr))`, gap: 'var(--sp-4)' }}>
      {children}
    </div>
  )
}
