# egui object / dependency graph UI

**Status:** implemented architecture record; current operator behavior lives in [`EGUI_USAGE.md`](EGUI_USAGE.md)
**Audience:** implementers of `crates/rag-mcp-ui`  
**Domain authority:** [`GRAPH_DESIGN.md`](GRAPH_DESIGN.md) owns node/edge types, extract/resolve, MCP filters, rebuild, vault live graph. **This file owns only client layout, process split, and inspector UX.**  
**Related:** [`SPEC.md`](../SPEC.md) object graph · [`ARCHITECTURE_NOTES.md`](ARCHITECTURE_NOTES.md) · [`STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md) §4b · [`FEATURES.md`](../FEATURES.md) · [`GRAPH_EGUI_DECISIONS.md`](GRAPH_EGUI_DECISIONS.md) · [`ROADMAP.md`](ROADMAP.md)  
**Product role:** optional native product client plus a read-only graph inspector (Obsidian local graph analog). It is not an agent/runtime dependency; the integrated product release gate may still require native visual QA as declared in [`ROADMAP.md`](ROADMAP.md).

Where this doc and [`GRAPH_DESIGN.md`](GRAPH_DESIGN.md) disagree on **domain** types, filters, or rebuild semantics, **GRAPH_DESIGN wins**. Where they disagree on **canvas / process / MVP chrome**, **this file wins**.

---

## 1. Implemented verdict

| Decision | Choice |
|----------|--------|
| Ship shape | **Separate workspace crate** `crates/rag-mcp-ui` + binary `rag-mcp-ui` |
| Gateway binary | Stays **headless** with stdio and optional HTTP transports; **zero** `egui` / `eframe` / graph-widget deps or features |
| Cargo feature `ui` on `rag-mcp` | **Forbidden** (GPU stack bloat + stdout risk for agent hosts) |
| Workspace `default-members` | MCP-only; UI is non-default member |
| Normal live path | **`--http URL`** to the one-writer gateway; snapshot and direct DB remain limited inspection paths. Omitting all source flags opens the connection screen |
| Live DuckDB (`--db`) | Optional exclusive, strictly read-only UI mode; refuse if file locked |
| Dual-live write with MCP | **Unsupported forever** in v1 UI |
| Layout (MVP) | **One algorithm:** deterministic **RadialLocal** on neighbor views; freeze after place |
| Connections interaction | **Read-only graph canvas:** pan/zoom, select, expand neighbors, open document. Wiki editing is a separate HTTP-only workspace |
| Server payload | Topology-only `GraphView` (nodes + edges). **Never** server positions or canvas fields ([GRAPH_DESIGN §0](GRAPH_DESIGN.md)) |
| Domain filters | Mirror server **PKB defaults** when loading live: `rel_types=[wikilink, related]`, tags off unless user toggles ([GRAPH_DESIGN §7.1](GRAPH_DESIGN.md)) |
| Critical path | The headless gateway does not depend on GUI code. The current integrated product gate separately requires native visual QA |

The original MVP deliberately avoided four layout modes, graph-write menus and
an early trait sandwich. The implemented client grew product workspaces around
that same bounded, read-only Connections canvas.

---

## 2. Process architecture

### 2.1 Why not a feature flag monobinary

| Risk | Effect |
|------|--------|
| stdio MCP owns **stdin/stdout** | Any accidental print / eframe log to stdout corrupts JSON-RPC |
| `eframe` / wgpu / Metal | Large compile cost, macOS backend quirks, CI time |
| Agent installs | Users pull GPU stacks for a headless tool |
| Feature matrix | `cargo build` / CI would need dual graphs; leaks into default path |

**Rule:** `rag-mcp` package deps must never list GUI crates, even under `optional = true`.

### 2.2 Crate layout

```
/ (repo root)
  Cargo.toml                 # [workspace] members include crates/rag-mcp-ui (non-default)
  src/ …                     # rag-mcp library + bin (unchanged)
  crates/
    rag-mcp-ui/
      Cargo.toml             # depends on rag-mcp as lib + egui stack
      src/
        main.rs              # CLI + eframe entry
        app.rs               # GraphApp (eframe::App)
        load.rs              # snapshot / HTTP / read-only DB graph and wiki loads
        adapter.rs           # GraphView -> UiGraph (collapse, clamp, style)
        layout.rs            # deterministic RadialLocal
        gateway.rs           # sanitized HTTP transport boundary
        product.rs           # Home and Library DTO/application adapters
        search.rs            # Search DTO/application adapter
        operations.rs        # Activity/jobs/health/backup adapters
        revisions.rs         # History timeline/diff/restore adapters
        worker.rs            # blocking HTTP/DB workers; separate maintenance lane
        ui/
          canvas.rs
          status.rs
          detail.rs
          empty.rs
          home.rs
          library.rs
          search.rs
          revisions.rs
          wiki.rs
          operations.rs
```

Workspace policy:

```toml
# root Cargo.toml (target)
[workspace]
members = [".", "crates/rag-mcp-ui"]
default-members = ["."]   # cargo build stays lean
```

Build UI explicitly:

```bash
cargo build -p rag-mcp-ui
```

### 2.3 Dependency / library boundary

- `rag-mcp-ui` depends on **`rag-mcp` as a library** for shared serde models:
  - `GraphNode`, `GraphEdge`, `GraphView`, `GraphFilter`, `Stats`
  - `Store` is called only by exclusive, read-only `--db` inspection/export
    paths; HTTP remains the normal live path
- **Do not duplicate DTOs.** Wire JSON is the same shape MCP tools already return.
- Thin UI-local types (`UiNode`, `UiEdge`, `PosCache`) live only in `rag-mcp-ui`.

### 2.4 Coexistence with stdio MCP

Engineering modes (internal; UI presents one path):

| Mode | Writer of DuckDB | UI data source | When |
|------|------------------|----------------|------|
| **A Exclusive DB inspection** | None; UI opens an exclusive read-only `Store` handle | Read-only Wiki/Connections via worker | Dev only; MCP **not** running |
| **B Gateway + HTTP client** | Gateway process | HTTP product/graph APIs | Normal agent and native use |
| **C Snapshot** | MCP (or export) | `GraphView` JSON / `.rag/graph.json` **read-only** | Portable/offline topology inspection |
| **D Dual-live write** | — | — | **Forbidden** |

**Hard rule:** the HTTP gateway owns product writes. Direct `--db` exposes no
Wiki or graph mutation path and still must not open the file beside the gateway;
a concurrent process uses HTTP or snapshot/export, never a second `Store::open`
on the same file.

UI open paths (user-facing):

```
open --http URL       | open --snapshot PATH | open --db PATH
gateway product APIs | load GraphView JSON  | exclusive Store (fail if lock)
all writes via HTTP  | read-only topology   | read-only Wiki/Connections
```

Never cold-start unfiltered global FR on full corpus.

### 2.5 Threading model (drop_gui / freeze risk)

| Thread | Allowed | Forbidden |
|--------|---------|-----------|
| **egui frame thread** | draw, layout tick (budgeted), input, channel `try_recv` | DuckDB, `Mutex<Connection>`, full-file parse of large vault, MCP stdio, blocking network |
| **Interactive worker** | blocking gateway requests, snapshot IO and read-only direct-DB queries | egui context, wgpu |
| **Maintenance worker** | long checkpoint/backup HTTP calls | egui context, interactive request queue |
| **Main** | eframe run, CLI parse | long DB work before first frame if avoidable |

```text
  GraphApp (UI thread)  --WorkerCmd-->  interactive worker
                        <--WorkerEvt--   maintenance worker
```

Snapshot parsing, HTTP and direct read-only DB work all stay off the frame thread.
Checkpoint/backup use the maintenance lane so a long verified copy does not
block ordinary interactive client requests.

### 2.6 stdio MCP coexistence checklist

1. UI binary never attaches to MCP stdin/stdout.
2. Tracing/logging: **stderr only** (`tracing-subscriber` like server).
3. No shared process with `serve(stdio())`.
4. Snapshot refresh is file reload or the shipped MCP tool `export_graph_snapshot`, not a second live DB handle.
5. Status line shows **source** (`http_service` | `snapshot` | `vault_graph_json` | `live_store`) plus mtime when applicable.

---

## 3. Crate choices (implemented)

| Crate | Role | Current contract |
|-------|------|------------------|
| `eframe` / `egui` | Window, workspaces and plain-Painter canvas | exact matched pin `0.36.1`; lightweight `glow` renderer |
| `rag-mcp` | shared models plus read-only `Store` inspection | path dependency |
| `reqwest` | blocking gateway client on worker threads | rustls + JSON; never called on the frame thread |
| `serde` / `serde_json` | HTTP and snapshot IO | shared wire shapes |
| `std::sync::mpsc` | worker commands/events | interactive and long-maintenance lanes |

The client intentionally uses its own bounded plain-egui canvas; it does not
depend on `egui_graphs` or `petgraph`. GUI dependencies must never be added to
the headless `rag-mcp` package.

---

## 4. Domain → UI types

### 4.1 Server / shared (from GRAPH_DESIGN; do not fork)

**Do not duplicate a second domain model.** Import serde types from `rag-mcp`. Full field tables: [`GRAPH_DESIGN.md`](GRAPH_DESIGN.md) §1.2–§1.3 and §7.1. Sketch (topology only — **no** `x,y,fx,fy` ever):

```rust
// Topology only — layout positions never on the wire
struct GraphView { nodes: Vec<GraphNode>, edges: Vec<GraphEdge> }

struct GraphNode {
    id: String,
    kind: String,                 // document | tag | stub | entity
    label: String,                // display_label
    // label_key: present on server after P0a; UI may ignore for paint
    document_id: Option<String>,
    uri: Option<String>,
    resolved: bool,
    // layer / wing / room: first-class after migrate; also may appear in metadata_json pre-migrate
    metadata_json: String,
}

struct GraphEdge {
    id: String,
    source_id: String,
    target_id: String,
    rel_type: String,  // wikilink | tagged | mentions | related | tunnel
                       // | depends_on | derived_from | supersedes
    // origin: extract | explicit | system  (GRAPH_DESIGN EdgeOrigin wire)
    weight: f64,
    context: Option<String>,
    // alias, heading, chunk_id, char_start, char_end, occurrence — detail panel, not canvas
}

struct GraphFilter {
    kinds: Option<Vec<String>>,
    rel_types: Option<Vec<String>>,
    origins: Option<Vec<String>>,       // optional; GRAPH_DESIGN
    layers: Option<Vec<String>>,
    wings: Option<Vec<String>>,
    rooms: Option<Vec<String>>,
    seed_ids: Option<Vec<String>>,
    max_nodes: Option<u32>,
    aggregate_view: bool,               // server-side collapse; UI also collapses for draw
}
```

**Backlinks:** server `list_backlinks` returns `Vec<BacklinkHit>` (`edge` + `source_node`) per GRAPH_DESIGN §7.3 — **not** a bare `GraphView`. UI maps hits into detail list and/or a local `GraphView` for canvas if needed.

SPEC / GRAPH_DESIGN non-goal: force-directed layout and canvas rendering are **client-owned**. MCP tools return pure topology JSON.

Server caps (mirror; UI may clamp tighter, never ask for more without reason):

| Tool | Default max_nodes | UI note |
|------|-------------------|---------|
| `get_neighbors` | 100 | UI local default = 100 |
| `get_graph` | 500 | UI hard cap **300** before layout (sample/refuse) |

### 4.2 UI-local types

```rust
/// Display node after clamp + style derivation.
struct UiNode {
    id: String,              // same as GraphNode.id — selection / PosCache key
    kind: String,
    label: String,
    document_id: Option<String>,
    resolved: bool,
    layer: Option<String>,   // from metadata if present; optional display
    wing: Option<String>,
    room: Option<String>,
    depth: u32,              // BFS depth from seed (local view)
    degree: u32,             // in loaded subgraph
    pinned: bool,            // user pin (post-MVP drag freeze)
}

/// Canvas edge after multi-edge collapse.
struct UiEdge {
    id: String,              // representative edge id or synthetic
    source_id: String,
    target_id: String,
    rel_type: String,
    weight: f64,             // max weight among members (not sum)
    /// Dominant origin for stroke style; mixed members keep full list in `members`.
    /// Wire strings match GRAPH_DESIGN: extract | explicit | system
    origin: String,
    multi_count: u32,        // 1 if single; N if collapsed
    /// Full provenance kept for detail panel only (not drawn).
    members: Vec<GraphEdge>,
}

enum GraphSourceKind {
    LiveStore,
    HttpService { base: String },
    SnapshotFile { path: PathBuf, mtime: Option<SystemTime> },
    VaultGraphJson { path: PathBuf, mtime: Option<SystemTime> },
}

/// Positions keyed by stable graph node id, never a transient canvas index.
type PosCache = HashMap<String, egui::Pos2>;

/// Topology generation for anti-thrash.
fn topology_generation(view: &GraphView) -> u64 {
    // hash(sorted node ids + sorted edge ids)
}
```

### 4.3 Multi-edge collapse

Wikilinks store **one row per occurrence** on the server ([GRAPH_DESIGN §1.6](GRAPH_DESIGN.md)). Drawing all of them spikes springs and memory.

| Layer | Policy |
|-------|--------|
| Store | multi-edge for `wikilink`; `aggregate_view` is optional export flag |
| Canvas | **always** collapse by `(source_id, target_id, rel_type)` |
| Display | one stroke; tooltip `×N` when `multi_count > 1` |
| Detail panel | list all `members` with `context`, `alias`, `heading`, offsets, weights |
| Layout springs | one spring per collapsed edge; optional `log1p(multi_count)` scale if FR later |

Do not require the server to pre-collapse for the UI to work.

### 4.4 Optional post-MVP: `GraphQuery` trait

Defer until second backend (vault live parse) needs it. Sketch only:

```rust
trait GraphQuery {
    fn get_graph_view(&self, filter: GraphFilter) -> Result<GraphView>;
    fn neighbors(&self, node_id: &str, depth: u32, max_nodes: u32) -> Result<GraphView>;
    fn backlinks(&self, node_id: &str) -> Result<GraphView>;
    fn find_nodes(&self, query: FindQuery) -> Result<Vec<GraphNode>>;
    fn stats(&self) -> Result<Stats>;
}
```

MVP: free functions over snapshot struct + optional Store calls. No trait sandwich before first paint.

---

## 5. Layout modes

### 5.1 MVP: RadialLocal only

**Default and only chrome control for MVP.**

Algorithm (client-side):

1. Seed at origin `(0, 0)`.
2. Undirected BFS depth rings matching server `neighbors` semantics.
3. Radius: `r = depth * ring_gap` (e.g. 120–160 px).
4. On each ring: stable sort by `(kind_rank, label)` then place at equal angles (or degree-weighted angular span if hub > 24 neighbors).
5. `kind_rank`: document=0, stub=1, entity=2, tag=3 (tags outer-ish within ring if mixed).
6. Optional ≤20 intra-ring repulsion steps, then **freeze**.
7. On focus change: lerp positions 150–250 ms, then freeze.
8. New nodes (expand): place near neighbor centroid; do not reseed whole graph.

**Why not FR as MVP default:** local neighbor views are the product default; FR is O(n²), thrash-prone without freeze discipline, and fights drag. RadialLocal is deterministic, freezes cleanly, and matches Obsidian local graph mental model.

### 5.2 Post-MVP modes (not in chrome until needed)

| Mode | Use | Notes |
|------|-----|-------|
| **ForceDirected** | Filtered global n ≤ soft FR cap | `LayoutForceDirected<FruchtermanReingoldWithCenterGravity>`; steps_per_frame 1–4; energy stop; auto-freeze; disable continuous force above ~150–200 nodes |
| **Hierarchical** | **Dep projection only** (`depends_on`, optional `derived_from` / `supersedes`) | Align with GRAPH_DESIGN Dep defaults; cycle-break reverse DFS back-edges; **never** hierarchy on full wikilink+tag soup; tunnel is a soft bridge, not DAG spine |
| **Manual** | After user drag / imported positions | Display-only; pins in PosCache |

Anti-thrash (all modes):

- Re-adapt canvas topology only when `topology_generation` changes.
- Same generation re-fetch: keep `PosCache` by **string node id**.
- Edge-only change: mild unfreeze (e.g. 30 frames) if FR mode; Radial keeps rings unless seed/depth changes.
- `fit_to_screen`: **one-shot** after load / focus change, then off. Continuous fit fights pan/zoom (perceived thrash).
- Never key positions by a transient vector/canvas index after rebuild.

### 5.3 LayoutEngine trait

**Defer.** MVP: functions `radial_place(view, seed, cache) -> PosCache`. Introduce trait only if second algorithm ships.

---

## 6. Interaction

### 6.1 MVP (read-only)

| Action | Behavior |
|--------|----------|
| Pan / zoom | bounded plain-egui canvas controls |
| Click node | Select; detail panel shows fields + collapsed edge provenance |
| Double-click / context **Open** | If `document_id` present → show id/uri/title (clipboard or external open hook); resolve by **document_id**, not label |
| Context **Expand neighbors** | Request neighbors of selected; merge into view under caps |
| Search / seed field | `find_node` by label or pick from snapshot index; set seed and reload local view |
| Hover | Show label if LOD would hide it |
| Filters (minimal) | Toggle kinds: hide tags / stubs; filter rel_types if data dense |

### 6.2 Explicitly deferred (not half-disabled menus)

- `link_nodes` / unlink / optimistic edges  
- Marquee multi-select  
- Drag-to-pin as primary workflow (pin optional later)  
- Hierarchical / deps chrome  
- Write-back to DuckDB while MCP runs  

**Context menu v0:** only **Open** (document_id) and **Expand neighbors**. No disabled Unlink row.

### 6.3 Empty / error states (required)

Blank canvas is a failure mode. Each state has **one** primary action:

| State | Banner | Primary action |
|-------|--------|----------------|
| No source opened | "Open a snapshot or database" | Pick file |
| Empty graph (0 nodes) | "Graph is empty" | Ingest docs (hint) / open other file |
| Missing seed | "Pick a seed node" | Focus search |
| Seed not found | "No node matches …" | Clear / retype |
| Load / lock error | Error text (DB locked, corrupt JSON) | Retry / switch to snapshot |
| Cap truncation | "Showing N of M (capped)" | Load filtered / reduce depth |
| Stale snapshot | "Snapshot mtime …; agent may have newer data" | Refresh file |
| Filters empty all | "Filters hide all nodes" | Clear filters |
| Over hard UI cap | "Too large for layout (N > cap)" | Sample / refuse with seed BFS |

Status line **always** shows: `source` · seed label · depth · `nodes/edges` · truncated flag · frozen/layout.

Detail panel only when selection non-empty (progressive disclosure).

---

## 7. Filters and visual encoding

### 7.1 Filters (MVP minimal)

Client clamp after load (every source):

```text
UI_HARD_MAX_NODES   = 300   // refuse or seed-BFS sample above; never feed layout 5k
UI_SOFT_FR_NODES    = 200   // post-MVP FR only
UI_MAX_DRAW_EDGES   = 2000  // after multi-edge collapse
UI_DEFAULT_DEPTH    = 1
UI_DEFAULT_MAX_N    = 100   // local load
```

Mirror server: local default max 100; global never cold-start at 500 unfiltered.

MVP chrome filters (aligned with server **PKB** defaults, GRAPH_DESIGN §7.1):

- Kind toggles: documents **on**, stubs **on**, tags **off** by default (tag hubs hairball; opt-in)
- Rel types default: `wikilink` + `related` on; `tagged` / `tunnel` / Dep types **off** until user enables
- When loading **live** Store: pass the same defaults into `NeighborsOpts` / `GraphFilter` (do not fetch full soup then hide only in paint, if avoidable)
- Snapshot load: filter client-side after parse if file is already a full dump
- Project (`wing`) selection and room filtering are implemented; layer remains
  contextual workspace/filter data rather than a graph-layout mode
- Dep projection chrome (depends_on walks): post-MVP, separate from local PKB graph

Global view (post-MVP): only behind explicit control + required filter or confirm + soft max ~200.

### 7.2 Visual encoding

| Channel | Mapping |
|---------|---------|
| Node fill | kind: document = blue, wiki/document-layer = teal, tag = amber, stub = gray dashed border, entity = purple |
| Node size | `base + k * log1p(degree)` clamped |
| Node border | selected = bright; unresolved stub = dashed |
| Edge stroke | rel_type: wikilink solid, tagged dotted, related thin, tunnel thick accent, depends_on arrowed/directed (post-MVP) |
| Edge weight | stroke width clamp from weight / multi_count |
| Label LOD | **always:** seed + selected; **hover:** any; **all labels:** only if n ≤ 40 or zoom high |
| Multi-edge | single stroke + `×N` in tooltip / panel |

No server physics. Edge `weight` is visual / optional spring scale only.

---

## 8. Performance caps and algorithms

### 8.1 Hard caps (code, not docs-only)

```rust
const UI_HARD_MAX_NODES: usize = 300;
const UI_MAX_DRAW_EDGES: usize = 2000;
const UI_LOCAL_MAX_NODES: u32 = 100;
```

On every source (live, snapshot, vault json):

1. If nodes exceed `UI_HARD_MAX_NODES`: seed-BFS sample from the current seed,
   otherwise deterministic stable-id truncation, with an honest banner.
2. Collapse multi-edges; if draw edges > `UI_MAX_DRAW_EDGES`: drop lowest-weight / tagged first with banner.
3. Never run FR on uncapped full export.

### 8.2 Latency failure modes

| Failure | Cause | Mitigation |
|---------|-------|------------|
| Frame freeze | Store/DuckDB on UI thread | Worker only; `try_recv` |
| Frame freeze | FR O(n²) unbounded steps | Radial MVP; FR budget + freeze + hard n |
| Layout thrash | rebuild / random reseed every poll | `topology_generation` + PosCache by string id |
| Pin loss | positions keyed by a transient canvas index | string id cache |
| Multi-second Expand | large neighbor query or transport latency | project-scope/frontier SQL before caps; UI cap + spinner |
| Alloc storm | full GraphView clone every frame | load once; update on user action / Refresh |
| Hairball | unfiltered global + tag hubs | no cold global; hide tags; local default |
| Snapshot lie | agent ingested newer data | mtime/source badge + Refresh |
| DB corruption class | dual writer | exclusive open fails; document Mode C |

### 8.3 Worker boundary

`WorkerCmd`/`WorkerEvt` cover topology, project catalog/Home, Library, Search,
Wiki/backlinks, History, Activity and jobs. Every blocking HTTP, snapshot or
read-only DuckDB operation runs outside the frame thread and carries a sequence
identifier so stale responses can be dropped. Checkpoint and backup use a
separate maintenance worker, keeping the interactive queue responsive during a
long copy. `topology_generation` plus the stable-ID position cache prevents an
unchanged refresh from resetting layout.

---

## 9. CLI and env

```text
rag-mcp-ui [--http URL | --snapshot PATH | --db PATH]
           [--seed LABEL_OR_ID] [--depth N] [--max-nodes N]

Env:
  RUST_LOG          tracing on stderr only
```

Rules:

- At most one source flag is accepted. If all are omitted, the UI opens an
  empty connection screen with the loopback gateway URL prefilled.
- `--http` is the normal live/product mode. `--snapshot` is topology-only;
  `--db` is exclusive, current-schema and strictly read-only.
- `--seed` is optional. Connections can resolve a seed from its focus field;
  other HTTP product workspaces do not require one.
- `--depth` defaults to 1 and clamps to 1–3. `--max-nodes` defaults to 100 and
  clamps to the 300-node layout cap.
- `RAG_DB_PATH`, `RAG_VAULT_PATH` and `RAG_UI_MAX_NODES` are not implicit UI
  source/default controls; pass the corresponding CLI source and cap explicitly.

Shipped MCP tool (server side, topology dump only):

```text
export_graph_snapshot → writes GraphView JSON (no positions)
```

Agents refresh Mode C without UI opening DuckDB.

---

## 10. Snapshot / vault formats

### 10.1 GraphView JSON (Mode C)

Canonical: same serde as MCP `get_graph` / `get_neighbors` result:

```json
{
  "nodes": [
    {
      "id": "…",
      "kind": "document",
      "label": "Note A",
      "document_id": "…",
      "uri": "…",
      "resolved": true,
      "metadata_json": "{}"
    }
  ],
  "edges": [
    {
      "id": "…",
      "source_id": "…",
      "target_id": "…",
      "rel_type": "wikilink",
      "weight": 1.0,
      "context": "…"
    }
  ]
}
```

Optional envelope (UI accepts either bare `GraphView` or wrapper):

```json
{
  "version": 1,
  "exported_at": "2026-07-29T00:00:00Z",
  "source": "duckdb",
  "db_path": "./rag.duckdb",
  "graph": { "nodes": [], "edges": [] }
}
```

### 10.2 Vault `.rag/graph.json`

Per `STORAGE_ADAPTERS.md`: vault SoT is markdown; `.rag/graph.json` is rebuildable cache. UI treats it as **snapshot** (`GraphSourceKind::VaultGraphJson`). Freshness: prefer mtime vs vault files if available; badge "cache may be stale" when uncertain. Live vault parse is **post-MVP**.

---

## 11. Implemented milestones

- The separate non-default workspace crate and headless dependency boundary are
  in place.
- HTTP, snapshot and exclusive read-only DB sources share the topology DTOs;
  snapshot/direct modes expose only their documented inspection subsets.
- Connections uses deterministic RadialLocal placement, stable ID-keyed
  positions, multi-edge collapse and hard 300-node/2,000-edge paint caps.
- HTTP mode adds independent Home, Library, Search, History, Wiki and Operations
  workspaces. The project catalog has its own retry lifecycle; graph failure is
  isolated to Connections.
- `export_graph_snapshot` is shipped through MCP, and the UI binary also has an
  offline read-only `export --db ...` command.
- Direct DB paths call `Store::open_read_only`: they require an existing current
  schema, run no migration and expose no write fallback. Every Wiki write uses
  the gateway.

Future work is evidence-gated in [`ROADMAP.md`](ROADMAP.md), not scheduled by
the original PR sketch that this section replaced.

---

## 12. Non-goals

| Non-goal | Reason |
|----------|--------|
| Server-side positions or canvas in DuckDB / MCP tools | SPEC non-goal; clients own viz |
| `ui` feature on `rag-mcp` binary | stdio + GPU bloat + agent risk |
| Dual-live DuckDB write with MCP | locks / corruption |
| GUI as a runtime dependency of the headless gateway | headless server ships without Metal/wgpu; integrated product visual QA is a separate release gate |
| Four layout modes day one | thrash + meaningless hierarchy on wiki soup |
| Write/edit graph from UI in MVP | races, incomplete unlink, cognitive noise |
| Full-corpus unfiltered global default | hairball + FR freeze |
| Continuous `fit_to_screen` / continuous force after settle | perceived thrash |
| Replacing MCP graph tools | UI is inspector, not protocol |
| Wasm / web graph host in MVP | native eframe only |
| Matching Obsidian plugin feature parity | local neighbors + readability first |

---

## 13. Addressing critics (explicit)

### 13.1 stdio MCP coexistence

- Separate binary; no shared process; no egui in MCP deps.  
- Tracing stderr only.  
- Coexistence = gateway-owned HTTP client or snapshot file, not dual writer.
- Engineering modes A/B/C documented; CLI accepts at most one of **http**,
  **db**, or **snapshot** (zero opens the connection screen), not a mode
  dropdown of four.

### 13.2 UI thread

- No DuckDB / heavy IO on frame thread.  
- Layout budget; Radial freeze; hard node/edge caps in code.  
- Plain-egui canvas adaptation is gated on topology generation; positions use
  stable string ids.

### 13.3 drop_gui risk

- UI is optional non-default workspace member.  
- `cargo build` default remains MCP-only.  
- Product core (ingest, search, graph tools) does not import UI.  
- Exact GUI dependency pins remain isolated from the agent/gateway package.

### 13.4 UX critic (revise → applied)

- One normal live path: HTTP gateway; Connections focus resolves a seed and
  requests bounded neighbors at depth 1.
- One layout (RadialLocal).
- Read-only Connections canvas; Wiki editing is HTTP-only.
- Empty states with one primary action.  
- Global behind explicit control later.  
- Progressive disclosure: status always, detail on select.  
- Labels LOD; multi-edge collapse; no disabled menus.

### 13.5 Realist critic (revise → applied)

- HTTP product mode plus snapshot and exclusive read-only DB inspection.
- Thin adapter, exact eframe/egui pins and no third-party graph widget.
- No trait/engine overdesign; blocking HTTP/DB and long maintenance work use
  background worker lanes.
- Hard caps 300 / 2000.

### 13.6 Perf critic (revise → applied)

- Single hard UI node/edge cap before layout/draw.  
- No continuous FR without freeze (FR post-MVP).  
- generation + string PosCache.  
- Collapse multi-edges.  
- One-shot fit.  
- Consistent caps documented once (not 200/300/400/500/800 scattered soft numbers without enforcement).

---

## 14. Dependency graph UX patterns (product)

Borrow from Obsidian / package dep UIs, not from IDE node editors:

1. **Local first:** ego-network around a seed beats global force hairball.  
2. **Filters before physics:** hide tags / limit rel_types before any global layout.  
3. **Stable spatial memory:** same node id keeps position across refresh when topology unchanged.  
4. **Honest truncation:** always show when the view is a sample.  
5. **Inspector, not editor:** graph canvas navigates; editing lives in notes / MCP tools.  
6. **Provenance in panel:** edge context snippets explain *why* a link exists.

---

## 15. Current acceptance boundary

1. `cargo build` (default members) does not pull egui/eframe.  
2. `cargo build -p rag-mcp-ui` builds the separate native binary; live visual
   behavior still requires GUI QA rather than inference from compilation.
3. No stdout pollution; logs on stderr.  
4. 300+ node snapshot does not freeze indefinitely (cap/sample + banner).  
5. Refresh with same topology does not thrash layout.  
6. Status line shows HTTP/snapshot/direct-DB source, seed, counts and truncation.
7. Empty/error states never look like a silent blank success.  
8. Direct DB inspection remains physically read-only; HTTP owns every mutation.
9. Home, Library, Search, History, Wiki, Connections and Operations are covered
   by the live visual release gate in [`ROADMAP.md`](ROADMAP.md).

---

## 16. Open points (intentionally deferred)

- Exact FR parameters when post-MVP global ships.  
- External "open document" (path open vs copy id) on each OS.  
- Vault live parse vs sidecar authority when both exist (domain: GRAPH_DESIGN §10 prefers live parse for correctness).  
- Mixed-origin multi-edge collapse: dominant origin heuristic (prefer explicit if any member is explicit).

---

## 17. Coherence checklist vs GRAPH_DESIGN

| Topic | Agreement |
|-------|-----------|
| Topology-only wire; no server layout | yes |
| Separate process / no dual DuckDB writer | yes (UI) |
| Multi-wikilink stored; UI collapses for draw | yes |
| PKB default rels = wikilink+related; tags off | yes (UI chrome + live Neighbors defaults) |
| Dep projection separate from local graph | yes |
| Hierarchical only on depends_on family | yes (post-MVP) |
| EdgeOrigin wire: extract \| explicit \| system | yes (UI uses string; no `Extracted` rename) |
| Backlinks = occurrence hits with provenance | yes |
| Caps: server 100/500; UI hard 300 / draw 2000 | yes (UI tighter for layout) |
| Domain bugs (tag hijack, wipe explicit, etc.) | server-only; UI does not redefine |

Shared decision log: [`GRAPH_EGUI_DECISIONS.md`](GRAPH_EGUI_DECISIONS.md).

---

*End of EGUI_GRAPH_VIEW.md — authoritative for optional `rag-mcp-ui` design. Domain graph contract: [`GRAPH_DESIGN.md`](GRAPH_DESIGN.md). Prefer this over specialist brainstorms when they conflict on UI; those inputs are absorbed in §13.*
