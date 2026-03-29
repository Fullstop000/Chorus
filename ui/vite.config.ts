import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

const apiPort = process.env.CHORUS_API_PORT ?? '3001'

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/internal': {
        target: `http://localhost:${apiPort}`,
        ws: true,
      },
      '/api': {
        target: `http://localhost:${apiPort}`,
        ws: true,
      },
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
})
