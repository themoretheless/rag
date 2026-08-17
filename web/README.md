# rag-mcp web UI

Vue 3 + Vite SPA: Notion-style wiki browser + buildin.ai-ish knowledge chrome + force-directed graph.

## Run

1. Start gateway:

```bash
RAG_HTTP_BIND=127.0.0.1:7432 RAG_HTTP_ONLY=true ./target/release/rag-mcp
```

2. Web:

```bash
cd web
npm install --registry https://registry.npmjs.org/
npm run dev
```

Open http://127.0.0.1:5173 — Vite proxies `/v1` and `/health` to `:7432`.

## Features

- Wiki sidebar, filter, article render (`[[wikilinks]]`, tags, code)
- Edit/Save via `PUT /v1/wiki` with CAS revision
- Backlinks + properties panel
- ⌘K command palette, ⌘B sidebar
- Dark/light theme
- Graph: d3-force layout, glow nodes, zoom/pan, seed expand, open as wiki
- Search view (catalog + find)

## Env

- `VITE_API_BASE` — empty uses same origin + proxy; or `http://127.0.0.1:7432`
