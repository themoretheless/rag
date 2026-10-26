# Shared symbol nodes — 2026-10-26

Implemented the [typed symbol inventory](../../SHARED_SYMBOL_NODES.md) for captured
translation keys, SQL tables and literal URLs, alongside legacy code definitions.
Full RAGRAF parity remains unproven.

**1,466 workspace tests passed, 0 failed, 1 ignored**. Clippy with `-D warnings`,
owned-source formatting, Python syntax and `git diff --check` passed. Five new
native cases cover stable project-scoped key identity/no fake definition path,
independent proof bounds/full counts, source-window filters, schema-only/use-only
tables, handler alternative deduplication, reference roundtrips, default code/name
alias/contains/all modes and stale exclusion. The first focused run retained two
prefix-lookup failures after a patch assertion prevented applying inference;
the patch was corrected, the final focused four passed, and the final workspace
includes the fifth 1..50 proof-limit/numeric-line-order case.

Actual MCP verifies shared identities by ID and prefix-qualified name, project
isolation, no last-writer location, source proof revision/hash anchors, exact-path
counts with stable identity, named-project references roundtrips, quoted/schema-
only/usage-only tables, one URL registration occurrence despite multiple handler
candidates, code-default compatibility, Rust-style namespace prefix guards,
explicit code override, domain counts before result limits, literal contains
without SQL/regex wildcard semantics, body-free deterministic metadata, invalid
selector/mode errors, stored observations despite file changes, stale exclusion
and source-sync replacement.

Six additional MCP scenarios passed on the same isolated executable: SQL/URL
references, translation-key extraction/history, SQL table impact, method relations,
Rust receiver references and Perl indexing/references. All sources/databases are
temporary and embeddings are mock. No ingested code or SQL executes, and no
production database is touched.

Reference revision `075caf86dc2f6274b9c48487e521915bcdfeaf2c` was inspected for its
symbol lookup and shared placeholder/no-last-writer-location contract. The full
reference application was not executed. Explicit local contains mode preserves
literal/case semantics rather than claiming the reference's fallback ranking.

The checked executable reports store schema **25**, extraction format **59**.
This query-only extension needs no extraction-version bump. Concurrent work then
advanced the observed root source to format **60**. `verification.json` records
that distinction, source hashes after checks, executable/log/reference hashes and
runtime reports. This qualifies the checked shared-tree build and isolated
executable, not a frozen checkout or the later entire shared-tree state.

Unassigned-project disambiguation, live catalogue/database/router definitions,
external/unresolved domains, implicit reference ranking, broader CLI/HTTP/UI
exposure, representative precision/recall/scale and full parity remain open.
