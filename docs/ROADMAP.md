# rag-mcp roadmap

**Updated:** 2026-09-02

**North star:** [`ARCHITECTURE_VISION.md`](ARCHITECTURE_VISION.md)

**Architecture truth:** [`SYSTEM_MAP.md`](SYSTEM_MAP.md)

**Structural audit:** [`SOLID_DRY_AUDIT.md`](SOLID_DRY_AUDIT.md)

This file contains sequencing, not feature research. `FEATURES.md`,
`BACKLOG_500.md`, and the tool matrices are historical input; unchecked items in
those files are not commitments.

## Shipped baseline

| Area | Current implementation | Verification boundary |
|------|------------------------|-----------------------|
| Retrieval | Generation-aware FTS, exact global vector snapshot cache, SQL-materialized transient snapshots for project/document/source scopes, URI lookup indexes, lex/vec/hybrid search, citations, packing and evaluation CLI | Selective vector scopes neither consult nor populate the global cache; exact search remains the default while the measured scale gate below passes |
| Ingest | Pure `DocumentIndexer`; changed source files share bounded embedding batches of at most 64 documents/64 chunks while ordered document, chunks, manifest and derived graph commits remain atomic; URI ownership is CAS-protected; source manifests survive parent/subdirectory root rebinding; oversized files are reported; deleted source state is removed transactionally; exact legacy source duplicates have a dry-run-first, reference-aware bounded cleanup | A provider failure writes none of its pending batch; cancellation drops the uncommitted batch or leaves an exact committed prefix; Store CAS resolves races between prepare and commit; duplicate cleanup converges in deterministic batches and skips ambiguous or protected history |
| One-writer operations | HTTP/MCP share one `Store`; source sync owns the write side of a process-local coordination lane for its full run while `lex`/`hybrid` use non-blocking read guards; jobs have progress, immediate queued cancellation, terminal-safe running cancellation, bounded retention and `completed_with_errors`; checkpoint and verified backup stay in the gateway | Active sync fails new lexical/hybrid work before embedding with HTTP 503 + `Retry-After` or structured MCP `STORE_BUSY`; job state is process-local and running cancellation is cooperative |
| Product API | Project Home, lean server-filtered Unified Library, search, SQL-scoped bounded project graph, lean paginated revisions with lazy snapshots/diff/restore and operational endpoints; status layer/index health is one aggregate query | All HTTP/MCP bodies are capped at 1 MiB; Activity retains sanitized operational metadata only; HTTP and MCP status share `DiagnosticsService` |
| Native client | Home, Library, Search, Wiki, Connections and Operations workspaces; safe transport errors; mutation-owned wiki/restore/Operations state; live activity/jobs/health, 30-minute backup and revision diff/CAS restore | Project switching cannot invalidate an in-flight mutation; HTTP is normal live mode while snapshot and exclusive `--db` remain limited modes |
| Recovery and portability | Verified DuckDB backup; atomically published no-clobber bundle export; transactional import that rejects id/URI cross-collisions before deletion; vault export; backend capability reporting | Explicit overwrite is required for atomic replacement; a cross-collision rolls back every item in the bundle; DuckDB is the only full application backend |
| Storage seam | `Storage` document contract with DuckDB and opt-in Markdown implementations; Markdown sidecar index and watcher | Search, graph, transactions, wiki and maintenance still use DuckDB APIs |

## Current release gate

The current product pass is complete only when all of these are green on the
same tree:

1. `cargo test --workspace` and strict workspace Clippy pass on the integrated
   tree.
2. A live gateway rollout verifies `/ready`, project-scoped search and graph,
   `STORE_BUSY` + retry metadata during active sync, successful and
   `completed_with_errors` sync jobs, cancellation, revision snapshot/restore
   refusal for raw, and a recoverable backup without opening a second DuckDB
   writer.
3. Native visual QA covers Home, Library, Search, History, Wiki, Connections
   and all Operations tabs at default and compact window sizes against the live
   gateway.

**Final release evidence:** pending terminal full-source sync, final installed
gateway rollout and verified final backup. Fill this only with evidence from the
same final tree; do not infer closure from an earlier candidate build.

## Evidence-gated future work

These are the only standing engineering candidates. They become scheduled work
only when their entry condition is observed.

| Candidate | Entry condition | Acceptance measure |
|-----------|-----------------|--------------------|
| Native ANN/VSS | Two representative release runs exceed 300 ms vector/hybrid p95, or breach the configured RSS/throughput threshold | Same labeled eval set does not regress recall/MRR/nDCG and p95 returns below the declared threshold |
| Full Markdown application backend | A real workflow requires Markdown as the active source of truth rather than export/document CRUD | Shared conformance covers document lifecycle, lexical search, wikilink graph, crash recovery and capability refusals; no silent DuckDB fallback |
| Native visual regression harness | A workspace-level layout regression escapes unit tests or a second supported desktop target is added | Deterministic screenshots for Home, Library, Search, Wiki, Connections and Operations at minimum and compact window sizes |

A recorded local exact-search run at 100,111 chunks observed 133.98 ms hybrid
p95, below the 300 ms scale gate. Adding ANN before repeated representative
release runs fail that gate is not roadmap work.

## Non-goals

- Multi-master writes or a second live DuckDB writer.
- A remote vector database as the primary source of truth.
- Mandatory LLM extraction or summarization on ingest.
- Server-side graph layout.
- Tool-count growth for parity with another product.

Deep contracts remain in [`GRAPH_DESIGN.md`](GRAPH_DESIGN.md),
[`STORAGE_ADAPTERS.md`](STORAGE_ADAPTERS.md), and
[`PRODUCT_PRINCIPLES.md`](PRODUCT_PRINCIPLES.md). They do not create roadmap
commitments by themselves.
