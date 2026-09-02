# System map

**Product:** `rag-mcp`

**Updated:** 2026-09-03

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
writer lane, immediate queued cancellation and cooperative running cancellation;
they do not open another database. Changed files share bounded embedding batches
before ordered per-document atomic commits. The same process-local lane is a
read/write lock: source sync and guarded corpus-scale doctor repair, duplicate
cleanup, delete-by-source, recovery import, and maintenance
apply/refresh/compress own its write side through derived-index finalization.
Admitted `lex`/`hybrid` requests hold read guards, while new ones fail fast with
generic retryable `STORE_BUSY` before embedding when an exclusive corpus
mutation is active.
Store-level CAS enforces one URI owner when synchronous MCP/autosync work races
the HTTP lane.

Source sync exposes its finalization as `refreshing_fts` and publishes
`succeeded` or `completed_with_errors` only after its committed text/row changes
are FTS-clean. Other guarded corpus-scale workflows likewise finalize stale FTS
before normal terminal success. Ordinary single-document authoring, plus
failed or interrupted workflows, retain the same next-read single-flight
fallback. Embedding-only updates advance vector/chunk generation while
preserving an already-clean lexical generation and never hide prior FTS debt.
Finalization failure is an additive structured aggregate: committed
counters/actions remain visible alongside `FTS_FINALIZATION_FAILED`,
`durable_mutation_committed`, retryability, and dirty-marker outcome; a
source-sync job becomes `completed_with_errors`.
Embedding identity migration persists an incompatible marker before its first
vector write; partial failure and runtime rollback to the old identity remain
blocked until a complete uncapped reembed publishes the target manifest. A
missing manifest on a corpus that already has chunks is also fail-closed: the
gateway remains available for diagnosis and complete `reembed_all`, but it does
not infer vector identity from runtime configuration.

The native client uses `--http` for live work and every Wiki mutation.
`--snapshot` is read-only; direct `--db` is an exclusive, strictly read-only
Wiki/Connections inspection mode.

Verified gateway backup briefly holds the Store mutex for `CHECKPOINT` and
cloning a dedicated DuckDB connection. A read transaction on that clone pins
one MVCC source generation; `COPY FROM DATABASE` writes and verifies a staged
database without retaining the shared Store mutex, so normal Store queries can
continue during the long copy. Publication is serialized per destination: the
checksum and metadata sidecars are staged and synced first, and the verified
database path is published last as the generation commit marker.

## 3. Code ownership

| Path | Owns |
|------|------|
| `ingest.rs` | transport-independent prepare/embed-many/atomic-commit ingest orchestration and immutable policy |
| `document_indexer.rs` | pure chunk policy, section metadata, cross-document embedding and `Chunk` construction; no storage writes |
| `source_scan.rs`, `source_sync.rs` | source discovery, manifest-aware bounded batching, ordered progress and cancellation |
| `retrieval.rs`, `search_pack.rs` | validated search use cases, multi-get/expand/similar and token packing |
| `revisions.rs` | revision diff and CAS-protected restore use cases |
| `diagnostics.rs`, `ops.rs` | shared HTTP/MCP health/status/doctor policy, aggregate layer health and scheduled operational state |
| `graph/` | pure wikilink/tag extraction and resolution |
| `wiki/` | wiki/schema/index/compile behavior over the ingest and atomic-write seams |
| `maintain/`, `memory_lifecycle.rs`, `diary/` | explicit maintenance and memory workflows |
| `db/store.rs` | compatibility DuckDB façade and transaction boundary |
| `db/{rows,fts,search,graph,kg,catalog,source_manifest,recovery,vault}.rs` | persistence repositories, scoped/global retrieval snapshots, aggregate health, atomic single-artifact recovery publication and coordinated backup-group publication |
| `storage/` | small backend-neutral document contract; DuckDB and Markdown implementations |
| `mcp/facade.rs`, `mcp/{ingest,search,graph,wiki,kg,maintain,recovery,collections}.rs` | macro composition root and bounded MCP tool routers |
| `http_api/{health,ops,jobs,retrieval,graph,wiki,activity}.rs` | thin HTTP route clusters |
| `crates/rag-mcp-ui/src/{product,search,operations,revisions}.rs` | native gateway DTOs and application adapters |
| `crates/rag-mcp-ui/src/ui/` | workspace rendering; `app.rs` coordinates navigation and worker state |

## 4. Structural seams

