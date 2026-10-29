import react from "@vitejs/plugin-react"
import { defineConfig } from "vite"

const accounts_service_url = process.env.ACCOUNTS_URL ?? "http://localhost:8082"

export default defineConfig({
  plugins: [react()],
  server: {
    port: Number(process.env.ACCOUNTS_HOST_PORT) || 4174,
    proxy: {
      "/api": {
        target: accounts_service_url,
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/api/, ""),
      },
    },
  },
})
