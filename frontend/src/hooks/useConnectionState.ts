// React binding for the realtime client's connection state (Phase 2.2).
import { useEffect, useState } from 'react'

import { getConnectionState, onConnectionChange, type ConnectionState } from '../realtime/client'

export function useConnectionState(): ConnectionState {
  const [state, setState] = useState<ConnectionState>(getConnectionState)
  useEffect(() => onConnectionChange(setState), [])
  return state
}
