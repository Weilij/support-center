// Bar — channel/progress bar following .cs-bar from the handoff.
// Renders a track with an inner <i> whose width = pct%, filled with given color.

export interface BarProps {
  pct: number
  color: string
}

export function Bar({ pct, color }: BarProps) {
  return (
    <div className="cs-bar">
      <i style={{ width: `${Math.min(100, Math.max(0, pct))}%`, background: color }} />
    </div>
  )
}
