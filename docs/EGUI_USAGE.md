# rag-mcp-ui usage

**Optional** graph inspector and wiki browser (read + in-app wiki edit when gateway/DB allows). Not an agent dependency and not a release gate for headless `rag-mcp`.

Design authority: [`EGUI_GRAPH_VIEW.md`](EGUI_GRAPH_VIEW.md) · decisions: [`GRAPH_EGUI_DECISIONS.md`](GRAPH_EGUI_DECISIONS.md).

## Open modes (exactly one source)

GUI open path is **XOR**: pick one of `--http`, `--snapshot`, or `--db`. Mixing them is rejected at parse time.

| Mode | Flag | DuckDB writer | Wiki catalog + article body | When |
|------|------|---------------|-----------------------------|------|
| **S Shared HTTP** | `--http URL` | MCP (same process as gateway) | Yes (`GET /v1/wiki`, `/v1/document`, `/v1/backlinks`) | **Preferred while agent/MCP runs** |
| **A Exclusive live** | `--db PATH` | UI process | Yes (direct Store read) | Dev only; **MCP must not hold the file** |
| **C Snapshot** | `--snapshot PATH` | MCP (or prior export) | **No** (topology only) | Offline inspect of exported `graph.json` |
| **B Exclusive MCP** | (no UI) | MCP | N/A | Normal agent use without UI |
| **D Dual-live write** | - | - | - | **Forbidden** |

Rules:

1. One process owns DuckDB write. Never open `--db` while `rag-mcp` holds the same file.
2. Prefer **Mode S** when MCP is up with `RAG_HTTP_BIND`: UI shares the writer via HTTP, no second DuckDB open.
3. Prefer Mode C only for offline topology (export while exclusive, then open snapshot). Snapshot has no wiki catalog and no document bodies.
4. Separate binary: zero egui/eframe deps on the `rag-mcp` package; UI is a non-default workspace member.
5. Wiki **Edit/Save** uses `PUT /v1/wiki` (HTTP) or exclusive `--db` save. Ingest/link/unlink stay on MCP tools.
6. Snapshot refresh = re-export file + reload, not a second live writer.

## Build and run

Workspace `default-members` is MCP-only (`cargo build` does not pull egui). Build the UI package explicitly:

```bash
cargo build -p rag-mcp-ui
# or release:
cargo build -p rag-mcp-ui --release
```

Open the GUI with exactly one source:

```bash
# Mode S (recommended with live MCP + HTTP gateway)
# Server: RAG_HTTP_BIND=127.0.0.1:7432 cargo run -p rag-mcp
cargo run -p rag-mcp-ui -- --http http://127.0.0.1:7432 --seed "Note title"

# Mode C (snapshot JSON; topology only, no wiki catalog)
cargo run -p rag-mcp-ui -- --snapshot ./graph.json --seed "Note title"

# Mode A (exclusive live DB; MCP must not hold the file)
cargo run -p rag-mcp-ui -- --db ./rag.duckdb --seed some-node-id
```

CLI flags (GUI path):

| Flag | Default | Meaning |
|------|---------|---------|
| `--http URL` | - | Mode S: rag-mcp HTTP base (e.g. `http://127.0.0.1:7432`) |
| `--snapshot PATH` | - | GraphView JSON (Mode C) |
| `--db PATH` | - | Exclusive DuckDB open (Mode A) |
| `--seed LABEL_OR_ID` | - | Seed by label, node id, or document_id (Graph paint; also seeds wiki open when title matches) |
| `--depth N` | `1` | Neighbor depth (clamped 1–3) |
| `--max-nodes N` | `100` | Local view cap (clamped to hard layout cap 300) |

Logging goes to **stderr only**. The UI never attaches to MCP stdin/stdout.

Without `--http` / `--snapshot` / `--db`, the window opens in the empty state (toolbar still shows Wiki | Graph; canvas prompts to launch with a source). In-app empty banner text still says "snapshot or database"; use `--http` the same way for Mode S.

## Wiki and Graph modes