| Seam | Contract | Enforced behavior |
|------|----------|-------------------|
| `DocumentIndexer` → `Store::write_document_atomic` | prepare before persistence, then commit document/chunks/derived graph together | embedding or graph failure preserves the previous state; URI CAS permits exactly one owner under racing initial ingest |
| `SourceSyncService` → ingest/manifest/FTS repositories | stat preflight avoids extraction/hash/embed for healthy unchanged files; changed files batch provider work before ordered commits; terminal completion refreshes stale FTS before releasing the lane | provider failure writes none of a pending batch; cancellation leaves no batch or an exact committed prefix; root rebinding and transactional deletion preserve ownership; the first search after a completed bulk sync does not pay the rebuild |
| Retrieval service → corpus-mutation lane + `db::search`/`db::fts` | transports pass validated commands; retrieval admits lexical work before embedding; persistence owns candidate loading and ranking substrate | an active exclusive corpus mutation yields generic retryable `STORE_BUSY`; project/document/source scopes materialize in SQL before exact vector top-k and bypass the global cache; unscoped cache invalidates by chunk generation |
| Project graph HTTP → `db::graph` | scope project documents and direct companions in SQL before traversal | deterministic BFS is capped at 300 nodes/depth 3 and excludes documents from another project |
| MCP/HTTP → application services | ports map wire values and response envelopes only | ingest, retrieval, diagnostics and revisions do not depend on a transport; both status ports use the same aggregate diagnostics service |
| Gateway → jobs/operations | one process owns writes and maintenance | bounded job retention, hard admission cap, `completed_with_errors`, terminal-safe cancellation and safe checkpoint/backup paths; HTTP busy includes 503 + `Retry-After`, MCP busy includes structured retry data; maintenance `max_docs` and duplicate-cleanup `max_candidates` cannot exceed `RAG_MAINT_MAX_DOCS` |
| HTTP boundary → product API and mounted MCP | one middleware owns admission and Activity sanitization | request bodies over 1 MiB are rejected even without `Content-Length`; that limit does not describe response size; raw IP/UA, request/result bodies, source paths, titles and concrete job resource IDs are not retained |
| Recovery adapters → `db::recovery` | MCP and offline CLI share staged artifact publication and transactional v2 import policy; gateway backup uses an MVCC snapshot connection outside the shared Store mutex | the verified backup database is the final commit marker for its serialized database/checksum/metadata generation; v2 carries a canonical embedding manifest and validates vector dimensions/target identity; JSON/JSONL materialization is capped at 64 MiB, 10,000 documents and 50,000 chunks; default no-clobber survives destination races; explicit overwrite replaces each published artifact atomically; an id/URI cross-collision rolls back the whole bundle; vector-bearing v1 is accepted only by gateway opt-in re-embedding, while the offline CLI refuses it |
| Native client → gateway | product workspaces consume lean APIs through one sanitized transport boundary; `/v1/projects` is authoritative independently from graph topology | graph failure is isolated to Connections; catalog failure retains the last authoritative list and has a targeted retry; Search responses retain submitted-request provenance; Library cursors reuse applied filters only; backlinks are project-scoped and failures stay visible; no second live DuckDB reader/writer is required; direct `--db` Wiki is read-only; project changes cannot drop in-flight wiki, restore or Operations mutations; verified backup gets a 30-minute timeout |
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
          → FTS refresh before terminal completed sync
```

Source sync first loads one root manifest view. Size/mtime plus document/chunk
health can skip extraction and embedding. Root scope can move between a parent
and child directory without losing ownership rows. Oversized sources become
visible per-file errors, and removal commits documents, chunks, graph, wiki
index and manifest state together. Background HTTP runs publish scan, sync,
delete, `refreshing_fts` and terminal progress phases. A run that reaches clean
or partial-success completion keeps the lane until FTS matches the committed
chunk generation.

### Query

```text
index/wiki navigation → scoped lex/vec/hybrid retrieval → optional graph expand
                    → packed cited context → client-owned answer/synthesis
```

FTS is rebuilt only when indexed text/rows or prior debt leave its recorded
generation stale. Embedding-only updates advance vector/chunk generation while
preserving an already-clean lexical generation. Guarded corpus-scale workflows
finalize stale FTS eagerly before normal terminal success; ordinary authoring
and failed/interrupted workflows refresh once on the next lexical/hybrid read.
Exact vector search uses a generation-keyed normalized global snapshot when
unscoped.
Non-empty document/project/room/URI/source scopes instead select matching chunks
and layer/kind/status/archive policy in SQL, build a transient snapshot, score
inside that scope and hydrate content only for winners; scoped reads never churn
the global cache. Document and graph-node URI lookup has dedicated schema indexes,
while Store CAS remains the ownership authority.

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
user selects it; raw documents cannot be restored as mutable heads. The project
catalog has an independent request/retry lifecycle and preserves its last
authoritative list on failure; only Connections depends on graph availability.

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
| Project Home and Unified Library | shipped | HTTP mode; authoritative project catalog is independent of graph and retains last success on refresh failure; lean document catalog never loads every body; cursor navigation is disabled until edited filters are applied |
| Search workspace | shipped | gateway lex/vec/hybrid API; results retain submitted-request provenance and flag later control edits; scoped vector snapshots; retryable fast-busy lexical contract during any guarded exclusive corpus mutation; no implicit LLM rewrite |
| Wiki reader/editor | shipped | HTTP writes use CAS and the sole-writer gateway; update preserves omitted kind/category/summary plus placement, lifecycle/pin/boost state, source ownership and unrelated metadata; direct `--db` Wiki is strictly read-only; backlinks are project-scoped and expose transport/DB failure separately from empty success; raw bodies remain immutable |
| Project Connections graph | shipped | SQL-scoped project graph; deterministic local expansion capped at 300 nodes/depth 3 |
| Activity | shipped | in-process bounded sanitized event history, not an audit-log replacement |
| Operations API | shipped | aggregate status/doctor, jobs with explicit partial-success state, checkpoint and allowlisted backup |
| Revision API | shipped | lean cursor pagination, lazy snapshot, bounded line diff and CAS restore-as-new-head with raw guard |
| Native Operations and revisions flows | shipped | sanitized errors, mutation-safe project/editor state, jobs poll/cancel, health, 30-minute backup and revision diff/CAS restore |
| Recovery bundles | shipped | v2 JSON/JSONL is capped at 64 MiB, 10,000 documents and 50,000 chunks; staged atomic no-clobber file export and whole-transaction rollback on id/URI cross-collision |
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
