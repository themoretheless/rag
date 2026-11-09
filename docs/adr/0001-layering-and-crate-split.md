# ADR 0001: Layering inside the root crate, then a two-crate split

- Status: accepted
- Date: 2026-11-09
- Scope: `rag-mcp` server (root crate). Clients are covered by PLAN.md Stages 6–7.

## Context

The root crate is 149k lines in one compilation unit with no enforced internal
boundaries (numbers in [BASELINE.md](../refactor/BASELINE.md), evidence in
[INVENTORY.md](../refactor/INVENTORY.md)):

- `Store` is a god object: 258 methods in 46 `impl Store` blocks, 19 of those
  blocks outside `src/db`, reaching the DuckDB connection through a
  `pub(crate)` `Store::lock`.
- Module cycles: `db ↔ code_index`, `db ↔ graph`, `db → external_db`,
  `code_index ↔ external_db`, `config →` llm/rerank/maintain/db, `models`/`error
  → duckdb`, `http_api → mcp`.
- There is no use-case layer. MCP tools and HTTP handlers build the same
  commands separately and have drifted in limits, defaults, envelopes and error
  codes.
- A clean build of the workspace takes minutes and every test binary links
  bundled DuckDB, so any edit pays for the whole crate.

An earlier document (`docs/REFACTOR_PLAN.md`) states that several of these
items are done; the code at `facf533` shows they are not.

## Decision

1. Adopt six layers with a one-direction dependency rule:
   `domain ← store ← intel ← app ← edge ← bins`
   (definitions in [ARCHITECTURE.md](../refactor/ARCHITECTURE.md)).
2. Fix the boundaries **inside the root crate first** (Stages 2–4), by moves
   and visibility changes, with behaviour pinned by tests. An architecture test
   keeps the rule once it holds.
3. Introduce an `app` layer of use cases; `mcp` and `http_api` become adapters.
   Where the two transports disagree today, one rule per use case is chosen and
   every changed limit/default/error code is listed in PLAN.md.
4. One composition root (`main.rs` / `bootstrap`) builds config, store,
   embedder, hooks, use cases, edges and background tasks.
5. Only at the end (Stage 8) split out two crates: `rag-store-duckdb` (store)
   and `rag-code-intel` (intel). Nothing else is split.

## Alternatives considered

- **Split into crates first.** Rejected: the cycles make it impossible without
  the same boundary work, and doing both at once mixes moves with logic edits.
  A crate boundary is also expensive to move if it turns out wrong.
- **One crate per layer (six or more crates).** Rejected for now: `domain`,
  `app` and `edge` change together during Stages 2–4; more crates would add
  manifest and re-export churn without a build-time gain that is measured.
  Revisit after Stage 8 with numbers.
- **Keep one crate and rely on convention.** Rejected: the present state is the
  result of convention without enforcement.
- **Keep `Store` as the single facade and only slim it.** Rejected: 40 methods
  outside `db` need raw SQL; without repository types and a private `lock`
  the leak reappears.
- **Trait-based storage abstraction (`Storage` trait, Markdown backend).**
  Exists in `src/storage` and is not used by production paths apart from
  `open_configured` and capability names. Rejected as the seam: it abstracts
  the wrong level (whole backend) and is scheduled for deletion in Stage 3.

## Consequences

- Positive: `store` and `intel` can be compiled and tested apart from the
  transports; a change in a tool handler no longer rebuilds DuckDB glue.
  MCP and HTTP answer the same way by construction. `rag-mcp-ui
  --features direct-db` stops depending on the whole server.
- Positive: `Store::lock` becomes private to `store`, so SQL has one home.
- Cost: Stage 2 is a large mechanical move (the ~40 `impl Store` methods
  outside `db` become repository types); it is done module by module with the
  full gates on each commit.
- Cost: a hook (`DocumentWriteHook`) replaces a direct call, which adds one
  indirection on the write path. Measured in Stage 5.
- Contract changes are limited to the three approved ones: a DB schema
  migration (Stage 5), aligning MCP and HTTP behaviour (Stage 4), deleting dead
  code (Stage 3). Anything else stops the work for a decision.
- The public re-exports in `src/lib.rs` stay stable until Stage 8, where they
  become re-exports of the new crates.
