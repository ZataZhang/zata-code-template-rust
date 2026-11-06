/// <reference types="vitest/config" />
import path from 'path'
import { defineConfig, type ProxyOptions } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { tanstackRouter } from '@tanstack/router-plugin/vite'
import { playwright } from '@vitest/browser-playwright'

const backendPort = Number(process.env.BACKEND_PORT) || 8080
const frontendPort = Number(process.env.FRONTEND_ADMIN_PORT) || 5173
const agentServiceUrl = process.env.AGENT_SERVICE_URL || 'http://localhost:8091'
const agentServiceToken = process.env.AGENT_SERVICE_TOKEN || ''

// 浏览器不持有 Agent 服务凭据：代理在服务端注入 Authorization，只转发 Cookie。
const agentProxy: Record<string, ProxyOptions> = {
  '/agent-api': {
    target: agentServiceUrl,
    changeOrigin: true,
    rewrite: (path) => path.replace(/^\/agent-api/, ''),
    configure: (proxy) => {
      proxy.on('proxyReq', (proxyReq) => {
        if (agentServiceToken) {
          proxyReq.setHeader('authorization', `Bearer ${agentServiceToken}`)
        }
      })
    },
  },
}

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    tanstackRouter({
      target: 'react',
      autoCodeSplitting: true,
    }),
    react(),
    tailwindcss(),
  ],
  resolve: {
    alias: {
      '@': path.resolve(import.meta.dirname, './src'),
    },
  },
  server: {
    port: frontendPort,
    proxy: {
      '/api': {
        target: `http://localhost:${backendPort}`,
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/api/, ''),
      },
      ...agentProxy,
    },
  },
  preview: {
    proxy: {
      '/api': {
        target: `http://localhost:${backendPort}`,
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/api/, ''),
      },
      ...agentProxy,
    },
  },
  test: {
    silent: 'passed-only',
    unstubEnvs: true,
    browser: {
      enabled: true,
      provider: playwright(),
      instances: [{ browser: 'chromium' }],
    },
    coverage: {
      // include: ['src/**/*.{js,jsx,ts,tsx}'], // Uncomment to expand the report to all src/**/* so untested modules appear as 0% coverage.
      exclude: [
        'src/components/ui/**',
        'src/assets/**',
        'src/routeTree.gen.ts',
        'src/test-utils/**',
        'src/routes/**',
      ],
    },
  },
})
