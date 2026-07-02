import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('../../stores/customers', () => ({
  loadCustomerDetail: vi.fn().mockResolvedValue(null),
  loadCustomerTags: vi.fn().mockResolvedValue([]),
}))

import { CustomerPanel } from './CustomerPanel'

afterEach(() => cleanup())

describe('CustomerPanel 指派團隊', () => {
  it('shows the assigned team name', () => {
    render(<CustomerPanel meta={{ platform: 'line', customerName: 'Alice', teamName: 'ＷＴＯ' }} />)
    expect(screen.getByText('指派團隊')).toBeTruthy()
    expect(screen.getByText('ＷＴＯ')).toBeTruthy()
  })

  it('shows 無 when the conversation has no team', () => {
    render(<CustomerPanel meta={{ platform: 'line', customerName: 'Alice', teamName: null }} />)
    expect(screen.getByText('指派團隊')).toBeTruthy()
    expect(screen.getByText('無')).toBeTruthy()
  })
})
