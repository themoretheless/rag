# SOLID / DRY audit

This audit is tied to the current source tree. It separates completed structural
changes from remaining work so architecture claims stay testable.

## Completed

### Transport-independent retrieval

`src/retrieval.rs` now owns document lookup, ordered multi-get, neighbor chunk
expansion, and similar-document search. MCP and HTTP are adapters over the same
use cases instead of maintaining separate validation, centroid, and filtering
implementations.

Principles improved: SRP, DIP, DRY. The transports depend on a domain use-case
module and preserve their existing response envelopes.

### Reading-first native viewer

The wiki renderer no longer owns backlinks and page metadata. Those live in an
independently hideable information panel. The catalog is independently
hideable, article text has a bounded readable width, and render dependencies are
grouped in `WikiReadContext` instead of an argument-heavy function.

Principles improved: SRP, ISP. The reading surface, navigation surface, and page
information surface can evolve separately.

### Timestamp conversion

RFC3339 and DuckDB timestamp parsing/formatting live in `util::time`. The KG
layer explicitly opts into the extra date-only form; persistence and corpus
analysis use the stricter database form.

Principles improved: DRY without erasing domain-specific policy.

### Ingest application service

`IngestService` owns normalization, immutable-source policy, chunking,
embedding, persistence, graph refresh, and document re-embedding. The MCP
facade delegates to this service instead of implementing the workflow.

Principles improved: SRP, DIP. Ingest behavior is now transport-independent and
accepts an `IngestCommand` rather than accumulating more facade parameters.

### Wiki commands and maintenance context

Wiki writes expose `WikiWriteCommand`; MCP and HTTP use it while legacy
functions remain compatibility adapters. Maintenance plan execution carries
shared dependencies and execution policy in `MaintenanceExecutionContext`
instead of forwarding the same six arguments through every action handler.

Principles improved: ISP, SRP, DRY.

### Explicit slug policies

Shared slug mechanics now require a named `SlugPolicy`. Wiki pages, index
fallbacks, and graph link targets retain their different slash, punctuation,
and empty-value behavior without maintaining three loop implementations.

Principles improved: DRY while keeping policy differences explicit.

### Diagnostics and document updates

`DiagnosticsService` now supplies HTTP and MCP status/doctor payloads without
constructing or depending on an MCP server. Document metadata and body updates
run through `IngestService`, including conditional re-chunking, re-embedding,
graph synchronization, title-only node updates, and operation logging.

Principles improved: SRP and DIP. Transport code no longer owns diagnostics or
document mutation workflows.

### Validated search commands

`SearchCommand` and the retrieval application service now own mode parsing,
filter normalization, diversity/grouping policy, limits, context expansion,
and embedding preparation. HTTP and MCP retain their compatibility defaults
without constructing independent `SearchQuery` values.

### Projects, revisions, and safe ingest preparation

Project ids are validated domain values and `/v1/projects` exposes the catalog
without breaking the existing `wing` wire alias. The native viewer uses a
project picker. Schema v8 preserves immutable document snapshots before each
successful update and `/v1/revisions` exposes history. Ingest prepares chunks
and embeddings before it removes active chunks.

### Persistence codecs and operational state

Document, chunk, and embedding-manifest row codecs live in `db::rows`, leaving
transaction ownership in `Store`. Automatic backup status is reconstructed
from retained snapshots after process restart instead of reporting a false
`null` completion time.

## Deliberate compatibility boundaries

These are not active refactor tasks without a versioned compatibility decision:

1. `mcp/facade.rs` remains the macro-owned tool implementation/composition
   root. Capability registration is already split into bounded routers. Moving
   individual macro methods should wait until rmcp supports composable handler
   implementations or a v2 tool surface justifies the churn.
2. Long-form wiki functions remain public compatibility adapters over command
   APIs. Removing them requires a major-version deprecation window; new internal
   transport code must use command APIs.
3. `wing` remains the v1 persistence/wire alias for `ProjectId`. A physical
   `project_id NOT NULL` column belongs to a v2 migration with dual-write and
   rollback, not a silent schema rename.

## Guardrails

- Preserve HTTP and MCP wire shapes during internal refactors.
- Keep DuckDB writes behind the live one-writer gateway in production.
- Commit one structural slice at a time and run targeted tests plus
  `cargo check --all-targets` before integration.
- Do not deduplicate code whose superficially similar behavior encodes different
  domain policy.
