# PHP captured inheritance and method lookup

PHP/PHTML `extends` and `implements` occurrences are captured with namespace/import
scope and source identity. `inheritance` exposes candidate base identities and
separate declaration-to-declaration method relations. Method relations retain
`semantic_override_proven: false`: signatures, variance, compiler validity and
runtime behavior are not established by matching declarations.

The call/reference graph now follows a single captured class chain for literal
class calls, `self::`, `$this->` and `parent::`. Name resolution uses the declaring
class's namespace/import table, not the caller's table. `parent::` starts at the
immediate base and bypasses a local override; normal lookup selects the nearest
declared method. All candidates stay within the source project. Inherited public
and protected methods can contribute to callers and transitive impact. Private
base methods do not become inherited targets; scoped instance calls require a
compatible captured instance context. External calls cannot acquire protected or
private access merely by sharing a method name.

Lookup stops at a missing, conditional, duplicate, invalid-kind or final base,
cycles, an abstract target without a body, or the 64-class ancestry limit. Before
using an inherited method, the captured chain must be complete within that bound.
A local declaration can still be reported as a syntax candidate when unrelated
ancestors are unavailable. This is not proof that the whole class compiles.

Traits take precedence over inherited methods. If no local declaration exists,
trait use at an intervening class blocks inherited lookup until trait composition
is modeled. Local declarations retain their precedence. Late `static::`, arbitrary
receivers, runtime subtypes, magic methods, Composer/autoload/load order, dynamic
class names and interface implementation dispatch remain unverified. Implemented
interfaces are recorded as relationships, not treated as concrete method bodies.
Constructors are not fabricated from `new`; constructor and final-parent matches
are excluded from the method-override candidate view.

PHP capture format 2 and index format 73 invalidate old name-only cache payloads.
Unchanged files repair through `index_code` or source sync. Frozen snapshots carry
`php_inheritance_format`; diff reports `php_inheritance_comparison.available` and
excludes PHP inheritance/override relation comparisons when either snapshot lacks
that capability. Candidate-call changes remain explicitly analysis-qualified and
never prove a semantic behavior change.

[Qualification](qualification/php-inheritance-2026-10-26/README.md) uses native
regressions and isolated real MCP fixtures; no fixture PHP/SQL is executed.
Language references: [inheritance](https://www.php.net/manual/en/language.oop5.inheritance.php),
[visibility](https://www.php.net/manual/en/language.oop5.visibility.php),
[trait precedence](https://www.php.net/manual/en/language.oop5.traits.php).

Bounded method-provider composition now supplements the original trait barrier: [trait composition contract](PHP_TRAIT_COMPOSITION.md). Unknown/unsupported composition and consumer-specific trait-body dispatch remain explicit gaps.
