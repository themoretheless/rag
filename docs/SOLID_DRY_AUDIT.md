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

## Highest-priority remaining debt

1. `mcp/facade.rs` remains a large composition root because tool registration
   and several recovery/collection adapters still share one implementation.
   Split registration by bounded capability without duplicating tool policy.
2. Wiki update/compile/consolidate compatibility functions still have long
   parameter lists. Migrate internal callers to command APIs before deprecating
   the old library surface.
3. `db/store.rs` combines persistence operations and row mapping for many
   aggregates. Extract row codecs by aggregate first; do not split transaction
   ownership across repositories.
4. Search construction is repeated across several tools. Add a validated search
   request builder only after transport defaults are recorded as compatibility
   tests.

## Guardrails

- Preserve HTTP and MCP wire shapes during internal refactors.
- Keep DuckDB writes behind the live one-writer gateway in production.
- Commit one structural slice at a time and run targeted tests plus
  `cargo check --all-targets` before integration.
- Do not deduplicate code whose superficially similar behavior encodes different
  domain policy.
