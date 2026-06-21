import { describe, expect, it } from 'vitest'

import { movedIds } from '../lib/flip'

describe('movedIds', () => {
  it('returns ids whose top changed between snapshots', () => {
    const prev = new Map([['a', 0], ['b', 60], ['c', 120]])
    const next = new Map([['a', 60], ['b', 0], ['c', 120]])
    expect(movedIds(prev, next).sort()).toEqual(['a', 'b'])
  })
  it('ignores ids missing from either snapshot', () => {
    const prev = new Map([['a', 0]])
    const next = new Map([['a', 0], ['d', 60]])
    expect(movedIds(prev, next)).toEqual([])
  })
})
