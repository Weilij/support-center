// Teams lookup store (shared by Phase 1.2 assign/transfer dropdowns and later
// team-management screens). Holds the team directory the user can route to.

import { get } from '../api/client'
import { Store } from './store'

export interface Team {
  id: number
  name: string
  description?: string | null
  isActive?: boolean
  memberCount?: number
  [key: string]: unknown
}

interface TeamsState {
  items: Team[]
  busy: boolean
  error: string | null
}

const FRESH_MS = 120_000

export const teamsStore = new Store<TeamsState>({ items: [], busy: false, error: null })

export async function loadTeams(force = false): Promise<void> {
  if (!force && teamsStore.isFresh(FRESH_MS) && teamsStore.get().items.length > 0) return
  teamsStore.update((s) => ({ ...s, busy: true, error: null }))
  const resp = await get<Team[] | { items?: Team[] }>('/api/teams')
  if (resp.success && resp.data !== undefined) {
    const items = Array.isArray(resp.data) ? resp.data : (resp.data.items ?? [])
    teamsStore.set({ items, busy: false, error: null })
    teamsStore.markFresh()
  } else {
    teamsStore.update((s) => ({ ...s, busy: false, error: resp.message ?? '載入失敗' }))
  }
}
