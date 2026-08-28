import { defineConfig } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'

export default defineConfig({
  plugins: [svelte()],
  resolve: {
    alias: {
      '@': new URL('./src', import.meta.url).pathname,
    },
  },
  server: {
    host: '127.0.0.1',
    port: 5174,
    proxy: {
      '/health': 'http://127.0.0.1:7432',
      '/v1': 'http://127.0.0.1:7432',
    },
  },
})
