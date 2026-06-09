import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// Tauri reads this when it spawns the dev server. https://v2.tauri.app/start/frontend/vite/
const host = process.env.TAURI_DEV_HOST

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    react(),
  ],
  // Tauri expects a fixed port and manages its own console output.
  clearScreen: false,
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  server: {
    port: 5173,
    strictPort: true,
    host: host || false,
    hmr: host
      ? { protocol: 'ws', host, port: 1421 }
      : undefined,
    // Don't watch the Rust crate — it has its own (much slower) rebuild loop.
    watch: {
      ignored: ['**/src-tauri/**'],
    },
    // Web dev only: proxy /api to a locally running backend. The desktop build
    // talks to the active server via absolute URLs, so this is unused there.
    proxy: {
      '/api': {
        target: 'http://localhost:9090',
        changeOrigin: true,
        secure: false,
      },
      '/api/v1/ws': {
        target: 'ws://localhost:9090',
        ws: true,
        changeOrigin: true,
        secure: false,
      },
    },
  },
})
