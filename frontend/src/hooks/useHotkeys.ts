import { useEffect } from 'react'

// Combo grammar: optional "mod+" (meta on mac / ctrl elsewhere) + key name
// (case-insensitive), e.g. "mod+k", "mod+enter", "escape".
export function matchHotkey(e: KeyboardEvent, combo: string): boolean {
  const parts = combo.toLowerCase().split('+')
  const key = parts[parts.length - 1]
  const needMod = parts.includes('mod')
  const hasMod = e.metaKey || e.ctrlKey
  if (needMod !== hasMod) return false
  return e.key.toLowerCase() === key
}

export function useHotkeys(map: Record<string, (e: KeyboardEvent) => void>): void {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      for (const [combo, handler] of Object.entries(map)) {
        if (matchHotkey(e, combo)) { handler(e); return }
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [map])
}
