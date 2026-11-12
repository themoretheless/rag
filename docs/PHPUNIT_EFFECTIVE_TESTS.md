# Effective PHPUnit test candidates

The PHPUnit catalog now represents a test as an existing source method plus its
captured host class and effective method name. A method supplied by a base class
or trait can therefore yield several candidates without inventing source spans,
method declarations or runtime executions. `test_context` retains the host
symbol/document/path/revision/hash, lexical class owner, effective name and
`declared`, `trait`, `inherited_declared` or `inherited_trait` origin. Top-level
entrypoint provenance continues to describe the actual method body. `entrypoints` accepts an explicit `project` parameter that scopes stored-source
loading and discovery; omitting it retains the global catalog. Project filtering
is applied before discovery, including ancestor/trait lookups. Entrypoint
filters can match either the body symbol or host class, including a host path
whose inherited body is in another file.

Captured TestCase ancestry, namespace imports and Test attributes retain their
previous syntax guards. Concrete hosts are considered; abstract bases can supply
a method. Effective lookup uses class declarations, composed traits and then
captured bases, preserving overrides, aliases, visibility adaptations and
`insteadof` choices. Private/static/bodyless/conditional candidates, ambiguous
identities, recursive or incomplete trait composition and unrelated roots are
excluded. Attribute names resolve in the declaration's own namespace/import
scope, rather than the consuming host's scope.

Discovery is bounded: 128 class hosts per document, 1024 captured names per host,
64 ancestry steps, 32 trait nesting steps and 65536 composition/name work units
per host. The query reports these limits and keeps
`phpunit_candidate_enumeration_complete=false` and
`phpunit_runtime_discovery_verified=false`. Full compiler validity, framework
installation/version/configuration and exhaustive discovery are not proven.

`affected_tests` distinguishes `(body, host, effective method)` candidates. It
matches trait-body walk states against the candidate's lexical owner and retains
multiple states for a shared body. Context-independent dependencies can recommend
all applicable hosts, while an A-only lexical dependency cannot recommend a B
consumer. Direct changes to a shared test body recommend its captured invocations
with zero-edge witnesses. Test limits apply after discovery; candidate matching
also consumes the shared work budget, with `invocation_match_work` diagnostics.
Direct changed tests remain available independently of graph traversal budget.
`test_host` exposes the real class definition; `symbol` remains the body method.
No PHPUnit command, runtime instance, subtype dispatch or permission to skip
unlisted tests is inferred.

Frozen graphs use `phpunit_tests_format=2`. Host and lexical-owner IDs normalize
to stable definition identities in entrypoint relations, alongside effective
name/origin. Position/hash-only shifts do not create new invocation identities.
Comparison is separately unavailable against marker 1 or missing legacy data;
capability enrichment is not a source-semantic change. Capture format 6 already
contains the required facts; this derived-query change needs no extraction bump.

Limits still include data providers, TestWith datasets, legacy annotations,
Pest, runtime `$this` subtype dispatch, Composer/bootstrap/runner configuration,
full PHPUnit-version/compiler qualification and representative quality/scale.
The contract describes captured candidates, not complete runtime test execution.

[Qualification](qualification/phpunit-effective-2026-10-26/README.md).

[Changed-host recommendations](PHPUNIT_HOST_DEPENDENCIES.md) connect this inventory
to class/base/trait changes with source-labelled availability witnesses, preserving
host identity even when bodies are shared or unchanged. Call-path and runtime
semantics remain separate.
