import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// Local development talks to the backend through the /api proxy (CRD 6497).
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': { target: 'http://localhost:3000', changeOrigin: true },
      '/phase2-auth': { target: 'http://localhost:3000', changeOrigin: true },
    },
  },
})
