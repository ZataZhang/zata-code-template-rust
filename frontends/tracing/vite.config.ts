import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

const tracingPort = Number(process.env.AGENT_TRACING_PORT) || 8090
const accountsPort = Number(process.env.ACCOUNTS_PORT) || 8080
const frontendPort = Number(process.env.FRONTEND_TRACING_PORT) || 5174

export default defineConfig({
  plugins: [react()],
  server: {
    host: '127.0.0.1',
    port: frontendPort,
    proxy: {
      '/api/admin/auth': {
        target: `http://127.0.0.1:${accountsPort}`,
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/api/, ''),
      },
      '/api/v1/ingest': {
        target: `http://127.0.0.1:${tracingPort}`,
        changeOrigin: true,
      },
      '/api': {
        target: `http://127.0.0.1:${tracingPort}`,
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/api/, ''),
      },
    },
  },
})
