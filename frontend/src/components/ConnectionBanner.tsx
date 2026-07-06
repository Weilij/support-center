// Slim connection-status banner (Phase 2.2): warns while the realtime socket
// is reconnecting and briefly confirms once it recovers. Silent otherwise.
import { useEffect, useRef, useState } from 'react'

import { onConnectionChange } from '../realtime/client'

export function ConnectionBanner() {
  const [reconnecting, setReconnecting] = useState(false)
  const [recovered, setRecovered] = useState(false)
  const wasReconnecting = useRef(false)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    const off = onConnectionChange((state) => {
      setReconnecting(state === 'reconnecting')
      if (state === 'reconnecting') {
        wasReconnecting.current = true
        setRecovered(false)
      } else if (state === 'connected' && wasReconnecting.current) {
        wasReconnecting.current = false
        setRecovered(true)
        if (timer.current) clearTimeout(timer.current)
        timer.current = setTimeout(() => setRecovered(false), 2500)
      }
    })
    return () => {
      off()
      if (timer.current) clearTimeout(timer.current)
    }
  }, [])

  if (reconnecting) {
    return (
      <div className="cs-conn-banner cs-conn-banner--warn" role="status">
        連線中斷，正在重新連線…
      </div>
    )
  }
  if (recovered) {
    return (
      <div className="cs-conn-banner cs-conn-banner--ok" role="status">
        連線已恢復
      </div>
    )
  }
  return null
}
