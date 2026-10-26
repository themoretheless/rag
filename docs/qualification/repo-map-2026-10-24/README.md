# Repository-map subset qualification — 2026-10-24

The implementation adds exact project and component-aware directory scope,
project-isolated graph/file identities, scoped coverage, and an optional estimated
token budget to the compact ranked map. Existing graph ranking includes callback
references from files without definitions, uses deterministic ordering, and
selects files in rounds rather than exhausting one file first.

Evidence:

- Five module tests cover central API ordering, round selection, stale-reference
  exclusion, Unicode character accounting, oversized-entry skipping, identical
  paths in separate projects, unaffected scoped coverage after foreign-project
  mutations, directory boundaries and tiny/normal estimated token budgets.
  A real temporary DuckDB corruption case verifies that a project-scoped snapshot
  does not decode another project's broken cached payload; project parameters
  are bound rather than interpolated.
- [Real stdio MCP](runtime.json) exercises the compiled binary and public input
  schema with isolated mock embeddings and a temporary database. It checks actual
  PageRank ordering, determinism, project/prefix filtering, both budgets, empty
  results, and scoped stale/unindexed coverage after real ingest operations.
- [Pinned reference contracts](reference-contracts.json) execute only RAGRAF's
  `op_repo_map` and payload-weight helper from revision
  `075caf86dc2f6274b9c48487e521915bcdfeaf2c`, with owned in-memory SQLite rows and
  the reference's unavailable-centrality fallback. The first record consumes 924
  characters despite a 400-character budget; `/src/lib` also includes
  `/src/lib-copy`. Local tests prove strict first-record handling and component
  boundaries. These are controlled contract comparisons, not a representative
  ranking, information-density, latency or memory benchmark.

Reproduce:

```sh
cargo test --lib code_index::repo_map
cargo build --bin rag-mcp
python3 scripts/smoke_repo_map.py target/debug/rag-mcp /tmp/repo-map-runtime.json
python3 scripts/compare_repo_map_contracts.py /path/to/pinned/RAGRAF /tmp/repo-map-reference.json
```

Budgets cover map text, not response metadata. Token counts remain estimates of
the target model. Full language/relationship coverage, complete response token
bounds, additional CLI/resources/prompts and representative repository quality
and scale evidence remain open. This report does not establish full RAGRAF parity
or whole-product superiority.

Final checks: `cargo test --workspace --locked --offline` passed 1167 tests
with one ignored; workspace Clippy passed with `-D warnings`, formatting and the
rebuilt binary's MCP smoke passed. [Source/binary hashes](verification.json) pin the
qualified working-tree implementation against the listed base revision.
