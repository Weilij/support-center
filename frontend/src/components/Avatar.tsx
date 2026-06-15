// Avatar — round initials avatar, hashed color from AV_COLORS palette.
// Ported from handoff assets/components.jsx Avatar + avColor.
// Prop API: { name: string; size?: 'sm'|'md'|'lg'; ring?: boolean }
// Sizes: sm=30px, md=38px, lg=46px via .cs-av .cs-av-{sm|md|lg} classes.

export const AV_COLORS = [
  '#0284c7', '#0d9488', '#7c3aed', '#db2777',
  '#ea580c', '#4f46e5', '#0891b2', '#65a30d',
]

export function avColor(name: string): string {
  let h = 0
  for (const ch of name) h = ((h * 31 + ch.charCodeAt(0)) >>> 0)
  return AV_COLORS[h % AV_COLORS.length]
}

export interface AvatarProps {
  name: string
  size?: 'sm' | 'md' | 'lg'
  ring?: boolean
}

export function Avatar({ name, size = 'md', ring = false }: AvatarProps) {
  const initials = name.slice(-2)
  return (
    <span
      className={`cs-av cs-av-${size}${ring ? ' cs-av-ring' : ''}`}
      style={{ background: avColor(name) }}
    >
      {initials}
    </span>
  )
}
