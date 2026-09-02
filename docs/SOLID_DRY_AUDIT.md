# SOLID / DRY audit

**Updated:** 2026-09-02

This audit records responsibility boundaries that are present in the current
tree. File size alone is not a refactor reason; behavior-preserving seams and
tests are the acceptance criteria.

## Completed responsibility boundaries

### Document preparation and atomic persistence

`DocumentIndexer` owns chunk policy, Markdown section metadata, batch embedding
and `Chunk` construction without database access. `IngestService` and wiki/raw
write paths prepare everything before calling `Store::write_document_atomic`.
The Store transaction owns CAS/upsert, single-URI ownership, chunk replacement
and derived graph rebuild. Racing first ingests through different admission
paths resolve to one owner plus one explicit conflict; they cannot create two
documents for the same URI.

Embedding and injected graph failures preserve the previous document, chunks,
graph, wiki index and operation log. This is the main SRP/DIP boundary for all
content writes.

### Transport-independent application services

- `retrieval.rs` owns validated search commands, multi-get, chunk expansion and
  similar-document use cases.
- `diagnostics.rs` owns status and doctor reports.
- `revisions.rs` owns bounded line diff and restore-as-new-head with CAS.
- Wiki writes use `WikiWriteCommand`; maintenance uses
  `MaintenanceExecutionContext`.

MCP and HTTP preserve their wire defaults and envelopes but do not reimplement
these workflows. This removes transport-level copies without erasing deliberate
compatibility differences.

`DiagnosticsService::status` is the single HTTP/MCP status owner. Store supplies
raw/wiki/index coverage and uncompiled-raw debt as one SQL aggregate, preserving
the existing health semantics without materializing document bodies or issuing
one backlinks query per raw document.

### Incremental source synchronization

`SourceSyncService` owns allowlist validation, scan policy, manifest preflight,
repair detection and explicit deleted-source pruning. The source-manifest
repository owns its SQL and root summaries. Progress/cancellation is a service
contract, while `http_api::jobs` owns process-local lifecycle, retention,
admission and the serialized background writer lane.

The Store owns a process-local read/write coordination lane: a source sync keeps
the write guard for its complete run, while `lex`/`hybrid` acquire a non-blocking
read guard before query embedding. This makes concurrent searches compatible,
prevents a sync from crossing an already-admitted search, and returns a
retryable `AppError::Busy` without embedding/FTS work when sync is active. HTTP
and MCP remain thin, policy-specific mappings to 503 + `Retry-After` and
structured `STORE_BUSY` metadata respectively.

Unchanged healthy files skip extraction, hashing and embedding. The existing
MCP `sync_sources` path remains a synchronous adapter over the same service.
Changed files are prepared without I/O to the embedding provider, grouped into
bounded cross-document embedding batches, then committed individually in
canonical path order through the same atomic/CAS persistence boundary. A batch
provider failure writes none of that batch; cancellation before commit drops it,
while cancellation between commits preserves an exact deterministic prefix.
Manifest ownership survives parent ↔ child root changes; oversized files are
reported as per-file errors instead of silently remaining stale; and
`delete_source_state` removes documents, chunks, graph, wiki index and manifest
rows in one transaction. Jobs distinguish clean success from
`completed_with_errors`, and cancellation preserves a terminal result when it
races completion.

### Retrieval index ownership

FTS dirty/generation state and single-flight refresh live in `db::fts`. Exact
vector candidate loading, normalized snapshot caching and bounded top-k live in
`db::search`. Mutation paths advance one shared chunk generation. A non-empty
document/project/room/URI/source scope is materialized with a SQL join into a
transient normalized snapshot; it neither reads nor populates the database-wide
cache. The unscoped path retains the generation-keyed global snapshot.

Search orchestration does not know DuckDB FTS DDL or vector cache invalidation.
Project filters are applied before scoring instead of after a global top-k.

Project graph lookup follows the same rule: SQL selects project documents and
their direct companion nodes before deterministic traversal. The induced graph
is bounded to 300 nodes and neighbor expansion to depth 3, without admitting
documents owned by another project.

Schema owns non-unique lookup indexes for `documents.uri` and
`graph_nodes.uri`. They accelerate recovery, document-node reuse and wikilink
resolution without moving URI-ownership policy out of the Store CAS boundary.

### HTTP boundary and operational privacy

