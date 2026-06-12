// Reactive state containers (CRD §8.1): a slice holds data + busy/error,
// supports subscriptions, time-based cache freshness, and the
// optimistic-update pattern (apply -> server call -> reconcile | rollback).

import { useSyncExternalStore } from 'react'

export class Store<T> {
  private state: T
  private listeners = new Set<() => void>()
  private fetchedAt = 0

  constructor(initial: T) {
    this.state = initial
  }

  get(): T {
    return this.state
  }

  set(next: T) {
    this.state = next
    this.listeners.forEach((fn) => fn())
  }

  update(patch: (current: T) => T) {
    this.set(patch(this.state))
  }

  /// Time-based cache freshness (CRD §8.1 purpose).
  markFresh() {
    this.fetchedAt = Date.now()
  }
  isFresh(maxAgeMs: number): boolean {
    return Date.now() - this.fetchedAt < maxAgeMs
  }
  invalidate() {
    this.fetchedAt = 0
  }

  subscribe = (fn: () => void) => {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }

  /// Optimistic update: apply locally now, reconcile with the authoritative
  /// response on success, revert to the prior snapshot on failure (CRD §8.1).
  async optimistic<R>(
    apply: (current: T) => T,
    call: () => Promise<{ success: boolean; data?: R }>,
    reconcile?: (current: T, data: R) => T,
  ): Promise<boolean> {
    const snapshot = this.state
    this.set(apply(this.state))
    try {
      const resp = await call()
      if (!resp.success) {
        this.set(snapshot) // rollback
        return false
      }
      if (reconcile && resp.data !== undefined) {
        this.set(reconcile(this.state, resp.data))
      }
      return true
    } catch {
      this.set(snapshot)
      return false
    }
  }
}

export function useStore<T>(store: Store<T>): T {
  return useSyncExternalStore(store.subscribe, () => store.get(), () => store.get())
}