With `--http` or live `--db`, the app opens in **Wiki** mode by default. Toolbar tabs switch **Wiki | Graph**. Snapshot-only open has no wiki catalog (Graph is usable after seed; Wiki shows an error that snapshot has no catalog).

### Wiki mode (Obsidian / Notion-like)

Layout:

- **Left:** page list + filter (title, slug, summary, category)
- **Center:** article body + **Backlinks** section under the body
- **Toolbar:** `← Back` (history stack, max 64), **Reload wiki**, **Show in graph**

Behavior:

- Catalog load: `GET /v1/wiki` (HTTP) or `Store::list_wiki_page_metas` (`--db`). Full list only (no server-side `q` / limit / offset).
- Body load: `GET /v1/document?id=…` (or uri) / exclusive Store `get_document`.
- On open page: refresh backlinks for that document id (see [Backlinks](#backlinks)).
- Clickable `[[wikilinks]]` resolve by exact title / slug / id / `wiki://…` (case-insensitive title/slug). Blue = known in catalog; grey = unresolved.
- Unresolved link shows an error; there is **no** in-app page create stub. Create pages via MCP `write_wiki_page` / `update_wiki_page`.
- `← Back` walks history only (no forward stack).
- **Show in graph:** seeds Graph mode with the open article `document_id`, falls back to title if seed resolve fails.
- Dual-pane wiki layout (catalog + article; optional second article panel). In-app **Edit/Save** with CAS (`if_match_revision` / etag). MCP still preferred for multi-agent bulk writes.

Markdown-ish renderer (not full CommonMark):

| Supported | Not supported |
|-----------|----------------|
| `#` / `##` / `###` headings (plain text; no inline markup inside heading lines) | `*italic*` / `_underscore_` emphasis |
| `- ` / `* ` bullets, numbered lists | Tables, images |
| Fenced ` ``` ` code blocks | Nested `***` bold+italic |
| `**bold**`, `` `code` ``, `#tags` | Bold spans do not re-parse nested tags/code |
| `[[link]]` and `[[target\|display]]` | |

### Graph mode

Layout:

- **Center:** RadialLocal local-neighbor canvas (pan / zoom / select)
- **Right detail:** node fields + optional **Read content** body pane
- **Toolbar:** seed field, depth, tags/stubs toggles, **Rebuild**, **Expand neighbors**, **Open as wiki**, **Read content**

Behavior:

- Seed by node id, display label, or `document_id` (required before local paint; empty canvas otherwise).
- Kind toggles: documents on, stubs on, tags off by default. Layout: **RadialLocal** only; hard caps **300 nodes / 2000 draw edges** after multi-edge collapse.
- **Open as wiki:** switches to Wiki mode and resolves the selected node in order:
  1. `node.document_id` if present in the wiki catalog
  2. `node.uri` when it is `wiki://…` and matches a catalog page
  3. else label via the same exact-link path as `[[wikilinks]]` (may open raw docs or fail)
- Graph node `id` is a graph UUID **distinct** from `document_id`; wiki open keys on document id / uri / label, not the graph node UUID.
- If `document_id` is set but missing from the wiki catalog, the UI falls through to label silently (raw-layer nodes may mis-open or show unresolved).
- Snapshot / vault modes cannot open wiki by document id (no catalog / no bodies). Use `--http` or `--db`.
- Detail panel has **Read content** only (loads body into the graph side pane). **Open as wiki** is on the graph toolbar, not the detail panel.
- No canvas double-click or context menu to open wiki (toolbar button only).
- No force-directed layout, hierarchical Dep chrome, or graph edits in MVP.

## Backlinks

Incoming **wikilink** edges for the open wiki article.

| Path | Source |
|------|--------|
| Mode S | `GET /v1/backlinks?id=<document_id>` → `{ ok, count, backlinks: [{ label, id }] }` |
| Mode A | `Store::wiki_backlinks_for_document(document_id)` (same rows: label + document_id or node id) |
| Mode C | Not available |

UI:

- Section **Backlinks** under the article body.
- Click a backlink label to open that page (navigate by **label**, not by the returned `id` field). Prefer unique titles so label resolve does not pick the wrong page.
- HTTP fetch failures are **silent** (empty list, no error banner). Live-DB path only fills the list on success; empty list can mean "none" or "fetch failed".
- Endpoint query is `id` only (no limit/offset). Dedup is by source key after sort-by-label; non-adjacent same keys after sort can theoretically leave duplicates if sort order does not cluster them (implementation sorts by label then `dedup_by` key).

HTTP surface used by the UI:

| Method | Path | Role |
|--------|------|------|
| GET | `/health` | Liveness |
| GET | `/v1/graph` | Topology (UI load / export parity) |
| GET | `/v1/neighbors` | Neighbor expansion (live paths) |
| GET | `/v1/find` | Seed resolve helpers |
| GET | `/v1/document` | Article / raw body (`id`, `uri`, or `q`) |
| GET | `/v1/wiki` | Full wiki catalog (no `q` / limit / offset) |
| GET | `/v1/backlinks` | Incoming wikilinks (`id` required) |

Wiki writes: MCP tools **or** `PUT /v1/wiki` (slug/title/content + optional if_match). HTTP returns `revision` / `etag` on documents.

## Export snapshot from the database

Topology dump for Mode C. Exclusive `Store::open` (same single-writer rule as live `--db`). No GUI; path printed on stdout for scripts.

```bash
# Stop MCP first if it owns ./rag.duckdb (or export from a copy).
cargo run -p rag-mcp-ui -- export --db ./rag.duckdb -o graph.json

# PKB edge set only (wikilink + related; tags off)
cargo run -p rag-mcp-ui -- export --db ./rag.duckdb --pkb -o pkb-graph.json

# Caps / filters
cargo run -p rag-mcp-ui -- export --db ./rag.duckdb --max-nodes 500 --kinds document,stub -o graph.json
cargo run -p rag-mcp-ui -- export --db ./rag.duckdb --envelope -o graph.json
```

| Flag | Default | Meaning |
|------|---------|---------|
| `--db PATH` | (required) | DuckDB to open exclusively |
| `-o` / `--output` | `graph.json` | Output path |
| `--max-nodes` | `500` | Same order as MCP `get_graph` |
| `--pkb` | off | Export with PKB defaults (`wikilink`+`related`) |
| `--rel-types a,b` | all | Edge filter (conflicts with `--pkb`) |
| `--kinds a,b` | all | Node kind filter |
| `--seed-ids id1,id2` | - | Restrict export neighborhood seeds |
| `--envelope` | off | Wrapper `{version, source, db_path, graph}` |

JSON shape matches MCP `get_graph` / `get_neighbors` (`nodes` + `edges`, topology only, no positions). UI accepts bare `GraphView` or the envelope.

Then inspect (Graph only; no wiki):

```bash
cargo run -p rag-mcp-ui -- --snapshot graph.json --seed "Your note"
```

## Interaction summary (MVP)

Read-only inspector:

- **Wiki:** sidebar filter, article, `[[wikilinks]]`, backlinks, history Back, bridge to Graph
- **Graph:** pan / zoom, click select, detail panel, seed search, expand neighbors, kind toggles, **Open as wiki** / **Read content**
- Layout: RadialLocal; hard caps 300 nodes / 2000 draw edges after multi-edge collapse

Not in MVP: forward history, unresolved-link page create, force-directed layout, hierarchical Dep chrome, graph edits, dual-live DuckDB.

## See also

- [`EGUI_GRAPH_VIEW.md`](EGUI_GRAPH_VIEW.md) - full design (process split, caps, empty states)
- [`GRAPH_EGUI_DECISIONS.md`](GRAPH_EGUI_DECISIONS.md) - freeze log
- [`GRAPH_DESIGN.md`](GRAPH_DESIGN.md) - domain graph contract
- [`CONNECT.md`](CONNECT.md) - MCP + `RAG_HTTP_BIND` / gateway setup
- [`PROD_RUN.md`](PROD_RUN.md) - production-day run notes
