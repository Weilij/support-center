// Multi-language localization (CRD §8.4): zh-TW default with en fallback.

type Dict = Record<string, string>

const zhTW: Dict = {
  'app.name': '客服系統',
  'login.title': '登入',
  'login.email': '電子郵件',
  'login.password': '密碼',
  'login.submit': '登入',
  'login.mustChange': '您必須先變更密碼才能登入',
  'dashboard.title': '儀表板',
  'notfound.title': '找不到頁面',
  'error.badRequest': '請求格式錯誤',
  'error.unauthorized': '請重新登入',
  'error.forbidden': '權限不足',
  'error.notFound': '找不到資源',
  'error.tooManyRequests': '請求過於頻繁，請稍後再試',
  'error.server': '伺服器發生錯誤，請稍後再試',
  'error.network': '網路連線錯誤',
  'error.format': '伺服器回應格式錯誤',
}

const en: Dict = {
  'app.name': 'Support Center',
  'login.title': 'Sign in',
  'login.email': 'Email',
  'login.password': 'Password',
  'login.submit': 'Sign in',
  'login.mustChange': 'You must change your password before signing in',
  'dashboard.title': 'Dashboard',
  'notfound.title': 'Page not found',
  'error.badRequest': 'Bad request',
  'error.unauthorized': 'Please sign in again',
  'error.forbidden': 'Permission denied',
  'error.notFound': 'Not found',
  'error.tooManyRequests': 'Too many requests, please retry later',
  'error.server': 'Server error, please retry later',
  'error.network': 'Network connection error',
  'error.format': 'Server response format error',
}

const locales: Record<string, Dict> = { 'zh-TW': zhTW, en }

let current = localStorage.getItem('mcss.locale') ?? 'zh-TW'

export function setLocale(locale: string) {
  if (locales[locale]) {
    current = locale
    localStorage.setItem('mcss.locale', locale)
  }
}

export function t(key: string): string {
  return locales[current]?.[key] ?? locales['zh-TW'][key] ?? key
}
