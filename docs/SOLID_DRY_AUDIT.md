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

## Highest-priority remaining debt

1. `mcp/facade.rs` is still a large composition root and tool adapter. Move
   ingest/re-embed orchestration into an application service with request
   objects; keep MCP-specific error and response mapping in the facade.
2. Wiki write/update/ingest functions and maintenance action handlers still
   have long parameter lists. Introduce command objects and small execution
   contexts before adding more flags.
3. Slug functions in store, wiki, and graph look similar but have different
   policies (fallback value, slash handling, preserved punctuation). Model the
   policy explicitly before sharing code; a blind helper merge would change
   identifiers.
4. `db/store.rs` combines persistence operations and row mapping for many
   aggregates. Extract row codecs by aggregate first; do not split transaction
   ownership across repositories.
5. Search construction is repeated across several tools. Add a validated search
   request builder only after transport defaults are recorded as compatibility
   tests.

## Guardrails

- Preserve HTTP and MCP wire shapes during internal refactors.
- Keep DuckDB writes behind the live one-writer gateway in production.
- Commit one structural slice at a time and run targeted tests plus
  `cargo check --all-targets` before integration.
- Do not deduplicate code whose superficially similar behavior encodes different
  domain policy.
