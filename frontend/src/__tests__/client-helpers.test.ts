// Epic 0 client helpers: query-string building and list-envelope unwrapping
// tolerate the backend's inconsistent shapes, so test those contracts.

import { describe, expect, it } from 'vitest'

import { buildQuery, unwrapList } from '../api/client'

describe('buildQuery', () => {
  it('skips null/undefined/empty and encodes the rest', () => {
    expect(buildQuery({ page: 2, search: 'a b', skip: '', n: null, u: undefined })).toBe(
      '?page=2&search=a+b',
    )
  })

  it('returns empty string (not "?") when nothing is present', () => {
    expect(buildQuery({ a: '', b: null })).toBe('')
  })

  it('serialises booleans and numbers', () => {
    expect(buildQuery({ active: true, count: 0 })).toBe('?active=true&count=0')
  })
})

describe('unwrapList', () => {
  it('accepts a bare array in data', () => {
    const r = unwrapList({ success: true, data: [1, 2, 3] }, 1)
    expect(r.items).toEqual([1, 2, 3])
    expect(r.total).toBe(3)
  })

  it('accepts { items } with pagination meta', () => {
    const r = unwrapList(
      { success: true, data: { items: ['a'] }, pagination: { total: 42, page: 3 } },
      3,
    )
    expect(r.items).toEqual(['a'])
    expect(r.total).toBe(42)
    expect(r.page).toBe(3)
  })

  it('falls back to top-level total and item count', () => {
    const r = unwrapList({ success: true, data: { rows: ['x', 'y'] } }, 1)
    expect(r.items).toEqual(['x', 'y'])
    expect(r.total).toBe(2)
  })

  it('rejects malformed list and pagination shapes', () => {
    const r = unwrapList(
      {
        success: true,
        data: { items: 'not-an-array' },
        pagination: { total: 'many', page: 'later' },
      },
      4,
    )
    expect(r.items).toEqual([])
    expect(r.total).toBe(0)
    expect(r.page).toBe(4)
  })
})
