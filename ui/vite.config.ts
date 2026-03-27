import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

const apiPort = process.env.CHORUS_API_PORT ?? '3001'

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/internal': `http://localhost:${apiPort}`,
      '/api': `http://localhost:${apiPort}`,
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
})
