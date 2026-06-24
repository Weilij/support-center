import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// Local development talks to the backend through the /api proxy (CRD 6497).
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      // ws:true so the realtime WebSocket (/api/websocket/connect) is proxied in dev.
      '/api': { target: 'http://localhost:3000', changeOrigin: true, ws: true },
      '/phase2-auth': { target: 'http://localhost:3000', changeOrigin: true },
      '/installer': {
        target: 'http://localhost:8976',
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/installer/, ''),
      },
    },
  },
})
