# System map

**Product:** `rag-mcp`

**Updated:** 2026-09-02

**North star:** [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md)

**Laws:** [`PRODUCT_PRINCIPLES.md`](PRODUCT_PRINCIPLES.md)

This is the current ownership map. It describes code that exists; target work
belongs in [`ROADMAP.md`](ROADMAP.md).

## 1. Logical layers

```text
L4  MCP, HTTP and native client ports
 │
L3  compiled knowledge: wiki, index, schema, revisions, ops log
 │
L2  object graph: document nodes, tags, stubs, tunnels, provenance
 │
L1  retrieval: chunks, embeddings, FTS, exact vector cache, RRF
 │
L0  verbatim corpus: documents, project/room, source, hash, lifecycle
 │
SoT default: one DuckDB file owned by one gateway process
```

| Layer | Durable artifacts | Application entry points |
|-------|-------------------|--------------------------|
| L0 | `documents`, `source_manifest`, immutable raw bodies | ingest, source sync, document catalog, lifecycle |
| L1 | `chunks`, embedding manifest, FTS generation state | lex/vec/hybrid search, packing, expand, similar |
| L2 | `graph_nodes`, `graph_edges` | scoped graph, neighbors, backlinks, tunnels |
| L3 | wiki pages/index/schema, document revisions, `ops_log` | wiki CRUD, diff/restore, compile, maintenance |
| L4 | MCP tool routers, HTTP routes, native workspaces | adapters only; domain policy stays below the port |

The temporal `kg_facts` and diary surfaces remain supported but are not separate
sources of truth. They share the same store and are outside the default
compile-first spine.

## 2. Runtime topology

```text
MCP clients ── stdio or streamable HTTP /mcp ─┐
                                               │
native client ── /v1/* HTTP ──────────────────┼── rag-mcp gateway
                                               │      │
scheduled sync / backup ──────────────────────┘      ▼
                                              shared Store
                                                   │
                                              rag.duckdb
```

The gateway is the sole live DuckDB writer. MCP and `/v1/*` handlers share the
same `Store`, embedder and configuration. HTTP source-sync jobs use a serialized
writer lane and cooperative cancellation; they do not open another database.
Store-level CAS enforces one URI owner when synchronous MCP/autosync work races
the HTTP lane.

The native client uses `--http` for live work. `--snapshot` is read-only and
`--db` is an exclusive maintenance/development mode.

## 3. Code ownership

| Path | Owns |
|------|------|
| `ingest.rs` | transport-independent ingest command, immutable policy and orchestration |
| `document_indexer.rs` | pure chunk policy, section metadata, embedding and `Chunk` construction; no storage writes |
| `source_scan.rs`, `source_sync.rs` | source discovery, manifest-aware incremental sync, progress and cancellation |
| `retrieval.rs`, `search_pack.rs` | validated search use cases, multi-get/expand/similar and token packing |
| `revisions.rs` | revision diff and CAS-protected restore use cases |
| `diagnostics.rs`, `ops.rs` | health/doctor and scheduled operational state |
| `graph/` | pure wikilink/tag extraction and resolution |
| `wiki/` | wiki/schema/index/compile behavior over the ingest and atomic-write seams |
| `maintain/`, `memory_lifecycle.rs`, `diary/` | explicit maintenance and memory workflows |
| `db/store.rs` | compatibility DuckDB façade and transaction boundary |
| `db/{rows,fts,search,graph,kg,catalog,source_manifest,recovery,vault}.rs` | persistence repositories and backend-specific algorithms |
| `storage/` | small backend-neutral document contract; DuckDB and Markdown implementations |
| `mcp/facade.rs`, `mcp/{ingest,search,graph,wiki,kg,maintain,recovery,collections}.rs` | macro composition root and bounded MCP tool routers |
| `http_api/{health,ops,jobs,retrieval,graph,wiki,activity}.rs` | thin HTTP route clusters |
| `crates/rag-mcp-ui/src/{product,search,operations,revisions}.rs` | native gateway DTOs and application adapters |
| `crates/rag-mcp-ui/src/ui/` | workspace rendering; `app.rs` coordinates navigation and worker state |

## 4. Structural seams

| Seam | Contract | Enforced behavior |
|------|----------|-------------------|
| `DocumentIndexer` → `Store::write_document_atomic` | prepare before persistence, then commit document/chunks/derived graph together | embedding or graph failure preserves the previous state; URI CAS permits exactly one owner under racing initial ingest |
| `SourceSyncService` → source manifest repository | stat preflight avoids extraction/hash/embed for healthy unchanged files | parent ↔ child root rebinding preserves ownership; oversized files are reported; `delete_source_state` removes source-owned state transactionally |
| Retrieval service → `db::search`/`db::fts` | transports pass validated commands, persistence owns candidate loading and ranking substrate | project filters run before exact vector top-k; cache invalidates by chunk generation |
| Project graph HTTP → `db::graph` | scope project documents and direct companions in SQL before traversal | deterministic BFS is capped at 300 nodes/depth 3 and excludes documents from another project |
| MCP/HTTP → application services | ports map wire values and response envelopes only | ingest, retrieval, diagnostics and revisions do not depend on a transport |
| Gateway → jobs/operations | one process owns writes and maintenance | bounded job retention, hard admission cap, `completed_with_errors`, terminal-safe cancellation and safe checkpoint/backup paths |
| HTTP boundary → product API and mounted MCP | one middleware owns admission and Activity sanitization | headerless bodies over 1 MiB are rejected; raw IP/UA, bodies, source paths and titles are not retained |
| Native client → gateway | product workspaces consume lean APIs | no second live DuckDB reader/writer is required for normal use |
| `Storage` | backend identity, capabilities and document lifecycle | unsupported backends/capabilities fail explicitly; no silent DuckDB alias |

