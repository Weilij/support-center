// Store layer contract (CRD §8.1): optimistic apply -> reconcile | rollback.

import { describe, expect, it } from 'vitest'

import { Store } from '../stores/store'

interface S {
  items: string[]
}

describe('Store.optimistic', () => {
  it('keeps the applied change and reconciles on success', async () => {
    const store = new Store<S>({ items: ['a'] })
    const ok = await store.optimistic(
      (s) => ({ items: [...s.items, 'b-temp'] }),
      async () => ({ success: true, data: 'b-confirmed' }),
      (s, data) => ({ items: s.items.map((x) => (x === 'b-temp' ? data : x)) }),
    )
    expect(ok).toBe(true)
    expect(store.get().items).toEqual(['a', 'b-confirmed'])
  })

  it('reverts to the prior snapshot when the server refuses', async () => {
    const store = new Store<S>({ items: ['a'] })
    const ok = await store.optimistic(
      (s) => ({ items: [...s.items, 'b'] }),
      async () => ({ success: false }),
    )
    expect(ok).toBe(false)
    expect(store.get().items).toEqual(['a'])
  })

  it('reverts when the server call throws', async () => {
    const store = new Store<S>({ items: ['a'] })
    const ok = await store.optimistic(
      (s) => ({ items: [] }),
      async () => {
        throw new Error('network')
      },
    )
    expect(ok).toBe(false)
    expect(store.get().items).toEqual(['a'])
  })

  it('notifies subscribers and tracks cache freshness', () => {
    const store = new Store<number>(0)
    let seen = 0
    const off = store.subscribe(() => {
      seen += 1
    })
    store.set(1)
    store.update((n) => n + 1)
    off()
    store.set(99)
    expect(seen).toBe(2)
    expect(store.isFresh(1000)).toBe(false)
    store.markFresh()
    expect(store.isFresh(1000)).toBe(true)
    store.invalidate()
    expect(store.isFresh(1000)).toBe(false)
  })
})
