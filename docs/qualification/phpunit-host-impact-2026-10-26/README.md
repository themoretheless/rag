# PHPUnit changed-host recommendations — 2026-10-26

[Contract](../../PHPUNIT_HOST_DEPENDENCIES.md).

before.json identifies the previous qualified executable. before.log reproduces
its empty recommendation set for an indexed class with effective trait tests.
The new implementation retains real method-body and host identities and adds a
separately labelled availability association; no call occurrence is fabricated.

Three native regressions cover changed-host isolation, aliases, source-labelled
non-call evidence, explicit disabling, direct-host joins under exhausted graph
budget, shared inherited non-trait bodies, base and nested trait chains,
association-aware depth, independent output limits, shared work accounting,
changed-body precedence and missing relation candidates. Existing related-test,
PHP and Git native cases are retained.

Real MCP checks exercise class/base/trait selectors, opt-out and relation modes,
source/host/edge provenance, nested depth limits, result/work caps, direct-host
budget independence, warm/full-resolution consistency, alias-only Git before/after
recommendations, unchanged Git staging/index state, stale-host exclusion, sync
replacement and project isolation. The existing PHP affected-context validator
now distinguishes explicit availability evidence from actual call references.

MCP fixtures use temporary DuckDB stores, mock embeddings and disabled LLMs.
PHP, PHPUnit and ingested SQL are never executed; Rust command fixtures execute
only their own temporary package. Production stores are untouched. Runtime and
safe-to-skip claims remain false. Framework configuration/bootstrap, interface
contracts, datasets, runtime dispatch, complete/minimal impact and representative
quality/scale remain open. Full RAGRAF parity and universal superiority are not
established. The executable is copied and hash-checked; source is a shared working
tree, with selected hashes observed rather than a frozen checkout claim.

See final logs and verification.json for gate counts and exact identities.

Final gates: **1616 workspace tests passed, 0 failed, 1 ignored**. All 15
related-test, 47 PHP and 11 Git-impact native cases passed. Workspace/all-target
Clippy with `-D warnings`, binary build, formatting, Python syntax checks and
`git diff --check` passed. Eight real stdio MCP suites passed on one copied
executable: host impact, effective PHPUnit, PHP affected contexts, existing
affected tests, SQL impact, trait impact paths, trace contexts and Git contexts.
The new host scenario passed 17 checks, including alias-only before/after Git
recommendations with unchanged staging and persistent index revision. Executable
hash and selected observed source hashes stayed unchanged during MCP runs.
Runtime index format **81**, store schema **25**, PHP capture **6**; this is a
query-only extension over existing facts.