Compatibility boundaries are intentional: `mcp/facade.rs` remains the rmcp
macro composition root, `Store` remains the public DuckDB façade, and `wing`
remains the v1 wire/storage alias for project.

## 5. Main workflows

### Ingest and sync

```text
allowlist → extract/normalize → DocumentIndexer → embed
          → atomic document + chunks + graph write
          → source manifest / operational metadata
```

Source sync first loads one root manifest view. Size/mtime plus document/chunk
health can skip extraction and embedding. Root scope can move between a parent
and child directory without losing ownership rows. Oversized sources become
visible per-file errors, and removal commits documents, chunks, graph, wiki
index and manifest state together. Background HTTP runs publish scan, sync,
delete and terminal progress phases.

### Query

```text
index/wiki navigation → scoped lex/vec/hybrid retrieval → optional graph expand
                    → packed cited context → client-owned answer/synthesis
```

FTS is rebuilt only when its recorded chunk generation is stale. Exact vector
search uses a generation-keyed normalized snapshot, scopes candidates before
scoring and hydrates content only for winners.

### Product navigation

```text
Project Home → Unified Library → document/wiki/graph
             → Search ──────────┘
             → Operations (activity, health, jobs, checkpoint, backup)
             → History (timeline, diff, CAS restore) from a document
```

Project graph queries scope in SQL before the 300-node/depth-3 caps; lazy
neighbor expansion cannot cross the selected project. History fetches a lean,
cursor-paginated timeline and loads a full revision snapshot only after the
user selects it; raw documents cannot be restored as mutable heads.

## 6. Storage truth

DuckDB is the full application backend. `Storage` has landed as a deliberately
small document-lifecycle contract rather than a fake all-backend façade.

| Backend | Current capability |
|---------|--------------------|
| DuckDB | documents, chunks, FTS, vectors, graph, wiki, revisions, transactions, recovery |
| Markdown | opt-in document CRUD, frontmatter source of truth, deterministic sidecar rebuild and watcher |
| SQLite/Postgres/Memory | recognized identifiers only; startup refuses them |

Search, graph, wiki and maintenance still depend on the DuckDB surface. A full
Markdown application backend requires the conformance gate in the roadmap.

## 7. Current product surface

| Area | Status | Boundary |
|------|--------|----------|
| Project Home and Unified Library | shipped | HTTP mode; lean catalog never loads every body |
| Search workspace | shipped | gateway lex/vec/hybrid API; no implicit LLM rewrite |
| Wiki reader/editor | shipped | CAS conflict recovery; raw bodies remain immutable |
| Project Connections graph | shipped | SQL-scoped project graph; deterministic local expansion capped at 300 nodes/depth 3 |
| Activity | shipped | in-process bounded sanitized event history, not an audit-log replacement |
| Operations API | shipped | status/doctor, jobs with explicit partial-success state, checkpoint and allowlisted backup |
| Revision API | shipped | lean cursor pagination, lazy snapshot, bounded line diff and CAS restore-as-new-head with raw guard |
| Native Operations and revisions flows | shipped | jobs poll/cancel, health/backup and revision diff/CAS restore; native package tests are green |
| Retrieval scale | local observation | 100,111 chunks: 133.98 ms hybrid p95; exact path remains default until representative release reruns fail the gate |

## 8. Remaining concentration and measurable gates

| Concentration | Current risk | Gate |
|---------------|--------------|------|
| `Store`, `wiki`, `mcp/facade` and native `app` remain large compatibility/orchestration roots | Mechanical splitting could duplicate policy or break wire behavior | New workflows must enter through existing application seams; extract only with behavior tests |
| Markdown implements only the document slice | Capability names can be mistaken for full parity | Shared search/graph/recovery conformance before it is marketed as a full backend |
| Exact vector search is O(n) | Future corpora may exceed latency/RSS targets | ANN only after the roadmap's repeated 300 ms or resource gate fails |
| Native layout has unit tests but no deterministic screenshot suite | Visual regressions can pass compilation | Add screenshot baselines only under the roadmap's visual-regression entry condition |

## 9. Documentation authority

1. Product identity: `ARCHITECTURE_VISION.md` and `PRODUCT_PRINCIPLES.md`.
2. Current shipped behavior: code, `README.md`, then this map.
3. Sequencing and measurable gates: `ROADMAP.md`.
4. Deep contracts: `ARCHITECTURE_NOTES.md`, graph/storage/topic docs.
5. Research history: `FEATURES.md`, `REPOS_SURVEYED.md`,
   `BACKLOG_500.md`, and tool matrices. They are not execution queues.

Operator setup remains in [`README.md`](../README.md); HTTP details remain in
[`CONNECT.md`](CONNECT.md) and [`PROD_RUN.md`](PROD_RUN.md).
