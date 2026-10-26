# Stated PHPDoc parameter contracts

PHP/PHTML `symbol` results and `code.symbol` now carry `parameter_contracts`
(format 1) on named functions and methods. The existing AST-adjacent docblock
association is used; a previous declaration's docblock is never selected merely
because it precedes the function. Source parse errors make capture unavailable.

Line-leading `@param`, `@phpstan-param` and `@psalm-param` tags retain their full
bounded type expression and hash, parameter name, reference/variadic markers,
actual AST parameter-name match and original UTF-8 source spans. Duplicate tags
remain separate. No dialect precedence or inherited contract is inferred.
Missing names, unmatched declarations, duplicates, inherited documentation and
capture limits produce explicit partial/ambiguous or unavailable states.
`undocumented_parameters` lists AST parameter names absent from captured tags.
`complete` describes bounded evidence capture, not type validity, runtime
correctness or whether every parameter is documented.

Direct `array{}`, `list{}`, `object{}`, `non-empty-array{}` and
`non-empty-list{}` shapes expose ordered fields, required/optional keys, nested
shapes, tuple indices and final unsealed `...` rest expressions. Balanced
strings, callable signatures and generic delimiters prevent accidental comma
splitting. Quoted literal keys preserve commas and `*`; only comment-prefix
stars are removed. Constant keys retain an unresolved expression. Duplicate
literal keys, malformed delimiters, unsupported escapes and exhausted shape
budgets never publish a partial shape. A union/intersection or `[]` around a
shape stays an opaque `type_only` expression, so its fields are not falsely
reported as unconditional. Alias expansion, constant evaluation, PHP key
coercion, compiler validation and full PHPStan/Psalm dialect rules are open.

Requiredness applies to keys when the argument value is supplied. It does not
say the function parameter itself must be passed. PHP and static analyzers are
not executed. The shape vocabulary follows [PHPStan's documented forms](https://phpstan.org/writing-php-code/phpdoc-types#array-shapes);
retained type strings are stated documentation, not checked types.

Structured parsing independently limits one docblock to 64 KiB, 64 accepted
tags, 4096 bytes per type, 256 declared parameters, delimiter depth 32, nested
shape depth 16, 128 fields per shape and 512 total fields per docblock. A short
4096-character display excerpt does not truncate the independent parser's
input. Overlarge input has unknown tag count (`null`), never fabricated zero.
These limits exclude pre-existing AST parsing and full raw-document hashing.
Source spans refer to the stored decoded UTF-8 source and include multiline
comment prefixes where the selected span crosses lines.

Current cached PHP callables missing the contract capability are reparsed.
Snapshots retain the optional contract payload and legacy payloads deserialize
without it; dedicated semantic contract-diff reporting is not implemented.
See [qualification](qualification/php-parameter-contracts-2026-10-26/README.md).
