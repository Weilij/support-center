// Avatar — round avatar: a photo when `src` is given, else initials with a
// hashed color from AV_COLORS. A broken/expired image falls back to initials.
import { useEffect, useState } from 'react'

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
  src?: string | null
  size?: 'sm' | 'md' | 'lg'
  ring?: boolean
}

export function Avatar({ name, src, size = 'md', ring = false }: AvatarProps) {
  const [failed, setFailed] = useState(false)
  useEffect(() => {
    setFailed(false)
  }, [src])
  const cls = `cs-av cs-av-${size}${ring ? ' cs-av-ring' : ''}`
  if (src && !failed) {
    return (
      <img
        className={cls}
        src={src}
        alt={name}
        onError={() => setFailed(true)}
        style={{ objectFit: 'cover' }}
      />
    )
  }
  return (
    <span className={cls} style={{ background: avColor(name) }}>
      {name.slice(-2)}
    </span>
  )
}
