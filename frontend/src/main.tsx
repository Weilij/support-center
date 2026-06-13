import React from 'react'
import ReactDOM from 'react-dom/client'
import { RouterProvider } from 'react-router-dom'

import './styles/theme.css'
import { router } from './router'
import { session } from './auth/session'
import { connectRealtime } from './realtime/client'

// Establish the realtime channel once a session exists (CRD §8.3).
void session.init().then(() => {
  if (session.lifecycle() === 'authenticated') connectRealtime()
})

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <RouterProvider router={router} />
  </React.StrictMode>,
)
