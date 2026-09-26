# Object / Dependency Graph Design (rag-mcp)

**Status:** definitive design for L2 object graph  
**Scope:** data model, edge ontology, extract/resolve, incremental rebuild, query ops, hybrid expand, scale, markdown vault live graph, tests  
**Code today:** `src/graph/{extract,resolve}.rs`, `src/db/graph.rs`, `src/models.rs`  
**Related:** [`SPEC.md`](../SPEC.md) object graph · [`ARCHITECTURE_NOTES.md`](ARCHITECTURE_NOTES.md) L2 · [`STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md) markdown vault · [`FEATURES.md`](../FEATURES.md) graph gaps · [`EGUI_GRAPH_VIEW.md`](EGUI_GRAPH_VIEW.md) native client/Connections architecture · [`GRAPH_EGUI_DECISIONS.md`](GRAPH_EGUI_DECISIONS.md) decision log

This document is the contract implementers follow for **server / domain graph**. Where current code disagrees, **this wins**. Client canvas layout and inspector UX live in [`EGUI_GRAPH_VIEW.md`](EGUI_GRAPH_VIEW.md) (does not change domain semantics). Rejected alternatives are marked explicitly.

---

## 0. Product intent (two projections, one store)

The graph is **one physical edge table** serving two UX projections:

| Projection | Analogy | Default rel_types | Default undirected local graph |
|------------|---------|-------------------|--------------------------------|
| **PKB** | Obsidian local/global graph | `wikilink`, `related`, optional `tunnel`; tags **opt-in** | yes |
| **Dep** | package / task dependency DAG | `depends_on`, `derived_from`, `supersedes`; optional `tunnel` | directed walks only |

**Lock:** never auto-upgrade `wikilink` to `depends_on`. Never change undirected local-graph defaults to include `depends_on` or full tag stars. Dep tools are separate filters / MCP tools.

Server returns **topology JSON only**. Layout (RadialLocal, force-directed FR, layered DAG) is client-side only: optional `crates/rag-mcp-ui` ([`EGUI_GRAPH_VIEW.md`](EGUI_GRAPH_VIEW.md)), Obsidian, or agent. No server-side physics, positions, or canvas fields.

---

## 1. Data model

### 1.1 Enums (wire strings = snake_case; reject unknown at MCP/store boundary)

```rust
/// Node kinds. Wire: "document" | "tag" | "stub" | "entity"
pub enum NodeKind {
    Document, // backed by documents row (raw or wiki layer)
    Tag,      // classification hub; never a wikilink target
    Stub,     // unresolved wikilink target; resolved=false
    Entity,   // reserved P1+ (compiled entity pages may use document+kind)
}

/// Edge relation types.
pub enum RelType {
    Wikilink,     // intentional [[page]] reference
    Tagged,       // document → #tag
    Mentions,     // weak unlinked mention (P1; opt-in extract)
    Related,      // soft explicit (link_nodes default)
    Tunnel,       // cross-wing bridge (explicit)
    DependsOn,    // hard require: source needs target (Dep)
    DerivedFrom,  // wiki ← raw provenance
    Supersedes,   // replacement chain
    // Embeds reserved; ![[embed]] is NOT an edge in P0
    // Hand-authored wiki relations are NOT enum variants: closed string list
    // WIKI_SEMANTIC_REL_TYPES, accepted by RelType::validate_wire (below)
}

/// Graph layer copy from document (scope filter, not a second graph).
pub enum GraphLayer {
    Raw,
    Wiki,
    Diary,
    Schema,
    Index,
    Log,
}

/// Who owns the edge for rebuild / delete policy.
pub enum EdgeOrigin {
    Extract,  // produced by rebuild_document_graph from body text
    Explicit, // link_nodes / tunnel CRUD / agent
    System,   // derived_from on compile, etc.
}
```

Parse with `NodeKind::parse` / `RelType::parse` / `EdgeOrigin::parse`: unknown → structured error at MCP boundary. Store columns remain `VARCHAR` for adapter portability (DuckDB, Markdown sidecar same serde).

**Wiki semantic relations are a second, closed list.** `RelType` covers only the
structural relations extraction and the Dep/tunnel/compile workflows write. A
second accepted set — `WIKI_SEMANTIC_REL_TYPES` — holds the relations people type
by hand into wiki maps (`детализирует`, `реализует`, `проверяет`, `изменяет схему`,
`зависит от`, `вызывает`, `публикует`, `обновляет`, `хранит`, `использует`,
`компенсируется`). They carry spaces and Cyrillic, so they cannot be enum
variants; the store boundary validates with `RelType::validate_wire`
(`RelType::WIRE` ∪ this set), which is why it does not call `RelType::parse`.

| Property | Consequence |
|----------|-------------|
| No extractor emits them | always `origin = explicit` (§1.6); rebuild and §6.3 demote never own them |
| Endpoints are wiki pages | measured: 118 edges in live, 100% `document → document`, `weight = 1.0`, `context = NULL` |
| They are the author's own distinctions | included in the §7.1 PKB default read set, so a default `get_graph` / UI view still shows them |
| Still a closed list | `link_nodes(rel_type="зависимость")` is rejected; adding a name is a vocabulary change (§14) |

The live count was measured read-only on the gateway's own 2026-09-24 auto-backup
(`SELECT rel_type, COUNT(*) FROM graph_edges GROUP BY rel_type`), not assumed.
`crates/rag-mcp-ui` localizes the structural names for display only; a semantic
relation renders as its own `rel_type` string.

### 1.2 `GraphNode` (extended)

| Field | Type | Notes |
|-------|------|-------|
| `id` | String | PK; stable for document lifetime; promote preserves id |
| `kind` | NodeKind wire | |
| `label` | String | display_label (NFKC + trim only; preserves vault casing) |
| `label_key` | String | **single** normalize key (see §3); indexed |
| `document_id` | Option\<String\> | set when kind=document and live |
| `uri` | Option\<String\> | stable key; tags use `tag://{label_key}` |
| `resolved` | bool | stubs false; tags/docs true |
| `layer` | Option\<String\> | copy from Document on ensure |
| `wing` | Option\<String\> | copy from Document |
| `room` | Option\<String\> | copy from Document |
| `metadata_json` | String | freeform |
| `created_at` / `updated_at` | ts | preserve created_at on upsert |

**Rejected:** separate `label_norm` (NFC) and `label_key` (NFKC) columns. One key only: `label_key` (§3). Critic FATAL on NFC vs NFKC fork.

### 1.3 `GraphEdge` (extended)

| Field | Type | Notes |
|-------|------|-------|
| `id` | String | PK; derived edges use deterministic id (§4.4) |
| `source_id` | String | |
| `target_id` | String | |
| `rel_type` | RelType wire | |
| `origin` | EdgeOrigin wire | default `extract` for rebuild inserts; `explicit` for link_nodes |
| `weight` | f64 | finite > 0, clamp; extract 1.0; mentions 0.5; tunnel caller/1.0 |
| `context` | Option\<String\> | ±40 Unicode scalars snippet |
| `alias` | Option\<String\> | from `[[Target\|alias]]` |
| `heading` | Option\<String\> | from `[[Target#Heading]]`; page-level edge still |
| `chunk_id` | Option\<String\> | covering chunk for span; null if no chunks |
| `char_start` / `char_end` | Option\<i32\> | **Unicode scalar** indices, exclusive end; match FixedChunker |
| `occurrence` | i32 | 0-based among same (source,target,rel) derived multi-edges |
| `created_at` | ts | |

**Offset domain (FATAL fix):** extract, edges, `chunk_id_for_span`, and chunker all use **Unicode scalar indices**. No `byte_start`/`byte_end` on public API. Internal scanners may use bytes while converting to char indices at the extract boundary only.

### 1.4 `node_aliases` (new table)

```sql
CREATE TABLE IF NOT EXISTS node_aliases (
  id VARCHAR PRIMARY KEY,
  node_id VARCHAR NOT NULL,
  alias_key VARCHAR NOT NULL,   -- label_key(alias text)
  alias_label VARCHAR NOT NULL, -- display
  created_at TIMESTAMP NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_node_aliases_key ON node_aliases(alias_key);
-- NOT UNIQUE(alias_key): multi-claim allowed; link_health lints conflicts
```

Register on ensure_document_node: title, uri stem, frontmatter aliases. On title rename, **keep old title as alias**.

**Rejected:** `UNIQUE(alias_key)` with silent second-claimant loss (Obsidian critic FATAL).

### 1.5 DDL migration (best-effort additive)

```sql
-- graph_nodes
ALTER TABLE graph_nodes ADD COLUMN IF NOT EXISTS label_key VARCHAR;
ALTER TABLE graph_nodes ADD COLUMN IF NOT EXISTS layer VARCHAR;
ALTER TABLE graph_nodes ADD COLUMN IF NOT EXISTS wing VARCHAR;
ALTER TABLE graph_nodes ADD COLUMN IF NOT EXISTS room VARCHAR;
CREATE INDEX IF NOT EXISTS idx_graph_nodes_label_key ON graph_nodes(label_key);

-- graph_edges
ALTER TABLE graph_edges ADD COLUMN IF NOT EXISTS origin VARCHAR DEFAULT 'extract';
ALTER TABLE graph_edges ADD COLUMN IF NOT EXISTS alias VARCHAR;
ALTER TABLE graph_edges ADD COLUMN IF NOT EXISTS heading VARCHAR;
ALTER TABLE graph_edges ADD COLUMN IF NOT EXISTS chunk_id VARCHAR;
ALTER TABLE graph_edges ADD COLUMN IF NOT EXISTS char_start INTEGER;
ALTER TABLE graph_edges ADD COLUMN IF NOT EXISTS char_end INTEGER;
ALTER TABLE graph_edges ADD COLUMN IF NOT EXISTS occurrence INTEGER DEFAULT 0;
-- existing: idx_graph_edges_source, idx_graph_edges_target
CREATE INDEX IF NOT EXISTS idx_graph_edges_origin ON graph_edges(origin);
CREATE INDEX IF NOT EXISTS idx_graph_edges_rel ON graph_edges(rel_type);
```

Backfill: `label_key = label_key(label)` in migrate Rust path. DuckDB lacks hard FKs; doctor/lint for dangling edges.

> **2026-09-24 implementation notes.**
> `graph_edges.origin` is added **nullable** and backfilled in
> `schema::backfill_graph_edge_origins`, not with `DEFAULT 'extract'`: a column
> default would retroactively claim every pre-existing user edge as derived and
> hand it to the rebuild delete predicate. Mapping is `edge_origin='derived' →
> extract`, `'manual' → explicit`, and for rows predating both columns the
> extraction-owned relation names **plus the pipeline's structural membership
> `related` edges** → `extract` while everything else → `explicit` (§1.6
> amendment, 2026-09-25).
> `edge_origin` is left in place as the immutable pre-migration record; `origin`
> is authoritative. `label_key` cannot be derived in SQL at all (DuckDB has no
> NFKC), so it is backfilled row-by-row in Rust behind a missing-key count guard.
> Both `GraphNode.label_key` and `GraphEdge.origin` are kept at the store
> boundary rather than widened onto the wire structs: `label_key` is derived from
> `label` on write and `origin` is set by the insert helper that owns the write
> policy, so no caller can lie about either, and the 20-odd construction sites in
> `src/` plus `crates/rag-mcp-ui` stay untouched.

### 1.6 Edge identity partition (FATAL fix)

| Class | rel_types | Multi-edge? | Upsert key |
|-------|-----------|-------------|------------|
| **Derived** | `wikilink`, `tagged`, `mentions` | wikilink: **yes** (one row per occurrence); tagged: collapse duplicate tags per source | `derived_edge_id(source, target, rel, occurrence)` or span |
| **Explicit** | `related`, `tunnel`, `depends_on`, `derived_from`, `supersedes` | tunnel may multi if notes differ (P1); related **idempotent** | `UNIQUE` logical: `(source_id, target_id, rel_type)` for related/depends_on/derived_from/supersedes |

**Rejected:** global `UNIQUE(source, target, rel)` that kills multi-wikilink.  
**Rejected:** weight=count collapse as the only storage (aggregate is a **view flag**, not storage).  
**Rejected:** dual ownership half-wired as both `origin` and rel-only without single delete predicate.

**Single rebuild delete predicate (authoritative):**

```text
DELETE FROM graph_edges
WHERE source_id = ?
  AND (
    origin = 'extract'
    OR (origin IS NULL AND rel_type IN ('wikilink', 'tagged', 'mentions'))
  );
```

Implemented as `Store::delete_derived_edges_from(source_id)` /
`delete_derived_edges_from_locked`. Explicit `related`/`tunnel`/`depends_on` with
`origin=explicit` survive re-ingest, and extract-owned relation names are still
reclaimed when a row carries a mis-tagged or missing owner.

> **2026-09-24 amendment.** The originally written `origin='extract' OR
> rel_type IN ('wikilink','tagged','mentions')` was scoped to
> `origin IS NULL` rows. Applied unconditionally, the relation-name branch also
> deleted user-created `link_nodes(..., 'wikilink')` / `'tagged'` edges, which is
> what the `rebuild_preserves_explicit_edges_even_when_their_type_is_extractable`
> regression test had already locked down. The narrowed form keeps the documented
> intent (always reclaim extraction-owned noise, even from a mis-tagged row)
> without letting the OR clause override the owner column.

> **2026-09-25 amendment: the backfill must attribute membership edges.**
> `graph::resolve::append_structural_edges` writes the project/directory
> placement edges as `rel_type='related'`, which §1.6 classes as an **explicit**
> relation. A backfill keyed on relation name alone therefore stamped the
> pipeline's own rows `explicit`, hiding them from the delete predicate for good:
> the guard on `origin IS NULL` makes the mislabel sticky, and every re-ingest
> stacked a fresh pair instead of replacing it. Measured read-only on the
> gateway's auto-backup: **165 893 of 166 020 `related` edges** are membership
> rows (83 106 `project membership` + 82 787 `directory membership`); only 127
> are hand-made `link_nodes` edges with `context IS NULL`. The predicate now
> reads `rel_type='related' AND context IN (models::STRUCTURAL_EDGE_CONTEXTS)`,
> a context list shared with the writer as constants, plus a guarded repair pass
> for databases that already ran the name-only version. `context` is a safe
> ownership marker because `link_nodes` cannot set it: no hand-made edge can
> collide with the pipeline's literals.


---

## 2. Edge type ontology

| rel_type | Direction meaning | Origin | PKB default | Dep default | Auto from text? |
|----------|-------------------|--------|-------------|-------------|-----------------|
| `wikilink` | source cites target page | extract | yes | no | `[[...]]` |
| `tagged` | source classified under tag | extract | opt-in | no | `#tag` |
| `mentions` | weak unlinked mention | extract (P1) | opt-in | no | optional |
| `related` | soft association | explicit, **and** `extract` for the pipeline's structural membership edges (§1.6, 2026-09-25) | yes | no | no from markup; yes from document placement |
| `tunnel` | cross-wing bridge | explicit | opt-in | opt-in | no |
| `depends_on` | source **requires** target | explicit | no | yes | no |
| `derived_from` | wiki page from raw source | system/explicit — **no writer yet**: `EdgeOrigin::SYSTEM` exists but nothing stamps it, because the compile projection is P1 (§17) | no | yes | on compile |
| `supersedes` | source replaces target | explicit | no | yes | no |

**Weights:** extract wikilink/tagged `1.0`; mentions `0.5`; explicit uses caller weight (default `1.0`); clamp finite `> 0` — **not enforced by `link_nodes`** (see §17; measured 2026-09-25: the live corpus has zero non-positive or non-finite weights).

**depends_on:** directed only. Outbound = prerequisites; inbound = impact. Cycle on insert → structured SCC error (P1); do not silently accept DAGs that are not.

**tunnel:** is a `RelType`, not a node kind. P0: `link_nodes(..., rel_type=tunnel)`. P1: `create_tunnel` / `list_tunnels` / `delete_tunnel` / `follow_tunnels` (neighbors filter `rel_types=[tunnel]`).

---

## 3. Label normalization

Single module `graph/normalize.rs`:

```rust
/// Match / promote / stub key.
/// trim → NFKC → Unicode lowercase (to_lowercase) → collapse internal whitespace to single space.
pub fn label_key(s: &str) -> String;

/// Display only: trim + NFKC (preserve case).
pub fn display_label(s: &str) -> String;

/// Tags: same as label_key on body without leading #.
pub fn tag_key(s: &str) -> String { label_key(s) }
```

**Documented limits:** `to_lowercase` is not full Unicode CaseFold (Turkish I, final sigma). Homoglyphs are distinct keys; no confusable detection (OK for personal vaults).

**Rejected:** case-sensitive label match (current code leaves permanent stubs for `[[Foo]]` vs title `foo`).  
**Rejected:** dual NFC/NFKC columns.

---

## 4. Extract algorithm

### 4.1 `ExtractedLink`

```rust
pub struct ExtractedLink {
    pub target_label: String,   // raw target display after trim
    pub target_key: String,     // label_key(target_label) or path key
    pub rel_type: String,       // wikilink | tagged | mentions
    pub context: Option<String>,
    pub alias: Option<String>,
    pub heading: Option<String>,
    pub is_path: bool,          // contains '/' or looks like vault path
    pub char_start: i32,        // Unicode scalar index of match start
    pub char_end: i32,          // exclusive
    pub occurrence: i32,        // filled after sort within same key+rel
}
```

### 4.2 Wikilink parse rules (P0)

1. Mask code first: `mask_code_regions` (fenced N-backticks, inline 1–2 ticks; unclosed fence → mask to EOF, **fail-closed**). Defer indented-code / HTML comments.
2. Skip `![[embed]]` entirely (non-edge). No fake embed parity with Obsidian toggles until `embeds` rel exists.
3. Inner parse: first `|` splits alias; first `#` splits heading from target; `#^block` ignored (page-level edge only).
4. `[[#Heading]]` same-page: **skip in P0**.
5. Empty / whitespace-only target: skip.
6. Cap: `RAG_MAX_LINKS_PER_DOC` (default **2000**); warn/truncate beyond.

### 4.3 Tag extract

- Unicode body + boundary rules (current code).
- Default: **dedupe by `target_key`** per document (one `tagged` edge per tag node).
- Optional `ExtractOptions.include_frontmatter_tags` for YAML `tags:` list (P1).

### 4.4 `extract_links` / `extract_links_with`

Pure functions, backend-agnostic. Same code path for DuckDB ingest and Markdown vault live parse.

### 4.5 `chunk_id_for_span(chunks, start, end)`

Return id of chunk where `chunk.char_start <= start && end <= chunk.char_end` (or covering max-overlap if straddling: prefer chunk containing `start`). Null if no chunks. Refresh on re-ingest after chunk rewrite. If extract_hash skip keeps edges but chunks rechunked, null `chunk_id` or mark dirty (§6).

> **2026-09-24 implementation notes.**
> `extract_links` / `extract_links_with` return spans as **Unicode scalar** offsets
> (`char_start`/`char_end`), resolved in one linear pass over the document, because
> `chunks.char_start` is scalar-based too. Byte offsets would misattribute every
> link in a non-ASCII document.
>
> `alias`, `heading`, `chunk_id`, `char_start`, `char_end`, `occurrence` live on
> **`graph_edges` columns** and are written only for `origin = 'extract'` rows.
> They are deliberately absent from the wire [`GraphEdge`] struct: it has ~27
> construction sites across the crate and the UI, and §7.3's occurrence-aware
> backlink API (P1) is what should surface them. Explicit `link_nodes` edges keep
> all six NULL — a hand-made edge has no span in any document.
>
> Two extraction rules from §4.2/§4.3 that changed stored shape: `tagged` links
> collapse to **one edge per `target_key` per document** (mentions no longer
> multiply tag hubs), and repeated wikilinks keep an `occurrence` index in
> document order. Extraction stops at `RAG_MAX_LINKS_PER_DOC` (default 2000) with a
> warning rather than an ingest failure.

---

## 5. Resolve algorithm

### 5.1 Stable ids (`graph/ids.rs`)

```rust
// blake3 hex truncated to 32 chars
fn stable_node_id(kind: &str, key: &str) -> String;

pub fn document_node_id(uri_or_doc_id: &str) -> String; // prefer uri if non-empty
pub fn stub_node_id(label_key: &str) -> String;
pub fn tag_node_id(tag_key: &str) -> String;
pub fn derived_edge_id(source: &str, target: &str, rel: &str, occ: i32) -> String;
```

**Greenfield:** use deterministic ids from day of flag/schema_version gate.  
**Existing DBs:** **must** run merge/remap before enabling: merge stubs/tags by `label_key`/`uri`, remap edge endpoints, drop duplicates. Without migration, blake3 creates **duplicate stubs and split edge sets** (critic FATAL). Random UUID continues until migrate completes.

**2026-09-11 (V-O8):** the live corpus keeps UUID document/node ids. blake3 is the content-hash (`content_hash`, dataset/prompt identity), not a scheduled id migrate. Do not flip `stable_node_id` on an existing DuckDB without the merge pass.

Stub/tag insert: `INSERT ... ON CONFLICT(id) DO UPDATE` / ignore; unique constraint recommended on `(kind, label_key)` for stub+tag only (not documents: multi-doc same title allowed).

### 5.2 `ensure_document_node` order (no steal)

1. By `document_id` → update label/uri/layer/wing/room; return same id.  
2. By `uri` (if non-empty) → attach document_id if missing; **never** if another document_id already set to a different doc.  
3. Promote **only** `kind=stub` or `resolved=false` with matching `label_key(title)` or `label_key(uri_stem)` or registered aliases: set kind=document, document_id, uri, resolved=true; **keep node id**.  
4. Else create new document node (deterministic id from uri).  
5. Register aliases (title, stem, frontmatter).

**Rejected (current bug):** reuse existing **document** node by title alone (steals another doc's node).  
**Rejected:** resolve-by-newest on multi-document same `label_key`.

### 5.3 `resolve_wikilink` order (never tag)

1. **Path/uri variants** if `is_path` or target looks like path: exact uri, `vault://…`, relative path normalization (markdown vault §10).  
2. **Unique** document node by `label_key` (kind=document, resolved). If **multiple** documents share the key → **do not auto-bind**; attach to temporary stub OR leave unresolved stub and surface ambiguity via lint (`link_health`). Prefer stub + lint over wrong note (Obsidian path uniqueness).  
3. **Alias** via `find_node_by_alias_key` → if unique document, bind; if multi-claim, lint, no auto-bind.  
4. Existing **stub** by `label_key`.  
5. Else **create stub** (`stub_node_id(key)`, display label from target).

**Never** resolve wikilink to `kind=tag` even if `resolved=true` (current code bug: "any resolved" binds tags).

### 5.4 Tags

`upsert_tag_node` by `tag_node_id` + `uri = tag://{key}`. INSERT OR REPLACE by id.

### 5.5 Batch resolve (latency)

For large L: `SELECT WHERE label_key IN (...)` + alias batch; single transaction for new stubs. Avoid O(L) round-trips under Mutex.

### 5.6 P1 forced promote

`resolve_stub(stub_id | label, target_document_id | uri)`: promote without waiting for re-ingest title match.

---

## 6. Incremental rebuild

### 6.1 `rebuild_document_graph(store, doc) -> (node_id, edge_count)`

```
1. node_id = ensure_document_node(doc)
2. delete_derived_edges_from(node_id)   // §1.6 predicate ONLY
3. links = extract_links(doc.content)
4. assign occurrence per (target_key, rel_type) in document order
5. batch resolve targets → edge rows (origin=extract, offsets, alias, heading)
6. chunk_id_for_span for each link if chunks present
7. INSERT edges (uuid id; `derived_edge_id` upsert key deferred by V-O8, §17)
8. optional orphan prune AFTER reinsert (§6.3)
9. return (node_id, edge_count)
```

Single transaction where backend allows. Embed stays outside graph txn (already separate).

### 6.2 Dirty planning

| Signal | Action |
|--------|--------|
| content_hash / extract_hash unchanged | skip extract+edge rewrite |
| chunks rechunked but content same | null chunk_id on edges OR force edge rewrite if chunk_id required |
| embed-only change | never touch graph |

Include chunker params in extract dirty reasons if offsets would shift.

### 6.3 Orphan policy

- **Prune only:** stub-only nodes with **global degree 0** after reinsert.  
- **Never** cascade-delete stubs still referenced by other documents' edges.  
- **Never** prune between delete_derived and reinsert.  
- Prefer lint/doctor (`find_orphans`) over eager deletes.  
- Document delete: **demote** document node to stub (keep id, clear document_id, resolved=false, kind=stub) OR refuse when inbound explicit `depends_on`/`tunnel` exist and policy=`refuse`. Remove only that node's **outbound extract** edges; **retain inbound** edges from other notes (Obsidian: deleted note stays as unresolved target).  
- **Rejected:** `delete_edges_incident` wipe of inbound depends_on/tunnel/wikilink on delete_document (current path destroys honesty of backlinks).  
  **Implemented 2026-09-24:** `delete_document` now runs `demote_graph_for_document_locked` — the node becomes an
  unresolved `stub` with `document_id` cleared, only its extraction-owned **outbound** edges go (the same §1.6
  predicate the rebuild uses), and every inbound edge from surviving notes stays. `uri` is kept on purpose, so
  re-ingesting the same file re-binds the same node id and the retained backlinks point at real content again.
  `Store::delete_edges_incident` and `Store::delete_graph_for_document` were removed: with the demote path they
  had no caller left, and both existed only to perform the rejected wipe.

### 6.4 Explicit API

```rust
Store::link_nodes(source, target, rel, weight) // origin=explicit; related idempotent
Store::unlink_nodes(source, target, rel)       // required once explicit survives rebuild
Store::create_tunnel / list_tunnels / delete_tunnel / follow_tunnels  // P1
```

---

## 7. Query operations

### 7.1 Filters

```rust
pub struct GraphFilter {
    pub kinds: Option<Vec<String>>,
    pub rel_types: Option<Vec<String>>,
    pub origins: Option<Vec<String>>,
    pub layers: Option<Vec<String>>,
    pub wings: Option<Vec<String>>,
    pub rooms: Option<Vec<String>>,
    pub seed_ids: Option<Vec<String>>,
    pub max_nodes: Option<u32>,  // default 500
    pub aggregate_view: bool,    // collapse parallel same-(s,t,rel) for UI; default false for backlinks
}

pub struct NeighborsOpts {
    pub depth: u32,              // default 1
    pub max_nodes: u32,          // default 100
    pub undirected: bool,        // default true for PKB local graph
    pub direction: Direction,    // In | Out | Both when directed
    pub rel_types: Option<Vec<String>>,
    pub origins: Option<Vec<String>>,
    pub layers / wings / rooms: Option<...>,
    pub include_tags: bool,      // default false for local graph
}
```

**Defaults (Obsidian-shaped):**

- `get_neighbors`: undirected BFS; default `rel_types = pkb_rel_types(false)` =
  `[wikilink, related]` + the §1.1 wiki semantic set; tags off unless `include_tags`; tunnel opt-in.  
- `get_graph`: same PKB literary set unless client asks for Dep projection.  
- Cap export: 500 nodes / 100 neighbors default.

> **2026-09-24 implementation notes.** Both MCP tools now default to the literary
> set (`pkb_rel_types`), with `include_tags` as the tag opt-in and an explicit
> `rel_types` list as the escape hatch for `tunnel` / `mentions` / `depends_on`.
> Passing `rel_types` wins outright — including `["tagged"]` alone — and an empty
> list is treated as "not supplied" rather than "no relations". `get_graph` also
> infers tag edges when a caller names `tag` among `kinds`.
>
> **2026-09-25.** `pkb_rel_types` is now the only definition of this set: the UI
> export constant and the hand-written SQL `IN (…)` literal in `db/graph.rs`
> (`scoped_relation_types`, used by six project-scoped queries) were deleted and
> derive from it, so the default read set can no longer drift from the §1.1
> vocabulary. It includes the wiki semantic relations because they are in live
> data (§14, 2026-09-25); `tunnel` remains opt-in.

### 7.2 Scale: frontier SQL (FATAL fix)

**Rejected:** load all `graph_edges` into memory for BFS (O(|E|) under Mutex; fails at 100k+).

```text
BFS:
  frontier = {seed}
  for depth in 0..D:
    SELECT edges WHERE source_id IN frontier [OR target_id IN frontier if undirected]
      AND rel_type IN (...) AND filters
    expand next frontier; stop at max_nodes
```

Use `idx_graph_edges_source` / `idx_graph_edges_target`. Same pattern for depends_on closure (directed).

> **2026-09-24 implementation notes.** `db/graph.rs::expand_frontier` is the one
> BFS loop; `neighbors`, `follow_tunnels` and both UI projections pass it a
> per-hop discovery query, so no path materialises `graph_edges` any more. Two
> rules that loop encodes:
> * A hop only reports ids that still have a `graph_nodes` row. The old
>   adjacency-map walk could pass through a ghost endpoint (edge to a deleted
>   node id, then out of it); that traversal was an artifact of building the
>   adjacency list from edges, not a decision, and it made dangling ids consume
>   the node budget.
> * `depth` is clamped to the §9 cap of 5. The walk is budget-bound, so the
>   clamp exists to bound `get_neighbors` arguments from the MCP surface.
>
> `Store::neighbors` deliberately keeps the *raw* contract (every relation type,
> tag hubs reachable) because wiki backlink walking and graph maintenance depend
> on it. The §7.1 PKB defaults are applied at the read boundaries instead:
> `Store::neighbors_filtered` for MCP `get_neighbors`, a defaulted `GraphFilter`
> for MCP `get_graph`, and `export_pkb_neighbors_for_ui` /
> `export_project_neighbors_for_ui` for the HTTP `/v1/neighbors` route and the UI.
> All of them take the literary set from the single
> `pkb_rel_types(include_tags)` / `pkb_node_kinds(include_tags)` pair, so a client
> that wants tunnels, mentions or tags asks for it explicitly instead of each
> surface deciding for itself.

### 7.3 Backlinks API (occurrence-aware)

```rust
pub struct BacklinkHit {
    pub edge: GraphEdge,       // full provenance: alias, heading, offsets, context
    pub source_node: GraphNode,
}
// list_backlinks(node) -> Vec<BacklinkHit>
// NOT collapsed GraphView-only; aggregate optional flag for topology UI
```

Outgoing links surface: `get_neighbors(..., direction=Out)` or dedicated `get_outgoing_links` for Obsidian Outgoing Links parity.

### 7.4 MCP tools (graph surface)

| Tool | Priority | Behavior |
|------|----------|----------|
| `get_graph` | have | filtered topology; PKB defaults |
| `get_neighbors` | have | local BFS + NeighborsOpts |
| `get_backlinks` | extend | occurrence list + snippets |
| `link_nodes` / `unlink_nodes` | have / add | explicit |
| `find_node` | have | id / document_id / label_key |
| `graph_expand_search` | P0 | §8 |
| `resolve_stub` | P1 | forced promote |
| `create_tunnel` / `list_tunnels` / `delete_tunnel` / `follow_tunnels` | P1 | |
| `link_health` / `find_orphans` | P1 | ambiguity, alias conflicts, orphans |
| Dep walks | P1 | `depends_on` prereq/impact; separate from local graph |

---

## 8. Hybrid expand (`graph_expand_search`)

```
hits = search(query, mode, top_k, filters)
for each hit (unique document_id, preserve score order):
  node = find_node_by_document_id(document_id)
  if node: subgraph = neighbors_opts(node.id, depth=1, PKB defaults)
merge subgraphs (nodes/edges by id)
return { hits, graph: GraphView, seed_node_ids }
```

**Keep retrieval provenance separate from link provenance:** hits still carry chunk scores/offsets; edges carry wikilink offsets. Do not invent edges from search scores.

---

## 9. Scale limits

| Concern | Limit / policy |
|---------|----------------|
| Default export | max_nodes 500 |
| Local graph | max_nodes 100, depth 1–2 typical; hard cap depth 5 |
| Links per doc | 2000 extract cap |
| BFS | frontier SQL only |
| Multi-edge LLM context | `aggregate_view=true` optional; default store keeps multi for backlinks |
| Concurrent ingest | Mutex store; stub insert by deterministic id upsert (no find-then-insert race) |
| Layout | never server-side; optional UI: RadialLocal first, FR only post-MVP under hard caps ([`EGUI_GRAPH_VIEW.md`](EGUI_GRAPH_VIEW.md)) |

Practical DuckDB single-process: tens of k nodes / 100k edges OK if queries stay indexed. Beyond that: adapter to Postgres, or export projections.

---

## 10. Markdown vault live graph

Same domain types and extract/resolve pure logic.

| Concern | Policy |
|---------|--------|
| Edge SoT | live parse of note bodies preferred for correctness after Obsidian edits |
| Cache | `.rag/graph.json` rebuildable; same `GraphEdge`/`GraphNode` serde |
| Explicit edges | merge-preserve from sidecar/frontmatter (`related`, `tunnel`, `depends_on`); **never** wipe-all on reindex |
| Rebuild | replace **extract** edges per source file only |
| Path wikilinks | resolve relative path and `vault://` **before** label_key; stubs only when no file exists |
| Stubs | optional `stubs/*.md` or sidecar-only until file appears; promote on create |
| Tags | live `#tag` + optional frontmatter |

**Rejected:** vault reindex that rebuilds full graph.json without merge algorithm (reimplements wipe-all for explicit edges).

---

## 11. Failure modes (and mitigations)

| Failure | Mitigation |
|---------|------------|
| Wrong note on title collision | no auto-bind multi-doc; path-first; lint ambiguity |
| Explicit edges deleted on re-ingest | delete_derived only |
| Inbound edges lost on delete_document | demote-to-stub; keep inbound |
| Tag hijack of wikilink | never resolve to kind=tag |
| Document node steal by title | remove ensure_document_node title-document reuse |
| Byte/char offset mismatch | Unicode scalars end-to-end |
| Dual stubs concurrent | deterministic id + upsert |
| Stale chunk_id after rechunk | rebuild edges or null chunk_id |
| Alias exclusive lock | multi-claim + lint |
| Unclosed fence hides rest | fail-closed documented; doctor note |
| depends_on cycle | structured error |
| O(|E|) neighbors | frontier SQL |
| blake3 without migrate | version gate + merge pass |

---

## 12. Tests plan

### 12.1 Unit (extract / normalize / ids)

- label_key casefold / NFKC / whitespace  
- code fence + inline ignore; unclosed fence  
- `![[embed]]` skip  
- alias `|` and heading `#` split; block id ignored  
- Unicode tag body; mid-token `#` skip  
- multibyte: char_start/end match char indices (e.g. `café[[X]]`)  
- derived_edge_id stable across runs  
- occurrence ordering  

### 12.2 Resolve / rebuild invariants

- promote stub: **id stability**  
- re-ingest: document node id stable by uri/document_id  
- explicit `related`/`tunnel` **survive** re-ingest  
- explicit survive **document demote/delete** (inbound retained)  
- **no tag hijack** for `[[same_name_as_tag]]`  
- multi-doc same label_key: no silent bind / no steal  
- title rename: old title remains alias; historical links resolve  
- delete_derived does not remove tunnel  
- batch cap warn at 2000  
- concurrent stub create: single row  

### 12.3 Query

- neighbors frontier correctness vs small full-scan oracle  
- default PKB excludes tags/depends_on  
- backlinks multi-occurrence rows with contexts  
- graph_expand_search: hits independent of edge set  
- depends_on directed prereq/impact  

### 12.4 Vault (when adapter lands)

- path-style `[[folder/Note]]` resolves to file  
- explicit sidecar edges preserved on reindex  
- live parse after external edit  

### 12.5 Integration (existing + extend)

- ingest A with `[[B]]` + `#idea` → stub + tag; ingest B → promote; backlinks; neighbors  

---

## 13. Module layout (target)

```
src/graph/
  mod.rs              # rebuild_document_graph re-export
  extract.rs          # mask + parse → ExtractedLink (char indices)
  normalize.rs        # label_key, display_label, tag_key
  ids.rs              # blake3 stable ids
  resolve.rs          # ensure, resolve_wikilink, promote, rebuild
  chunk_map.rs        # chunk_id_for_span
src/db/graph.rs       # CRUD, delete_derived, neighbors_opts, backlinks, frontier SQL
```

Domain pure; store adapter-agnostic signatures for Markdown vault.

---

## 14. Open questions resolved by critics (decision log)

| Question | Decision |
|----------|----------|
| Byte vs char offsets? | **Unicode scalars only** end-to-end |
| Multi-edge vs UNIQUE vs weight=count? | **Partitioned identity**; multi for wikilink; aggregate is view flag |
| origin column vs rel_type ownership? | **Single delete predicate** (origin=extract OR managed rels); link_nodes sets origin=explicit |
| delete_document cascade? | **Demote to stub**; keep inbound; no silent incident wipe |
| Deterministic ids on old DBs? | **Migrate/merge first** or version gate; greenfield OK |
| NFC vs NFKC? | **One key: NFKC + lower + whitespace** as `label_key` |
| Wikilink to tags? | **Never** |
| Same-title multi-doc? | **No auto-bind**; lint ambiguity; path/uri first |
| Unique alias_key? | **No**; multi-claim + lint |
| Server layout? | **No**; client owns viz ([`EGUI_GRAPH_VIEW.md`](EGUI_GRAPH_VIEW.md): RadialLocal MVP, separate binary) |
| Orphan prune timing? | **After reinsert**; degree-0 stubs only |
| embeds as edges? | **P0 skip**; no partial Obsidian embed graph |
| Auto wikilink → depends_on? | **Never** |
| Rebuild wipe all outgoing? | **Rejected**; derived-only (current bug) |
| Prefer newest document on collision? | **Rejected** (wrong note) |
| Title-based document node reuse? | **Rejected** (steal) |
| `related` is an explicit rel_type, so the pipeline's membership edges keep that owner? | **Rejected** (2026-09-25). `resolve::append_structural_edges` emits `related` for project/directory placement; name-only attribution stamped **165 893 of 166 020** live `related` edges `explicit` (measured read-only on the gateway's auto-backup: 83 106 `project membership` + 82 787 `directory membership` vs 127 hand-made with `context IS NULL`), hiding them from §1.6 forever and stacking a fresh pair per re-ingest. Ownership is the `context` literal, shared with the writer as constants |
| Enforce §1.6 `related` idempotency and the §2 weight clamp in `link_nodes` now? | **No** (2026-09-25, measured). Read-only on the gateway's auto-backup: among 234 654 edges the *only* duplicated `(source,target,rel)` `related` triples are 7 membership rows, which the fixed §1.6 predicate already reclaims on the next rebuild; hand-made `related` edges have **zero** duplicates, and weights are **zero** non-positive / non-finite (range `[0.5, 1.0]`). Enforcing either would change a live MCP tool's semantics for no measured defect, so both stay recorded in §17 |
| 11 Cyrillic `rel_type` names in the PKB lists are dead → delete them? | **Rejected** (2026-09-25). Measured read-only on the gateway's own auto-backup: 118 live edges carry them, all `document → document`, `weight = 1.0`, `context = NULL`, `edge_origin = NULL` → the §1.6 backfill classifies them `explicit`. They are hand-authored facts, so the vocabulary grew to accept them (§1.1) instead of the lists shrinking |

---

## 15. Rejected ideas (do not reintroduce)

1. Full-edge load for BFS.  
2. Global UNIQUE(source,target,rel) for all edges.  
3. UNIQUE(alias_key) exclusive lock.  
4. Server-side force-directed layout.  
5. Mandatory LLM NER / LightRAG for core graph.  
6. Resolve-by-newest title collision.  
7. `delete_edges_from` all outgoing (wipes tunnel/related).  
8. `delete_edges_incident` as default document delete.  
9. Byte offsets in GraphEdge API.  
10. Dual label_norm NFC + label_key NFKC.  
11. Wikilink bind to tag nodes.  
12. Cascade orphan prune mid-rebuild.  
13. Weight-only multi-mention collapse as storage.  
14. Vault reindex wipe of explicit sidecar edges.  
15. Dep projection as default local graph.

---

## 16. Implementation priority

| Phase | Work |
|-------|------|
| **P0a** | label_key column + normalize; fix resolve order (no tag, no steal); delete_derived_edges_from; promote id stability tests; §1.1 store-boundary vocabulary incl. wiki semantic rel_types (2026-09-25) |
| **P0b** | char offsets on ExtractedLink/edges; multi-wikilink occurrence; GraphEdge provenance fields; neighbors frontier SQL |
| **P0c** | `graph_expand_search` only — held by the V-O4 decision; `link_nodes` tunnel support and the PKB default filters on `get_neighbors` / `get_graph` shipped 2026-09-24 |
| **P1** | unlink_nodes; resolve_stub; tunnel CRUD; node_aliases + rename retention; link_health; blake3 + migration; depends_on tools; backlinks occurrence API |
| **P1 vault** | live parse + extract-only rebuild + path resolve + explicit merge |
| **P2** | mentions extract; embeds rel; confusable detection (optional); VSS irrelevant to graph |

---

## 17. Current code debt (map to this design)

Status is as of 2026-09-25. "Closed" rows stay listed so a later reader can tell
which §1–§6 clauses once had no code behind them.

| Location | Bug vs design | Status |
|----------|----------------|--------|
| `resolve.rs` `delete_edges_from` | Wipes explicit edges → `delete_derived_edges_from` | Closed (P0a-2): scoped predicate + `origin` column, `extract`-owned rel_types only |
| `resolve_wikilink_target` prefers any resolved | Can bind tags → exclude kind=tag | Closed (P0a-3): the label tier binds a *unique* resolved document, else an unresolved stub; tags/entities are never wikilink targets |
| `ensure_document_node` title document reuse | Steal → remove block | Closed (P0a-3): uri reuse refuses a node owned by another document, and promotion skips `tag`/`entity` kinds |
| `find_nodes_by_label` case-sensitive | Permanent stubs → label_key | Closed (P0a-1): `graph/normalize.rs`, `label_key` column + migrate backfill, query keyed on `label_key` |
| `extract.rs` byte positions for context only | Add char_start/char_end; convert at boundary | Closed (P0b-1): `Hit` byte spans are resolved to unicode scalars once per document and stored on extract-origin edges |
| `db/graph.rs` neighbors loads all edges | Frontier SQL | Closed (P0b-2): `expand_frontier` + one indexed hop query per depth level for `neighbors`, `follow_tunnels` and both UI projections; §9 depth cap 5 enforced |
| `PKB_REL_TYPES` + `db/graph.rs::scoped_relation_types` | Two hand-written copies of the §7.1 default list, both naming `rel_type`s the §1.1 vocabulary rejected | Closed (2026-09-25): `models::pkb_rel_types(include_tags)` is the only list, and the SQL literals derive from it; the 11 wiki semantic names are now legal at the store boundary through `RelType::validate_wire` (§1.1), because 118 live edges carry them |
| `EXTRACT_OWNED_REL_TYPES` spelled out in three places (dead `RelType::is_extract_owned`, the §1.6 delete SQL, the origin backfill SQL) | Same name list hand-copied per site | Closed (2026-09-25): `models::EXTRACT_OWNED_REL_TYPES` is the only copy; the delete predicate builds its SQL once through `sql_string_list`, the backfill derives the same way, the unused predicate method is gone |
| Random UUID stubs/tags (nodes **and** edges) | Deterministic ids after migrate | Deferred by decision (V-O8): no blake3 id flip without the merge pass, so §1.6's `derived_edge_id` upsert key and §6.1 step 7 stay plain UUID inserts |
| `link_nodes` does not enforce §1.6 `related` idempotency or the §2 weight clamp | Repeated calls add a row; a `weight <= 0` / NaN caller weight is stored as-is | Open, deliberately (2026-09-25): measured zero hand-made duplicates and zero bad weights on the live corpus, so there is no defect to fix and enforcing would change MCP tool semantics; `unlink_nodes` (P1) is the sanctioned removal path |
| `schema::backfill_graph_edge_origins` keyed on rel_type only | `related` is an explicit name, so the pipeline's own project/directory membership rows were stamped `explicit` and escaped §1.6's delete predicate | Closed (2026-09-25): membership is attributed by the `context` literals shared with `resolve::structural_edge` (`models::STRUCTURAL_EDGE_CONTEXTS`), plus a guarded repair pass for stores that already ran the name-only version; `legacy_membership_edges_are_attributed_by_their_structural_context` locks the re-ingest behaviour |
| No origin / multi-edge / aliases | Schema + API extensions above | Closed for storage (P0a-2 origin, P0b-1 `alias`/`heading`/`chunk_id`/`char_start`/`char_end`/`occurrence`); occurrence-aware read APIs (`list_backlinks` → `BacklinkHit`, `aggregate_view`) remain P1 |
| `delete_document` wiped every incident edge | §6.3 demote-to-stub, keep inbound | Closed: `demote_graph_for_document_locked` clears `document_id`, sets `kind='stub'`/`resolved=false`, reclaims only outbound extract edges; inbound and explicit outbound stay. Applies to MCP delete, maintenance compaction and recovery replace alike, so a replaced note is an unresolved target rather than lost links |
| `label` written without `label_key` | Single §3 key must not go stale | Closed: title refresh (`refresh_document_graph_label_locked`) and dedupe promotion both rewrite `label_key`, so a renamed note keeps resolving by label |
| MCP graph reads had no §7.1 defaults | `get_neighbors` / `get_graph` must default to the literary set | Closed: `Store::neighbors_filtered` plus a defaulted `GraphFilter` in the tool layer, both fed by `pkb_rel_types` / `pkb_node_kinds`; `include_tags` and an explicit `rel_types` are the opt-outs. `Store::neighbors` stays raw for internal walking |

---

## 18. Client boundary (authoritative split)

| Concern | Owner | Doc |
|---------|-------|-----|
| Node/edge types, extract, resolve, rebuild, MCP tools | Server / domain | **this file** |
| Wire JSON `GraphView` topology | Server emits; clients consume | this file + SPEC |
| Multi-edge storage vs canvas collapse | Store keeps multi-wikilink; `aggregate_view` optional on export; **UI always collapses** for draw | this §1.6 / §7.1 · EGUI §4.3 |
| PKB default filters | Server: `rel_types=[wikilink,related]`, tags off | this §7.1; UI mirrors over HTTP and direct read-only inspection |
| Positions, layout algorithms, pan/zoom | Client only | [`EGUI_GRAPH_VIEW.md`](EGUI_GRAPH_VIEW.md) |
| Dual-writer DuckDB UI+MCP | Forbidden | Gateway-owned HTTP client or EGUI Mode C snapshot; direct `--db` is exclusive/read-only |
| Runtime dependency | Headless MCP + graph tools do not depend on UI code; integrated product visual QA is tracked separately | ROADMAP / EGUI non-goals |

Decision bullets: [`GRAPH_EGUI_DECISIONS.md`](GRAPH_EGUI_DECISIONS.md).

---

*End of GRAPH_DESIGN.md. Implementers treat §1–§6 and §14 as normative; FEATURES.md / ROADMAP priority tags schedule work, not semantics. UI design: [`EGUI_GRAPH_VIEW.md`](EGUI_GRAPH_VIEW.md).*
