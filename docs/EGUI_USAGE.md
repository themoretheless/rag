# rag-mcp-ui usage

`rag-mcp-ui` is the optional native client for browsing, searching and
operating a `rag-mcp` knowledge base. It is a separate workspace package, so
the headless gateway has no egui/eframe dependency.

The full product workspace requires HTTP mode. Snapshot and direct-DB modes
remain focused inspection/development paths.

## Open modes

Supply at most one source. Omitting all three flags opens the in-app connection
screen, prefilled with `http://127.0.0.1:7432`:

| Mode | Flag | Database writer | Available surface |
|------|------|-----------------|-------------------|
| Shared HTTP | `--http URL` | gateway | Home, Library, Search, History, Wiki, Connections and Operations |
| Exclusive direct DB | `--db PATH` | none; UI opens a read-only connection | Strictly read-only Wiki and Connections inspection; gateway must be stopped |
| Snapshot | `--snapshot PATH` | none | read-only Connections topology; no bodies or operations |

Never open `--db` while the gateway owns the same DuckDB file. Normal live use
is one gateway process plus one or more `--http` clients.

## Build and run

```bash
cargo build -p rag-mcp-ui
cargo run -p rag-mcp-ui -- --http http://127.0.0.1:7432

# Offline topology only
cargo run -p rag-mcp-ui -- --snapshot ./graph.json --seed "Note title"

# Exclusive read-only inspection; stop the gateway first
cargo run -p rag-mcp-ui -- \
  --db /Users/themoretheless/.local/share/rag-mcp/rag.duckdb \
  --seed some-node-id
```

Optional graph flags are `--seed`, `--depth` (default 1, clamped 1–3) and
`--max-nodes` (default 100, hard layout cap 300). Logs stay on stderr.

## Product workspaces

The top navigation is project-aware. In HTTP mode the authoritative project
catalog comes from `/v1/projects`, independently of graph topology. Selecting a
project scopes Home and Connections immediately and seeds Library/Search
filters without loading the whole corpus into the UI process.

Home, Library, Search, History, Wiki and Operations dispatch their own gateway
requests and remain usable when a Connections graph request fails. Only the
Connections workspace requires graph availability. A failed project-catalog
refresh retains the last successfully loaded authoritative list and current
selection; its visible **Retry** action refreshes only the catalog and does not
discard graph or other workspace state. The UI never derives the authoritative
HTTP project list from a bounded graph response.

### Home

Project Home summarizes document, wiki, chunk and graph inventory for the
selected project and links into its Library and Connections views.

### Library

Unified Library uses server-side filters and cursor pagination. It lists lean
metadata and fetches the body only for the selected row. The detail pane can
open the document in Wiki or Connections and exposes **History** when a head
revision is available. Pagination is bound to the last applied filter snapshot;
when draft filters differ, **Next**/**Back** are disabled until **Apply &
refresh** applies them and resets the cursor.

### Search

Search runs lexical, semantic or hybrid retrieval through `POST /v1/search`.
Results retain scores, snippets and document identity and can hand off to
Library or Connections. Each result set also retains the submitted request
snapshot (query, mode, project and filters), so edits made while a request is in
flight cannot relabel its response. A visible stale notice distinguishes those
results from the edited controls. Synthesis remains client-owned; the UI does
not rewrite the query or generate an answer implicitly.

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
reading and in-app edit/save. Editing and revision/etag CAS writes are available
only through the one-writer HTTP gateway. Direct `--db` Wiki is strictly
read-only, requires an existing current-schema database, runs no migration, and
never offers a local write fallback. Raw documents remain
immutable. Gateway updates preserve omitted kind/category/summary and retain
project placement, lifecycle/pin/boost state, source ownership and unrelated
metadata. Backlinks are scoped to the selected project (`wing`). A successful
response with no matching links is shown as empty; transport or database
failures appear separately in Page info with **Retry** and are never converted
into an empty result. An initial page-load failure leaves no backlink result; a
failed refresh retains the previous list but marks it potentially stale.

The renderer intentionally covers the common knowledge-base subset: headings,
lists, fenced code, bold, inline code, tags and `[[wikilinks]]`. It is not a
full CommonMark browser.

### Connections

Connections shows a bounded project-scoped local graph. Seed by node id,
document id or title; pan, zoom, select and expand neighbors locally. Document,
stub and tag visibility can be changed without requesting an unbounded graph.
The server scopes project documents and direct companions in SQL, then caps
deterministic expansion at 300 nodes/depth 3. It owns topology but does not
persist canvas positions. A graph load failure is local to Connections and does
not invalidate the authoritative project catalog or block other workspaces.

### Operations

Operations has three tabs:

| Tab | Behavior |
|-----|----------|
| Activity | one-second live polling; kind, status, client, action and full-text filters; stable anonymous client identifiers; no raw IP/UA, request/result bodies, source paths, titles or secret headers |
| Jobs | two-second polling; start source sync, inspect phase/progress/counters/current file/report/error (including terminal preparation in `refreshing_fts`), distinguish `succeeded` from `completed_with_errors`, cancel cooperatively |
| Health & backup | status and doctor cards, relational/FTS/WAL health, checkpoint, backup dry-run and explicit real/overwrite modes |

Background sync uses the gateway's serialized HTTP writer lane and the same
Store/embedder/configuration as MCP. Job state and Activity history are
process-local and bounded; they are operational views, not a durable audit log.

Backup destinations are constrained by the gateway allowlist. Use dry-run
first, then explicitly confirm a real or overwrite action. A real backup briefly
checkpoints and clones a dedicated connection under the Store mutex, then copies
one pinned MVCC generation and verifies the staged database without retaining
that shared mutex; ordinary Store queries remain available during the long copy.

## HTTP surface used by the client

| Method | Path | Workspace |
|--------|------|-----------|
| GET | `/v1/projects`, `/v1/project-home` | Project catalog and Home |
| GET | `/v1/documents`, `/v1/document` | Library and body preview |
| POST | `/v1/search` | Search |
| GET | `/v1/wiki`, project-scoped `/v1/backlinks`; PUT `/v1/wiki` | Wiki |
| GET | `/v1/graph`, `/v1/neighbors`, `/v1/find` | Connections |
| GET | `/v1/revisions`, `/v1/revisions/snapshot`, `/v1/revisions/diff`; POST `/v1/revisions/restore` | History |
| GET | `/v1/activity` | Operations / Activity |
| GET | `/v1/jobs`, `/v1/jobs/{id}`; POST `/v1/jobs/sync`; DELETE `/v1/jobs/{id}` | Operations / Jobs |
| GET | `/v1/status`, `/v1/doctor`; POST `/v1/operations/checkpoint`, `/v1/operations/backup` | Operations / Health & backup |

Route payload details are documented in [`CONNECT.md`](CONNECT.md).

## Export a topology snapshot

Snapshot export opens DuckDB read-only but must still be treated as an exclusive
inspection of that file. Prefer a verified offline backup. If the canonical DB
must be used directly, stop the gateway first.

```bash
cargo run -p rag-mcp-ui -- export \
  --db /path/to/a/verified/offline-backup.duckdb -o graph.json
cargo run -p rag-mcp-ui -- export \
  --db /path/to/a/verified/offline-backup.duckdb --pkb -o pkb-graph.json
cargo run -p rag-mcp-ui -- export \
  --db /path/to/a/verified/offline-backup.duckdb \
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
