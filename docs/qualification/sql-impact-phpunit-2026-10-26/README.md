# SQL impact and PHPUnit entrypoint qualification — 2026-10-26

Contracts: [SQL table impact](../../IMPACT_RELATIONS.md#sql-table-targets),
[PHPUnit declarations](../../PHPUNIT_ENTRYPOINTS.md).

SQL checks cover direct readers/writers, ownerless SQL, indirect callers,
read/write separation, project/schema identity, evidence/result limits, warm
reuse, stale exclusion and source-sync replacement. The real MCP scenario adds
schema-only identities: a known DDL table with no usage is observed without
invented readers/writers, and schema ambiguity is resolved before access/path
filters. Captured PHP trait tests separately check SQL write impact without
crossing incompatible class consumers. This is static captured-source impact;
executed queries, physical database binding, ORM/dynamic SQL and representative
precision/recall/scale remain unverified.

PHPUnit native checks cover TestCase/base/attribute imports, abstract intermediates,
public instance eligibility, rejection of loose names/unrelated classes,
private/static methods, cyclic/cross-project bases and old capture facts.
Frozen history comparison suppresses this kind when either revision lacks its
separate capability. Real MCP checks cover the declaration catalog, exact source
identity, warm reuse, stale-source exclusion and removal after source sync.
Inherited/trait-provided PHPUnit tests, runner/version/configuration, data
providers, framework execution and context-sensitive PHP affected_tests remain
open. No test-skipping or complete-discovery claim is made.

All runtime fixtures use temporary DuckDB stores and mock embeddings with LLMs
disabled. They execute neither PHP applications, PHPUnit nor ingested SQL; no
production database is touched. Runtime uses a copied executable whose hash is
checked before/after the suites. Sources are observed in a shared checkout with
another active task; this is not a frozen-source or deployed-gateway proof.

Final gates: 10 impact and 43 PHP native tests passed (including three new
PHPUnit regressions). Workspace: **1603 passed, 0 failed, 1 ignored**.
Workspace/all-target Clippy with `-D warnings`, binary build and all six real
stdio MCP suites passed: SQL table impact, PHP trait impact paths, PHPUnit
entrypoints, existing affected-test recommendations, trait-aware trace paths and
Git trait impact. Runtime index format **81**, store schema **25**, PHP capture
format **6**. The copied executable hash stayed unchanged; selected observed
source hashes stayed unchanged during the MCP run. The affected-tests regression
also executed its suggested Rust fixture commands; PHP/SQL remained unexecuted.
See logs and `verification.json` for exact binary identity and source observations.
Full RAGRAF parity and overall superiority remain open.
