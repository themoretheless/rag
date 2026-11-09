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

## After

Filled in at the end of the stage.
