# Effective PHPUnit candidates — 2026-10-26

[Contract](../../PHPUNIT_EFFECTIVE_TESTS.md).

The previous qualified binary is identified by hash in before.json. The new real
MCP case reproduces its missing class-scoped inherited/trait catalog in before.log.
This is not a measured runtime execution failure in PHPUnit.

Native cases cover abstract-base methods on concrete hosts, declared overrides,
trait aliases/visibility/conflicts/cycles, unrelated roots, independent body and
host source identities, lexical-owner frames, multiple states for a shared body,
changed-body invocations, stale host metadata and normalized marker-2 frozen
history with marker-1 comparison guards. The earlier direct-only expectation was
updated to include an abstract base's concrete inherited method on its eligible
child; initial.log retains that intermediate expected-change failure.

Real MCP checks cover host/path-scoped catalog pages, source/host provenance,
trait alias invocations, consumer isolation, common-body alternatives, inherited
methods, override precedence, changed trait bodies, limits/work accounting,
warm/full-resolution equivalence, stale-host exclusion, source-sync alias changes
and normalized immutable diff. Existing PHPUnit/affected-test/SQL-impact/trait
impact/trace/Git scenarios run against the same copied executable.

No PHP, PHPUnit or ingested SQL is executed. MCP uses temporary DuckDB stores,
mock embeddings and disabled LLMs; Rust command fixtures run only their own temp
package. Production data is untouched. Sources are a shared working tree, not a
frozen checkout; selected hashes and copied-executable identity are observed.
Compiler validity, framework/version/runner/configuration, runtime dispatch,
data-provider execution, exhaustive discovery and representative quality/scale
remain open. Full RAGRAF parity and universal superiority remain unproven.

The first runtime pass caught a missing API scope: the former entrypoints schema
had no `project` parameter, so the stale-alpha assertion still saw a fresh beta
candidate in the global catalog. `phpunit_effective-initial.log` and
`verification-initial.json` preserve that failure. The endpoint now exposes an
explicit project parameter and loads a project-scoped snapshot; a native Store
regression covers fresh alpha, stale alpha and independent beta catalogs.

See final logs and verification.json for exact gate counts and identities.

Final gates after the API scope fix: **1613 workspace tests passed, 0 failed,
1 ignored**. All 47 PHP, 12 affected-test and 10 impact native checks passed.
Workspace/all-target Clippy with `-D warnings`, binary build, formatting, Python
syntax checks and `git diff --check` passed. Eight isolated real stdio MCP suites
passed against the same copied executable: effective PHPUnit, PHPUnit catalog,
PHP affected contexts, existing affected tests, SQL impact, trait impact paths,
trait-aware trace and Git trait paths. The new effective-test scenario passed
16 checks. Runtime index format **81**, store schema **25**, PHP capture **6**,
frozen PHPUnit invocation marker **2**. The executable hash stayed unchanged;
selected source hashes also stayed unchanged during the final MCP run. Earlier
runtime failure and intermediate gates remain explicitly retained separately.
