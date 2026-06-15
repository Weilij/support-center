// ChanGlyph — round colored badge with channel letter, monospace font.
// Ported from handoff assets/components.jsx ChanGlyph.
// Prop API: { type: 'chat'|'line'|'wa'|'fb'; size?: number }

import { CHANNELS } from './channels'

export interface ChanGlyphProps {
  type: 'chat' | 'line' | 'wa' | 'fb'
  size?: number
}

const GLYPH_LABEL: Record<string, string> = {
  chat: 'C',
  line: 'L',
  wa:   'W',
  fb:   'M',
}

export function ChanGlyph({ type, size = 18 }: ChanGlyphProps) {
  const c = CHANNELS[type] ?? CHANNELS.chat
  const label = GLYPH_LABEL[type] ?? 'C'
  return (
    <span
      style={{
        width: size,
        height: size,
        borderRadius: '50%',
        background: c.color,
        color: '#fff',
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        fontSize: size * 0.52,
        fontWeight: 700,
        fontFamily: 'var(--mono)',
        flexShrink: 0,
      }}
    >
      {label}
    </span>
  )
}
