# Stage 7 refactor baseline (web-svelte)

Numbers come from `npm run build`, `npm test` and `wc -l` in `web-svelte/`.

## Before (origin/main @ facf533)

### Build output

App-owned chunks (everything else is a lazily loaded mermaid internal chunk):

| Chunk | Size | gzip |
| --- | ---: | ---: |
| `index-*.js` (whole app, all views, d3) | 360.73 kB | 119.78 kB |
| `index-*.css` | 112.39 kB | 17.59 kB |
| `index.html` | 0.94 kB | 0.52 kB |

Largest lazy vendor chunks (mermaid, loaded only when a wiki page has a diagram):

| Chunk | Size | gzip |
| --- | ---: | ---: |
| `cynefin-*.js` | 690.86 kB | 155.11 kB |
| `mermaid.core-*.js` | 658.91 kB | 159.47 kB |
| `cytoscape.esm-*.js` | 443.83 kB | 142.41 kB |
| `katex-*.js` | 261.33 kB | 77.57 kB |
| `architectureDiagram-*.js` | 152.02 kB | 43.22 kB |
| `sequenceDiagram-*.js` | 117.45 kB | 31.27 kB |
| `swimlanes-*.js` | 117.12 kB | 42.84 kB |

63 emitted files, 3870 kB in total (uncompressed).

### Tests

16 test files, 111 tests.

### Largest source files (non-test, lines)

| Lines | File |
| ---: | --- |
| 1082 | `src/views/SearchView.svelte` |
| 767 | `src/lib/state/wiki.svelte.ts` |
| 753 | `src/views/AgentsView.svelte` |
| 397 | `src/views/CorpusView.svelte` |
| 395 | `src/lib/i18n.ts` |
| 393 | `src/api/types.ts` |
| 387 | `src/components/wiki/WikiArticle.svelte` |
| 376 | `src/components/wiki/WikiToolbar.svelte` |
| 366 | `src/lib/markdown.ts` |
| 363 | `src/components/wiki/WikiHome.svelte` |

## After (branch `refactor-web`)

### Build output

Entry (loaded on every URL):

| Chunk | Size | gzip | vs. before |
| --- | ---: | ---: | --- |
| `index-*.js` (shell, router, stores, API client, command palette) | 109.01 kB | 38.68 kB | −251.7 kB (−81.1 kB gzip) |
| `index-*.css` | 11.78 kB | 3.25 kB | −100.6 kB (−14.3 kB gzip) |
| `index.html` | 0.72 kB | 0.40 kB | |

Screens, each loaded on first visit (JS / CSS, gzip in brackets):

| Screen | JS | CSS |
| --- | ---: | ---: |
| `WikiView` | 55.83 kB (19.16) | 15.53 kB (3.34) |
| `SearchView` | 27.21 kB (9.29) | 14.37 kB (2.64) |
| `CorpusView` | 25.76 kB (8.72) | 14.68 kB (2.54) |
| `GraphView` | 22.29 kB (8.40) | 8.40 kB (1.84) |
| `ModelsView` | 16.49 kB (5.82) | 10.88 kB (1.91) |
| `EvaluationView` | 15.12 kB (6.00) | 9.35 kB (1.65) |
| `ConsoleView` | 13.99 kB (4.89) | 9.37 kB (1.67) |
| `AgentsView` | 13.57 kB (5.57) | 10.42 kB (2.03) |
| `SyncView` | 8.98 kB (3.42) | 8.72 kB (1.84) |
| `ResourcesView` | 7.57 kB (2.68) | 1.85 kB (0.60) |

Shared lazy chunks: `graph-d3-*.js` 64.89 kB (22.42 kB gzip; d3-force/drag/selection/zoom
and their d3 dependencies, loaded by the graph screen and by mermaid), plus three
sub-1 kB helpers (`stats`, `clipboard`, `select`). The mermaid chunks are unchanged
in size.

87 emitted files, 3893 kB in total (uncompressed). The total grew by 23 kB: per-chunk
module wrappers and the new stores / helpers; what a visit actually downloads shrank
(console: 360.7 + 112.4 kB → 109.0 + 11.8 + 14.0 + 9.4 kB).



### Tests

34 test files, 239 tests (was 16 / 111).

### Largest source files (non-test, lines)

| Lines | File |
| ---: | --- |
| 610 | `src/api/types.ts` (all response types now live here) |
| 393 | `src/components/wiki/WikiArticle.svelte` |
| 384 | `src/components/graph/GraphCanvas.svelte` |
| 376 | `src/components/wiki/WikiToolbar.svelte` |
| 366 | `src/lib/markdown.ts` |
| 363 | `src/components/wiki/WikiHome.svelte` |
| 351 | `src/lib/forceLayout.ts` |
| 337 | `src/lib/i18n.ts` |
| 321 | `src/lib/state/wikiPage.svelte.ts` |
| 311 | `src/components/notion/CommandPalette.svelte` |

The four files the stage targeted:

| File | Before | After |
| --- | ---: | --- |
| `src/views/SearchView.svelte` | 1082 | 310, plus `views/search/` (3 panels, model, session) |
| `src/lib/state/wiki.svelte.ts` | 767 | 195 facade over `wikiCatalog` (242), `wikiPage` (321), `wikiPrefs` (242) |
| `src/views/AgentsView.svelte` | 753 | 139, plus `views/agents/` (4 panels, model, call stream) |
| `src/views/CorpusView.svelte` | 397 | 169, plus `views/corpus/` (store, model) |
