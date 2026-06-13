// Customers directory store (Phase 1.1). The list endpoint returns every
// visible customer in one shot (no server-side paging/search — CRD §3.1), so
// the store caches the full set and the screen filters/paginates client-side.

import { get } from '../api/client'
import { Store } from './store'

export interface Customer {
  id: number
  platform: string
  platform_user_id: string
  display_name?: string | null
  avatar_url?: string | null
  email?: string | null
  phone?: string | null
  source_team_id?: number | null
  metadata?: unknown
  created_at?: string
  updated_at?: string | null
  [key: string]: unknown
}

export interface CustomerTag {
  id: number
  name: string
  color?: string
  description?: string
}

export interface CustomerConversation {
  id: string
  status: string
  priority: string
  team_id?: number | null
  last_message_at?: string | null
  created_at?: string
}

export interface CustomerDetail {
  customer: Customer
  conversations: CustomerConversation[]
  conversationCount: number
}

interface CustomersState {
  items: Customer[]
  busy: boolean
  error: string | null
}

const FRESH_MS = 60_000

export const customersStore = new Store<CustomersState>({
  items: [],
  busy: false,
  error: null,
})

export async function loadCustomers(force = false): Promise<void> {
  if (!force && customersStore.isFresh(FRESH_MS) && customersStore.get().items.length > 0) return
  customersStore.update((s) => ({ ...s, busy: true, error: null }))
  const resp = await get<{ customers?: Customer[] }>('/api/customers')
  if (resp.success && resp.data) {
    customersStore.set({ items: resp.data.customers ?? [], busy: false, error: null })
    customersStore.markFresh()
  } else {
    customersStore.update((s) => ({ ...s, busy: false, error: resp.message ?? '載入失敗' }))
  }
}

export async function loadCustomerDetail(id: number): Promise<CustomerDetail | null> {
  const resp = await get<CustomerDetail>(`/api/customers/${id}`)
  return resp.success && resp.data ? resp.data : null
}

export async function loadCustomerTags(id: number): Promise<CustomerTag[]> {
  const resp = await get<CustomerTag[]>(`/api/customers/${id}/tags`)
  return resp.success && Array.isArray(resp.data) ? resp.data : []
}
