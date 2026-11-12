# PHP captured trait method composition

PHP/PHTML class, enum and trait declarations now retain bounded `use` groups and
AST-bound `insteadof`/`as` adaptations. Namespace/class import aliases resolve trait
names within the same project. Nested traits reuse the declaring trait's import
table. A class declaration wins over a trait method, and a composed trait method
wins over an inherited base method. An empty known trait allows base lookup to
continue; an unknown provider cannot silently disappear from composition.

`insteadof` removes specified providers for a method; it never chooses a provider
by file order. A qualified alias can retain a method excluded from the original
name. An unqualified alias uses the selected original after exclusions. `as` adds
a callable name without removing the original. Visibility-only adaptations apply
to the selected original; visibility on an alias does not change other names.
Conflicts and invalid/ambiguous source selection have explicit unresolved reasons.

Resolved calls and callable-value references point to the original trait method
symbol. No synthetic class method or source location is invented. Access checks
use the consuming class as the effective owner, including private aliases and
protected members. A private method composed into a base is not inherited by a
child. Static/instance and late-static guards still apply. Enum use contributes
method candidates with the same explicit compiler/runtime limitations.

`inheritance` separately reports `php_trait_use_candidate` relationships between
captured class/trait/enum and trait identities. These feed `impact_relations=all`;
method calls feed normal callers and impact. Definition source bytes, revision and
hash remain those of the original trait. Separate inherited override declarations
retain their conservative exclusions when traits are involved.

Limits: 64 use groups per owner, 64 trait names per group, 256 adaptation clauses
per group, 32 nested composition levels and 4,096 charged work steps per method
lookup. These supplement the PHP source/AST/use budgets. Cycles, missing,
conditional, duplicate, wrong-kind and conflicting providers fail closed. Limit
states do not fall back to unrelated base methods. Direct local declarations can
still be syntax candidates without proving the whole class valid.

PHP capture format 3 and index format 74 repair older payloads. Frozen snapshots
carry `php_traits_format`; diff exposes `php_traits_comparison.available` and
suppresses trait-use relation comparisons against legacy snapshots lacking the
inventory. Source changes to selection rules and trait files refresh through sync;
forced-full resolution can check cache equivalence. Candidate diffs do not prove
runtime behavioral changes.

## Remaining qualification

This is captured method composition, not PHP compilation. Signature/variance,
property/constant compatibility, final/abstract requirements across an entire
composition and actual loading/execution are unverified. Captured trait-body
`self`, `$this` and `parent` calls now retain separate consumer evidence:
[consumer contexts](PHP_TRAIT_CONTEXTS.md). Full path correlation remains open.
Runtime subtypes, late
`static::`, magic dispatch and dynamic receivers remain open. A direct static
call on a trait name is not treated as an ordinary concrete-class call.

The locked tree-sitter PHP grammar does not accept every modern adaptation form.
Parse-error inputs are excluded, including unsupported multi-name exclusion
forms. Final adaptations are rejected explicitly because the grammar can classify
`as final` as an alias. Multiple separate supported `insteadof` clauses are covered. This is not complete PHP-version grammar coverage or universal trait
semantics. PHP CLI is unavailable in the qualification environment.

[Qualification](qualification/php-traits-2026-10-26/README.md).
PHP language reference: [traits](https://www.php.net/manual/en/language.oop5.traits.php).
