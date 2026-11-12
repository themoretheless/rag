# PHPUnit declaration candidates

`entrypoints` returns `phpunit_test_candidate` for effective public instance
methods named `test*` or carrying the captured
`PHPUnit\Framework\Attributes\Test` attribute, in concrete classes whose captured
ancestry reaches the name `PHPUnit\Framework\TestCase`. Namespace imports and
aliases are resolved through the existing PHP scope facts. Intermediate captured
base classes are project scoped; cycles, ambiguous identities, conditional or
incomplete definitions, abstract host classes and static/private methods are excluded.
Each candidate retains exact definition/document/revision/hash provenance.

These are declaration candidates. Composer autoload, installed PHPUnit identity,
XML configuration, runner/version eligibility, data providers, legacy annotations
and Pest are unverified. Captured inherited and trait-provided candidates are
covered separately by [effective test discovery](PHPUNIT_EFFECTIVE_TESTS.md). PHP and PHPUnit are
never executed by discovery. `test_discovery_complete` and
`phpunit_runtime_discovery_verified` remain false. `affected_tests` now includes these PHPUnit candidates and preserves
coherent captured trait consumer frames in recommendation witnesses. The query
keeps separate states per symbol/context, including shared traits reached through
multiple consumers; it does not infer a PHPUnit command without runner facts.
See [recommendation qualification](qualification/affected-php-contexts-2026-10-26/README.md).

PHP capture format 6 repairs older cached facts, including missing abstract and
attribute metadata. Frozen graphs now carry `phpunit_tests_format=2` for host/effective-method
identity; entrypoint diff
only compares this kind when both revisions support it, with separate
`phpunit_test_comparison` availability and false semantic/runtime proof flags.
Legacy capability enrichment is not reported as added or removed tests.

The syntactic PHPUnit rules were checked against the official
[writing tests](https://docs.phpunit.de/en/13.4/writing-tests-for-phpunit.html) and
[attributes](https://docs.phpunit.de/en/13.4/attributes.html) documentation.

[Qualification](qualification/sql-impact-phpunit-2026-10-26/README.md).