One request-body middleware wraps both product HTTP routes and the mounted MCP
router. It enforces a 1 MiB limit even when `Content-Length` is absent. Activity
keeps bounded operational lineage with anonymous client identifiers; it does
not retain raw IP/UA, request or result bodies, source paths, titles or search
queries.

### Persistence codecs and focused repositories

Document/chunk/manifest row decoding lives in `db::rows`. Catalog aggregation,
source manifests, FTS, search, graph, KG, recovery and vault export have focused
`db/*` owners while `Store` remains the compatibility façade and transaction
owner.

Timestamp and slug mechanics are shared only where policy is genuinely shared:
`util::time` retains strict and date-flexible modes, and `SlugPolicy` preserves
wiki/index/graph differences.

### Recovery publication and transactional collision policy

`db::recovery::publish_recovery_artifact` is the shared MCP/offline bundle
publication seam. It stages and syncs complete bytes beside the destination,
performs a race-safe no-clobber publish by default or an atomic replace only
after explicit overwrite, syncs the parent directory and cleans the stage on
all outcomes. The offline adapter additionally refuses the live database target
or another path to the same inode.

Bundle overwrite resolves at most one existing document by id or URI. A cross
collision that selects two distinct rows is a conflict before deletion; the
surrounding import transaction rolls back both that item and any earlier item
from the same bundle. Persistence, rather than either transport, owns graph,
chunk and wiki-index cleanup for the single valid replacement.

### Native product workspaces

Gateway DTO/load behavior is split into `product`, `search`, `operations` and
`revisions`; workspace rendering lives under `ui/`. `app.rs` coordinates
navigation and worker sequence ownership rather than performing HTTP or
persistence itself. Project Home, Unified Library, Search, Operations and
History use lean server-side APIs instead of loading the corpus into the UI
process. Revision summaries are cursor-paginated and snapshots load only after
selection. Native tests cover polling/cancellation, destructive backup
confirmation and CAS revision restore; the service refuses restore for
immutable raw documents.

The gateway adapter is also the single UI error-disclosure boundary: raw
transport failures become stable timeout/connect/generic messages, while safe
HTTP formatting retains status, operation context, structured code and the
server-provided human-readable message rather than the raw envelope. UI state owns
non-cancellable mutations until their matching sequence completes. Project
switching cannot discard wiki save, revision restore or Operations mutation
state; wiki save freezes edit/navigation actions, restore cannot be locally
cancelled, and verified backup alone uses a 30-minute request timeout.

## Deliberate compatibility boundaries

1. `mcp/facade.rs` remains the rmcp macro composition root. Tool families have
   bounded routers; moving macro methods without framework support would add
   forwarding code rather than reduce responsibility.
2. `Store` remains the public DuckDB façade and transaction owner. Focused
   repository modules may move algorithms out, but callers do not receive raw
   DuckDB connections.
3. Long-form wiki functions remain compatibility adapters over command APIs
   until a versioned deprecation window.
4. `wing` remains the v1 persistence/wire alias for project. A physical rename
   requires a dual-write migration and rollback plan.
5. The `Storage` trait intentionally covers document lifecycle only. Claiming a
   full multi-backend abstraction before search/graph/wiki conformance would
   violate interface segregation and capability honesty.

## Remaining measurable debt

| Boundary | Current limitation | Completion test |
|----------|--------------------|-----------------|
| Source manifest | Manifest metadata commits after the atomic document write, so a crash can leave repairable stale metadata | Failure injection proves the next sync repairs without duplicate documents or missed deletion; fold into the document transaction only if exact atomic metadata becomes required |
| Storage | Markdown has document CRUD, sidecar rebuild and watcher, but not application search/graph/transaction parity | One shared conformance suite passes for every capability advertised by both DuckDB and Markdown; unsupported capabilities return structured refusal |
| Large orchestration roots | `Store`, `wiki`, `mcp/facade` and native `app` still coordinate many compatibility paths | Extract a slice only when its behavior tests can run through the new boundary and the old façade becomes delegation-only for that slice |

## Guardrails

- Preserve HTTP and MCP wire shapes during internal refactors.
- Keep production DuckDB writes behind the live one-writer gateway.
- Do not deduplicate code whose similar shape encodes different mutation,
  timestamp, slug or compatibility policy.
- Every structural slice runs targeted regression tests, full workspace tests,
  strict Clippy and `git diff --check` before integration.
