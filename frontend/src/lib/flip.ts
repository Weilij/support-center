// FLIP (First-Last-Invert-Play) helpers for the conversation list. The pure
// position-diff (`movedIds`) is unit-tested; `animateMoves` does the DOM play.

export type PosMap = Map<string, number>

export function recordPositions(container: HTMLElement): PosMap {
  const map: PosMap = new Map()
  container.querySelectorAll<HTMLElement>('[data-flip-id]').forEach((el) => {
    map.set(el.dataset.flipId!, el.getBoundingClientRect().top)
  })
  return map
}

export function movedIds(prev: PosMap, next: PosMap): string[] {
  const ids: string[] = []
  for (const [id, top] of next) {
    if (prev.has(id) && prev.get(id) !== top) ids.push(id)
  }
  return ids
}

export function animateMoves(container: HTMLElement, prev: PosMap, durationMs = 220): void {
  const next = recordPositions(container)
  for (const id of movedIds(prev, next)) {
    const el = container.querySelector<HTMLElement>(`[data-flip-id="${id}"]`)
    if (!el) continue
    const delta = (prev.get(id) ?? 0) - (next.get(id) ?? 0)
    el.style.transition = 'none'
    el.style.transform = `translateY(${delta}px)`
    void el.offsetHeight // force reflow so the inverted transform applies before play
    el.style.transition = `transform ${durationMs}ms cubic-bezier(.22,.61,.36,1)`
    el.style.transform = ''
  }
}
