# PHPUnit host availability in affected tests

Changing a class's trait composition or aliases can change its effective tests
while leaving every source method body untouched. `affected_tests` now joins
captured PHPUnit invocations to their host classes. PHP `extends` and trait-use
candidates additionally connect changed bases and nested traits to those hosts.
The query uses only fresh, same-project definitions and the existing qualified
candidate inventory; it never synthesizes executable methods.

`include_host_dependencies` defaults to true. The association is separate from
`relations=calls|references|all`, which still controls ordinary code dependencies.
Set `include_host_dependencies=false` for code dependency witnesses only. Rust,
Python and other test candidates keep their existing code-path semantics.

Availability witnesses start with a `phpunit_host_candidate` step from the
actual method body to the actual host class. Its evidence retains both source
identities, effective name and captured lexical owner; `call_edge=false` and
`runtime_registration_verified=false` are explicit. Subsequent steps retain
captured inheritance/trait-use evidence plus source revision/hash provenance.
`reason=captured_test_host_dependency` identifies this witness. The result still
has a contiguous `path.symbols`/`path.steps` path ending at the selected seed.
Shared bodies on different hosts remain distinct, including inherited non-trait
methods whose lexical source alone cannot distinguish the invocation.

The incoming host walk reserves one depth step for the association, shares the
graph/traversal work budget, uses project/freshness guards and the common bounded
state walker, and reports `host_dependency_traversal`. Matching indirect host
invocations also spends work (`host_invocation_match_work`). Direct selected-host
identity joins remain available independently of traversal budget, like directly
changed test bodies; `direct_test_host_matches` reports them. Output test limits
apply after discovery. Ordinary complete code witnesses win equal-length ties;
a shorter complete availability witness can replace a longer code path.
Directly changed method bodies retain their zero-step `changed_test` witnesses.

`detect_changes(include_tests=true)` uses the same behavior on each captured
before/after source graph. Alias-only class edits can therefore recommend the
old and new effective test names even when the trait method body is unchanged.
Historical commands still require their matching captured source state. Git and
persistent index state remain read-only during investigation.

These are static candidate-availability dependencies. Interface contracts,
framework bootstrapping/configuration, PHPUnit runtime discovery, datasets,
subtype dispatch and complete/minimal impact sets remain unverified. Other
changed source/binding seeds can independently recommend the same shared body.
Coverage/runtime/absence/safe-to-skip flags remain false. No PHPUnit command or
call edge is inferred from class membership. No extraction or frozen-history
format changes are required; these witnesses are query results over existing
facts, not new persisted source relations.

[Qualification](qualification/phpunit-host-impact-2026-10-26/README.md).
