# rag-mcp-ui usage

`rag-mcp-ui` is the optional native client for browsing, searching and
operating a `rag-mcp` knowledge base. It is a separate workspace package, so
the headless gateway has no egui/eframe dependency.

The full product workspace requires HTTP mode. Snapshot and direct-DB modes
remain focused inspection/development paths.

## Open modes

Choose exactly one source:

| Mode | Flag | Database owner | Available surface |
|------|------|----------------|-------------------|
| Shared HTTP | `--http URL` | gateway | Home, Library, Search, History, Wiki, Connections and Operations |
| Exclusive direct DB | `--db PATH` | UI | Wiki and Connections development path; gateway must be stopped |
| Snapshot | `--snapshot PATH` | none | read-only Connections topology; no bodies or operations |

Never open `--db` while the gateway owns the same DuckDB file. Normal live use
is one gateway process plus one or more `--http` clients.

## Build and run

```bash
cargo build -p rag-mcp-ui
cargo run -p rag-mcp-ui -- --http http://127.0.0.1:7432

# Offline topology only
cargo run -p rag-mcp-ui -- --snapshot ./graph.json --seed "Note title"

# Exclusive development mode; stop the gateway first
cargo run -p rag-mcp-ui -- --db ./rag.duckdb --seed some-node-id
```

Optional graph flags are `--seed`, `--depth` (default 1, clamped 1–3) and
`--max-nodes` (default 100, hard layout cap 300). Logs stay on stderr.

## Product workspaces

The top navigation is project-aware. Selecting a project scopes Home and
Connections immediately and seeds Library/Search filters without loading the
whole corpus into the UI process.

### Home

Project Home summarizes document, wiki, chunk and graph inventory for the
selected project and links into its Library and Connections views.

### Library

Unified Library uses server-side filters and cursor pagination. It lists lean
metadata and fetches the body only for the selected row. The detail pane can
open the document in Wiki or Connections and exposes **History** when a head
revision is available.

### Search

Search runs lexical, semantic or hybrid retrieval through `POST /v1/search`.
Results retain scores, snippets and document identity and can hand off to
Library or Connections. Synthesis remains client-owned; the UI does not
rewrite the query or generate an answer implicitly.

### History

History opens from a selected Library document. It loads a lean cursor-paginated
revision timeline, fetches the selected historical snapshot on demand, compares
it with the current head using a bounded line diff, and can restore it as a new
head.

Restore is explicit and confirmed in the workspace. The request includes the
head revision as `if_match_revision`; a concurrent update surfaces a conflict
instead of overwriting it. On success the timeline and document body reload.
The service refuses restore for immutable raw documents and directs the user to
restore or resync the source instead.

### Wiki

Wiki provides catalog filtering, linked article navigation, backlinks, split
reading and in-app edit/save. HTTP and direct-DB writes use revision/etag CAS.
Raw documents remain immutable.

The renderer intentionally covers the common knowledge-base subset: headings,
lists, fenced code, bold, inline code, tags and `[[wikilinks]]`. It is not a
full CommonMark browser.

### Connections

Connections shows a bounded project-scoped local graph. Seed by node id,
document id or title; pan, zoom, select and expand neighbors locally. Document,
stub and tag visibility can be changed without requesting an unbounded graph.
The server scopes project documents and direct companions in SQL, then caps
deterministic expansion at 300 nodes/depth 3. It owns topology but does not
persist canvas positions.

### Operations

Operations has three tabs:

| Tab | Behavior |
|-----|----------|
| Activity | one-second live polling; kind, status, client, action and full-text filters; stable anonymous client identifiers; no raw IP/UA, request/result bodies, source paths, titles or secret headers |
| Jobs | two-second polling; start source sync, inspect phase/progress/counters/current file/report/error, distinguish `succeeded` from `completed_with_errors`, cancel cooperatively |
| Health & backup | status and doctor cards, relational/FTS/WAL health, checkpoint, backup dry-run and explicit real/overwrite modes |

Background sync uses the gateway's serialized HTTP writer lane and the same
Store/embedder/configuration as MCP. Job state and Activity history are
process-local and bounded; they are operational views, not a durable audit log.

Backup destinations are constrained by the gateway allowlist. Use dry-run
first, then explicitly confirm a real or overwrite action.

## HTTP surface used by the client

| Method | Path | Workspace |
|--------|------|-----------|
| GET | `/v1/project-home` | Home |
| GET | `/v1/documents`, `/v1/document` | Library and body preview |
| POST | `/v1/search` | Search |
| GET | `/v1/wiki`, `/v1/backlinks`; PUT `/v1/wiki` | Wiki |
| GET | `/v1/graph`, `/v1/neighbors`, `/v1/find` | Connections |
| GET | `/v1/revisions`, `/v1/revisions/snapshot`, `/v1/revisions/diff`; POST `/v1/revisions/restore` | History |
| GET | `/v1/activity` | Operations / Activity |
| GET | `/v1/jobs`, `/v1/jobs/{id}`; POST `/v1/jobs/sync`; DELETE `/v1/jobs/{id}` | Operations / Jobs |
| GET | `/v1/status`, `/v1/doctor`; POST `/v1/operations/checkpoint`, `/v1/operations/backup` | Operations / Health & backup |

Route payload details are documented in [`CONNECT.md`](CONNECT.md).

## Export a topology snapshot

Snapshot export opens DuckDB exclusively. Stop the gateway or export from a
copy.

```bash
cargo run -p rag-mcp-ui -- export --db ./rag.duckdb -o graph.json
cargo run -p rag-mcp-ui -- export --db ./rag.duckdb --pkb -o pkb-graph.json
cargo run -p rag-mcp-ui -- export --db ./rag.duckdb \
  --max-nodes 500 --kinds document,stub -o graph.json
```

The output is topology only (`nodes` and `edges`); it contains no document
bodies or canvas positions.

## Verification boundary

The current native package passes its unit tests, package check and strict
Clippy. Those checks cover DTO/load behavior, worker sequencing and operational
flows. They do not replace a live GUI smoke for window sizing, rendering and
real gateway interaction.

## See also

- [`SYSTEM_MAP.md`](SYSTEM_MAP.md) — current product/runtime ownership
- [`EGUI_GRAPH_VIEW.md`](EGUI_GRAPH_VIEW.md) — Connections design and caps
- [`GRAPH_EGUI_DECISIONS.md`](GRAPH_EGUI_DECISIONS.md) — frozen graph decisions
- [`PROD_RUN.md`](PROD_RUN.md) — production operation
