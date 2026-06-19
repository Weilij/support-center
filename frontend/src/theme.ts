// App-wide light/dark theme. Resolves from localStorage or the OS preference,
// applies via the [data-theme] attribute on <html>, and persists explicit choices.

export type Theme = 'light' | 'dark'

const STORAGE_KEY = 'theme'

export function storedTheme(): Theme | null {
  const v = localStorage.getItem(STORAGE_KEY)
  return v === 'light' || v === 'dark' ? v : null
}

export function systemTheme(): Theme {
  return typeof window !== 'undefined' &&
    window.matchMedia?.('(prefers-color-scheme: dark)').matches
    ? 'dark'
    : 'light'
}

export function resolveInitialTheme(): Theme {
  return storedTheme() ?? systemTheme()
}

export function currentTheme(): Theme {
  return document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light'
}

export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme
  localStorage.setItem(STORAGE_KEY, theme)
}

export function toggleTheme(): Theme {
  const next: Theme = currentTheme() === 'dark' ? 'light' : 'dark'
  applyTheme(next)
  return next
}

// Applies the resolved theme on boot. Does NOT persist, so an OS-preference
// user is not locked in until they explicitly toggle.
export function initTheme(): void {
  document.documentElement.dataset.theme = resolveInitialTheme()
}
