import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import basicSsl from '@vitejs/plugin-basic-ssl'

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    react(),
    // Enables HTTPS with a self-signed cert for local dev (TLS 1.2+)
    basicSsl(),
  ],
  server: {
    port: 5173,
    proxy: {
      '/api': {
        target: 'https://localhost:3000',
        changeOrigin: true,
        secure: false,
      },
      '/api/v1/ws': {
        target: 'wss://localhost:3000',
        ws: true,
        changeOrigin: true,
        secure: false,
      },
    },
  },
})
