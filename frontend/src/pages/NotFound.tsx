import { t } from '../i18n'

export default function NotFound() {
  return (
    <main style={{ maxWidth: 480, margin: '15vh auto', fontFamily: 'sans-serif' }}>
      <h1>404</h1>
      <p>{t('notfound.title')}</p>
    </main>
  )
}
