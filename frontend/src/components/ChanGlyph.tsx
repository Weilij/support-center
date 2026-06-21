// ChanGlyph — round colored badge with channel letter, monospace font.
// Ported from handoff assets/components.jsx ChanGlyph.
// Prop API: { type: 'chat'|'line'|'wa'|'fb'|'ig'|'shopee'; size?: number }

import { CHANNELS } from './channels'

export interface ChanGlyphProps {
  type: 'chat' | 'line' | 'wa' | 'fb' | 'ig' | 'shopee'
  size?: number
}

const GLYPH_LABEL: Record<string, string> = {
  chat: 'C',
  line: 'L',
  wa:   'W',
  fb:   'M',
  ig:   'IG',
  shopee: 'S',
}

export function ChanGlyph({ type, size = 18 }: ChanGlyphProps) {
  const c = CHANNELS[type] ?? CHANNELS.chat
  const label = GLYPH_LABEL[type] ?? 'C'
  const background = type === 'ig' ? 'var(--brand-ig-gradient)' : c.color
  return (
    <span
      style={{
        width: size,
        height: size,
        borderRadius: '50%',
        background,
        color: '#fff',
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        fontSize: type === 'ig' ? size * 0.4 : size * 0.52,
        fontWeight: 700,
        fontFamily: 'var(--mono)',
        flexShrink: 0,
      }}
    >
      {label}
    </span>
  )
}
