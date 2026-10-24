import { defineConfig } from 'vitest/config'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import { svelteTesting } from '@testing-library/svelte/vite'

export default defineConfig({
  plugins: [svelte(), svelteTesting()],
  resolve: {
    alias: {
      '@': new URL('./src', import.meta.url).pathname,
    },
  },
  test: {
    // Default stays node; DOM tests opt in with `// @vitest-environment jsdom`.
    environment: 'node',
    setupFiles: ['./src/test/setup.ts'],
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
