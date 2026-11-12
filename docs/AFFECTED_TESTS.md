# Related test recommendations

`affected_tests` finds static paths from test entrypoints to selected changed
symbols. It returns evidence and suggested argument vectors. It does not execute
commands or authorize skipping tests. `coverage_complete`,
`runtime_coverage_measured`, `safe_to_skip_unlisted_tests` and `absence_proven`
remain false even when a bounded traversal finishes.

```json
{
  "symbols": ["api"],
  "project": "my-project",
  "depth": 10,
  "limit": 100,
  "max_work": 50000,
  "relations": "references"
}
```

The stored-source query uses one read transaction and a project-scoped fresh
input view. No live filesystem freshness is checked; edits must be ingested and
indexed first. Stale/unindexed/parse-error bridges are excluded and reported.
Every selector must identify exactly one symbol, optionally restricted by `path`.
Ambiguous or unavailable selectors return explicit status and no guessed seed
union. Selectors are limited to 1..1000; candidate samples cap at 20 each.

Rust queries use a separate transient `cfg(test)` overlay. Explicit captured
build configurations gain the `test` flag while retaining their feature/target
selection. Without an explicit configuration, only `test`/`not(test)` predicates
are decided; other cfg predicates stay uncertain. Pure test-only external modules
become traversable, known production-only declarations/imports are excluded, and
unknown imports retain a shadow barrier instead of supplying a guessed target.
Raw extraction caches and normal symbol-query views are unchanged by the query.
IDs from a current normal configured view map to corresponding test-view IDs;
an obsolete manifest/configuration ID is not silently accepted as current.

Format 39 refreshes test extraction and binding: Rust attribute paths are matched
exactly (`test`, `tokio::test`, `async_std::test`, including leading `::`), comments
between attributes are allowed, and doc strings/prefix lookalikes do not create
tests. Direct `ignore` attributes are captured. Inline-module spans normalize
`self`/`super` calls and imports relative to their true module, preserving the
original source expression in reference evidence. Cargo target facts retain raw
CLI names separately from Rust crate identifiers, including hyphens, and respect
`test=false` and `harness=false` for command suggestions.

Relations default to calls plus bound value/type references, including callbacks.
`calls` excludes value references; `all` also includes inheritance/override
candidates. Ambiguous edge candidates are retained as possible relationships,
with their original candidate lists and resolution metadata. Project identity
is checked at both endpoints. Breadth-first traversal keeps separate captured lexical trait states per symbol
and one shortest discovered witness per test; steps retain relation type, source revision/hash/
location and definition metadata. A directly changed test has a zero-edge
`changed_test` witness. This is structural dependence, not execution coverage.

Depth is 1..20 (default 10), test records 1..500 (default 100), and a shared
candidate/traversal work budget is 1..200000 (default 50000). Explicit flags expose
work exhaustion, depth and result truncation. Snapshot loading, parsing and
Cargo module/command lookup are separate work; limits are not an end-to-end
deadline or physical memory bound. Diagnostic/context samples cap at 50, command
samples at eight per test and full-suite fallback manifests at 20, with counts.

Rust suggestions use `cargo test --manifest-path ...` with the captured target
(`--lib`, `--bin`, `--test`, or explicitly test-enabled bench/example), captured
feature/default-feature/target options and a substring test-name filter. They do
not guess an exact harness path. Directly ignored tests append `-- --ignored`.
Custom harness targets receive an unfiltered target command; their runner/filter
protocol is unknown. All commands require configuration review and matching
captured source state. The query does not verify that Cargo, dependencies,
toolchains or a successful build are available on the caller's machine.

Python test-name candidates may receive `python -m pytest <file> -k <name>`;
collection/import/runtime validity remains unverified. Existing Go and Java test
entrypoint candidates can participate in paths; runner commands for them are not
invented. Unknown/custom/generated test macros, dynamic dispatch, doctests,
unsupported frameworks, Cargo multi-unit/feature-unification semantics and
unresolved relationships can leave gaps. Explicit one-target configurations can
exclude other test targets. Full-suite fallback commands are investigation
candidates and must be adapted to all relevant features, platforms and ignored
test policies. An empty result is not proof that no test is affected.

For Git inputs, pass `include_tests=true` to `detect_changes`. `max_tests` and
`test_max_work` bound independent recommendations under each side's
`before_impact.affected_tests` / `after_impact.affected_tests`. The test overlay is
used for both graphs; `depth` also controls the test traversal when supplied
(otherwise the test default is 10). Changed-group/seed limits can omit tests and
remain explicit in the surrounding impact report. Commands for historical
sources must be run only against that matching source state; no checkout occurs.

Qualification: [native, real MCP and actual suggested-command execution](qualification/affected-tests-2026-10-26/README.md).

PHPUnit declaration and [effective inherited/trait candidates](PHPUNIT_EFFECTIVE_TESTS.md) from the separate
[entrypoint catalog](PHPUNIT_ENTRYPOINTS.md) now participate in recommendations.
The common incoming walker tracks `(symbol, captured lexical consumer)` states;
paths are reconstructed from those exact states, preserving intermediate class
frames instead of stitching independently chosen symbol witnesses. Alternative
consumers can reach the same trait symbol before their own tests are found.
Class/trait frame definitions are returned as `path.trait_context_symbols`, and
steps carry `source_context`/`target_context`; `complete_to_seed` requires a
contiguous path ending in a selected root. Runtime context/coverage proof remains
false. A changed test keeps a zero-step witness.

Intermediate functions do not consume the returned-test limit. The query applies
that limit after deterministic discovery, reports total tests found in the
examined graph and retains independent depth/work/state/metadata diagnostics.
Graph construction, traversal, context rows, candidate membership checks and the
depth-bound probe share `max_work`; paths support this query's full depth 20.
Missing required entry context refuses the branch and marks metadata incomplete.
`traversal_truncated` and `traversal_metadata_incomplete` report walker limits;
`trait_context_traversal` retains detailed counters. These are examined-graph
limits, never a claim of complete program discovery.

PHPUnit package loading, runner configuration/version, exhaustive inherited/trait
test discovery, data providers and runtime coverage remain open. No PHPUnit argv
is guessed: `command_status=no_qualified_command_candidate` explains an empty
command list. Existing Rust/Python candidates retain their prior semantics.
[Qualification](qualification/affected-php-contexts-2026-10-26/README.md).

Effective PHPUnit recommendations retain host/effective-method identity rather
than deduplicating every invocation by body ID. See the effective-test contract
for lexical-owner matching, shared bodies, alias identity and frozen history.

[PHPUnit host dependencies](PHPUNIT_HOST_DEPENDENCIES.md) additionally recommend
captured invocations for changed classes, bases and traits, including alias-only
changes with unchanged method bodies. They carry explicit non-call associations
and can be disabled with `include_host_dependencies=false`. Source-identity joins
for directly selected hosts are independent of graph traversal budget; indirect
host matching shares the work/depth limits. These witnesses are separate from
the ordinary calls/references/all selector.
