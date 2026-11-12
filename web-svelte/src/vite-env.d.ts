/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Gateway origin when the UI is served from a different host; empty = same origin. */
  readonly VITE_API_BASE?: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
