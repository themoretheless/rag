# PHP namespace and declared-method candidates

PHP/PHTML snapshots capture namespace regions, separate class/function/constant
import tables, flat and grouped imports, aliases, absolute and namespace-relative
names. Symbol qualified names use PHP backslashes and `Class::method`. Namespace
regions do not share imports. Function lookup considers the captured namespace
before a global fallback; class lookup never invents a global fallback. Candidate
lookup is project scoped and ASCII case insensitive for class/function names.

`references`, `callers`, `callees` and `impact` consume these captured candidates.
Literal static calls and `$this`/`self` can target declared methods with a captured
body and compatible static/visibility context. First-class callable syntax produces
a value reference, not a call. Literal object creation produces a class reference,
not a fabricated constructor call. PHPDoc parameter contracts remain attached to
the original declaration and source bytes.

These are syntactic candidates. Composer/autoload/include ordering, actual execution,
compiler acceptance, trait dispatch, late static binding,
dynamic receivers/callables and full PHP typing remain unverified. Conditional
and ambiguous declarations retain explicit uncertainty; unsupported receiver forms
are not resolved by a global short-method-name fallback. Trait/interface contexts
are not treated as concrete runtime owners.

Capture is bounded to 2 MiB source, 100,000 AST nodes, 128 namespace regions,
1,024 imports per region and 10,000 uses. Invalid/duplicate import tables or capture
limits fail closed. Unavailable declaration names use an explicit sentinel instead
of falsely claiming global names. Legacy snapshots missing the PHP capability are
stale and repaired by indexing. Frozen graph diffs expose
`php_name_binding_comparison`: availability identifies captured backend support,
not complete PHP semantics or proof of a behavioral source change.

Verification: [qualification](qualification/php-names-2026-10-26/README.md).
Language rules: [name resolution](https://www.php.net/manual/en/language.namespaces.rules.php),
[imports](https://www.php.net/manual/en/language.namespaces.importing.php),
[first-class callables](https://www.php.net/manual/en/functions.first_class_callable_syntax.php).

Captured single-class inheritance now supplements direct methods: [contract](PHP_INHERITANCE.md).
