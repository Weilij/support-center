// Chip — .cs-chip generic chip (optional tone='blue').
// Tag — colored tag from the handoff tag palette; unknown labels fall back to line-2/ink-2.

import type { ReactNode } from 'react'

export interface ChipProps {
  children: ReactNode
  tone?: 'blue'
}

export function Chip({ children, tone }: ChipProps) {
  const isBlue = tone === 'blue'
  return (
    <span
      className={`cs-chip${isBlue ? ' cs-chip--blue' : ''}`}
    >
      {children}
    </span>
  )
}

// TAG_COLORS: [bg, text] for each label from the design handoff.
export const TAG_COLORS: Record<string, [string, string]> = {
  '訂單':  ['#e0f2fe', '#0369a1'],
  '優惠':  ['#ede9fe', '#5b21b6'],
  '退換貨': ['#ffedd5', '#9a3412'],
  '客訴':  ['#ffe4e6', '#9f1239'],
  '會員':  ['#dcfce7', '#166534'],
  '已結案': ['#f1f5f9', '#64748b'],
  '運送中': ['#fef3c7', '#b45309'],
}

export interface TagProps {
  label: string
}

export function Tag({ label }: TagProps) {
  const colors = TAG_COLORS[label]
  return (
    <span
      className="cs-tag"
      style={
        colors
          ? { background: colors[0], color: colors[1] }
          : { background: 'var(--line-2)', color: 'var(--ink-2)' }
      }
    >
      {label}
    </span>
  )
}
