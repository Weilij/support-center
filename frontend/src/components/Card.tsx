import type { ReactNode } from 'react'

// Clean-light card surface: solid white, 1px --line border, --radius-lg, --shadow-sm
const cardBase: React.CSSProperties = {
  background: 'var(--surface)',
  border: '1px solid var(--line)',
  borderRadius: 'var(--radius-lg)',
  boxShadow: 'var(--shadow-sm)',
}

export function Card({ title, actions, children, style }: { title?: ReactNode; actions?: ReactNode; children: ReactNode; style?: React.CSSProperties }) {
  return (
    <section style={{ ...cardBase, padding: 'var(--sp-5)', ...style }}>
      {(title || actions) && (
        <div style={{ display: 'flex', alignItems: 'center', marginBottom: 'var(--sp-3)' }}>
          {title && (
            <h3 style={{ margin: 0, fontSize: 15, fontWeight: 700, letterSpacing: '-.01em' }}>
              {title}
            </h3>
          )}
          {actions && <div style={{ marginLeft: 'auto' }}>{actions}</div>}
        </div>
      )}
      {children}
    </section>
  )
}

export const Panel = Card

export function StatGrid({ children, min = 160, style }: { children: ReactNode; min?: number; style?: React.CSSProperties }) {
  return (
    <div style={{ display: 'grid', gridTemplateColumns: `repeat(auto-fill, minmax(${min}px, 1fr))`, gap: 'var(--sp-4)', ...style }}>
      {children}
    </div>
  )
}
