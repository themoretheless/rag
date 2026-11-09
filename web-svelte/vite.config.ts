import { defineConfig } from 'vitest/config'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import { svelteTesting } from '@testing-library/svelte/vite'

/** d3 packages imported by the graph canvas, plus their transitive d3 dependencies. */
const GRAPH_D3 =
  /node_modules\/d3-(force|drag|selection|zoom|dispatch|timer|quadtree|interpolate|color|transition|ease)\//

export default defineConfig({
  plugins: [svelte(), svelteTesting()],
  resolve: {
    alias: {
      '@': new URL('./src', import.meta.url).pathname,
    },
  },
  build: {
    rollupOptions: {
      output: {
        // Keep the d3 modules behind the graph screen in one chunk that only the graph
        // (and mermaid, which shares them) pulls in; they must never reach the entry.
        manualChunks(id) {
          return GRAPH_D3.test(id) ? 'graph-d3' : undefined
        },
      },
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
