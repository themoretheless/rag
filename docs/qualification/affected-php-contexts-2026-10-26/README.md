# Captured PHP trait frames in affected test recommendations — 2026-10-26

[Contract](../../AFFECTED_TESTS.md), [PHPUnit discovery](../../PHPUNIT_ENTRYPOINTS.md).

The previous qualified executable omitted PHPUnit entries from affected_tests.
`before.json` and `before.log` retain the real MCP reproduction against its
verified hash: an indexed direct PHPUnit test reaches the changed method but
recommendations return no observed test entrypoints. This is evidence for missing
PHP recommendations, not an observed false-positive context bug in that binary.

The new query includes direct PHPUnit declaration candidates and uses the common
incoming traversal with separate `(symbol, lexical consumer)` states. Native
regressions cover incompatible class consumers, alternate states through a shared
trait, context-contiguous shortest witnesses, direct changed tests, value/call
modes, missing entry metadata, independent depth/output/work limits, a complete
20-step witness and stale-source exclusion. Intermediate routine states do not
consume the output-test limit. Context candidate-membership scans now count
against the same traversal budget, including in ordinary impact/Git paths.

The real MCP scenario checks imports/attribute candidates, separate and shared
trait consumers, exact source evidence and class-frame definitions, calls versus
references/all modes, changed tests, bounded traversal, warm/full-resolution
consistency, stale consumer hosts, sync replacement and project isolation.
Existing affected-test, SQL impact, PHP trait impact, trace and Git context suites
are rerun against the same copied executable.

PHPUnit runner/package/version/configuration, inherited or trait-provided test
discovery, data providers, dynamic execution and runtime coverage remain open.
No PHPUnit argv is guessed without captured runner facts. An empty command list
has an explicit status. Unlisted tests are never declared safe to skip.
These are captured lexical frames, not proof of runtime object identity or PHP
compiler validity. No PHP, PHPUnit or ingested SQL is executed. MCP fixtures use
temporary DuckDB stores, mock embeddings and disabled LLM calls; existing Rust
command fixtures execute only their own temporary test package. Production stores
are untouched. Full RAGRAF parity and general superiority remain open.

See logs and verification.json for final native/workspace/Clippy/build/MCP gates,
binary identity and selected source observations. The copied executable is fixed;
the source checkout is shared with another active task and is not frozen.

Final gates: **1607 workspace tests passed, 0 failed, 1 ignored**. All 10
related-test, 43 PHP and 10 impact native cases passed. Workspace/all-target
Clippy with `-D warnings`, binary build, formatting, Python syntax checks and
`git diff --check` passed. All six isolated stdio MCP suites passed on one copied
binary; its hash stayed unchanged. Selected observed source hashes also stayed
unchanged during the MCP run. The new scenario passed 16 checks, including
before/after Git recommendations without using the persistent index. Runtime
index format **81**, store schema **25**, PHP capture format **6**; this is a
query/traversal change and does not require a new extraction format.
