import { act, renderHook } from '@testing-library/react'
import { beforeEach, describe, expect, it } from 'vitest'

import { useCollapsed } from '../hooks/useCollapsed'

describe('useCollapsed', () => {
  beforeEach(() => localStorage.clear())

  it('uses the default when nothing is stored', () => {
    const { result } = renderHook(() => useCollapsed('k1', true))
    expect(result.current[0]).toBe(true)
  })

  it('reads a stored value over the default', () => {
    localStorage.setItem('collapsed.k2', 'false')
    const { result } = renderHook(() => useCollapsed('k2', true))
    expect(result.current[0]).toBe(false)
  })

  it('toggle flips and persists', () => {
    const { result } = renderHook(() => useCollapsed('k3', true))
    act(() => result.current[1]())
    expect(result.current[0]).toBe(false)
    expect(localStorage.getItem('collapsed.k3')).toBe('false')
    act(() => result.current[1]())
    expect(result.current[0]).toBe(true)
    expect(localStorage.getItem('collapsed.k3')).toBe('true')
  })
})
