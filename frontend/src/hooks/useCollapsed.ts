import { useCallback, useState } from 'react'

// Persisted open/closed (collapsed) boolean keyed in localStorage under
// `collapsed.<key>`. Returns [collapsed, toggle]. `defaultCollapsed` applies
// only when nothing is stored yet.
export function useCollapsed(
  key: string,
  defaultCollapsed: boolean,
): [boolean, () => void] {
  const storageKey = `collapsed.${key}`
  const [collapsed, setCollapsed] = useState<boolean>(() => {
    const v = localStorage.getItem(storageKey)
    return v === null ? defaultCollapsed : v === 'true'
  })
  const toggle = useCallback(() => {
    setCollapsed((prev) => {
      const next = !prev
      localStorage.setItem(storageKey, String(next))
      return next
    })
  }, [storageKey])
  return [collapsed, toggle]
}
