# PHP parameter contracts — 2026-10-26

This milestone extends the existing raw AST-adjacent docblocks with bounded,
source-backed stated parameter contracts. [Contract](../../PHP_PARAMETER_CONTRACTS.md).
Pinned RAGRAF source: 075caf86dc2f6274b9c48487e521915bcdfeaf2c.
Its pure `_parse_array_shapes` function is extracted via Python AST without
importing or starting the reference application. The simple required/optional
array fixture matches. Additional local fixtures independently exercise nested
shapes, quoted commas/literal stars, generics/callable types, duplicate dialect
tags and exact parameter binding; these are improvements, not proof of universal
dialect equivalence.

Native regressions exercise source spans, declaration mismatch, methods and
promoted parameters, variadics, malformed and duplicate keys, opaque union
suffixes, full docblocks beyond display excerpts, empty/adjacent comments,
cache capability guards and independent tag/type/doc/field/depth limits.
Real MCP uses only temporary source files and databases with mock embeddings.
It checks `symbol`/`code` agreement, unchanged caches, ingest-induced stale
exclusion and changed requiredness after `sync_sources`.

SQL table impact and existing documentation/literal/shared-symbol/route
regressions are rerun against the same copied executable. No ingested PHP,
JavaScript or SQL is executed; no production database is opened.

Evidence belongs to a build from the shared dirty checkout, not an isolated
frozen source checkout. Another user-owned chat edits Rust analysis concurrently.
The copied executable is held fixed during MCP qualification; post-check source
hashes are observational, not an assertion of an atomic source snapshot.
See verification.json, runtime.json and retained logs for final gates/hashes.

Compiler/dialect validation, inherited contracts, alias/constant expansion,
PHP key coercion, dedicated semantic contract diffs and representative quality/
scale remain open. These fixtures do not establish full RAGRAF parity.

Final native gates: all **9 new PHP contract regressions passed**. Workspace
Clippy (`--all-targets -- -D warnings`) and the MCP binary build passed. An earlier
workspace run passed **1,491 tests, 0 failed, 1 ignored**. After concurrent Rust
cfg edits, the final workspace attempt stopped in the root library with
**1,242 passed, 1 failed, 1 ignored**:
`code_index::rust_cfg::tests::configured_fields_select_all_raw_alternatives_and_the_active_owner`
(assertion on `binding_limitations.is_empty()`). That work is active in the other
user-owned chat; this milestone does not claim a green final workspace or repair
those in-progress changes. Both complete logs are retained. The final focused
PHP tests and final build/Clippy ran after the PHP metadata correction.

All **8 real MCP runners passed** against the same fixed executable: PHP parameter
contracts, SQL table impact, literal target impact, raw documentation, shared
symbol nodes, translation keys, non-code references and PHP routes. The SQL
regressions include read/write filters, indirect callers, ownerless SQL,
project/schema disambiguation, schema-only candidates before source/access
filters, stale exclusion and source-sync replacement. Runtime index format 62,
store schema 25. Exact executable hash and scenario inventory: binary.json.
