# Translation-key syntax and references

`references(key="reg.months", project="web")` joins literal template and PHP
translator-call candidates using the exact decoded key and project identity.
It returns `uses_key` records alongside the existing symbol/table/URL target
modes. A key target cannot be combined with another target or lexical mode.
Key names are case-sensitive, nonempty, at most 120 Unicode characters (480 UTF-8 bytes), without controls.
An observed key is not a proved catalogue declaration or runtime translation.

## Captured syntax

PHP/PHTML/HTML/HTM files capture `%{Key}`, `@{Key}` and `{{Key}}` with the pinned
RAGRAF ASCII key alphabet: letters, digits, underscore, dot and hyphen. The raw
scan preserves each byte-position occurrence, including repeated mentions on one
line. Comments, strings and inactive templates can contain these candidates;
template-engine selection/evaluation is not inferred. Other formats are not
silently labelled scanned.

PHP/PHTML AST capture also recognizes direct or namespace-qualified function
calls whose final name matches translate, trans, t, __, tr, gettext, _t or lang
(case-insensitively). Only a positional first argument containing one quoted
string is supported. A local function with such a name remains a syntax candidate;
there is no claim that its binding is a translation implementation. Method calls,
variable callees, concatenation, spreads, named arguments, interpolation,
heredoc/nowdoc and non-UTF-8/control-valued keys are not turned into invented keys.

Single- and double-quoted literals use their distinct static escape rules,
including literal dollars, backslashes/quotes, byte hex/octal and Unicode escapes.
These rules were checked against the [PHP string manual](https://www.php.net/manual/en/language.types.string.php).
The decoded key is independent of source spelling. `key_encoding` identifies
placeholder/single-quoted/double-quoted evidence; offsets address the original
encoded content (or the full placeholder), not a fabricated decoded byte span.
For PHP calls the captured lexical owning symbol is preserved when qualified;
raw template candidates remain file origins.

## Provenance, scope and work limits

Every result has document/path/project/revision/hash, UTF-8 start/end byte offsets,
line and one-based UTF-8 byte column. Offsets address the captured decoded UTF-8
source anchored by its content hash, not pre-decoding bytes of a legacy-encoded
file. `callee_binding_proven`,
`template_evaluation_proven` and `translation_definition_proven` remain false.
No source bodies or translation values are included. Exact same keys observed
in different projects require disambiguation before source-path filtering.
Matching counts and `uses_key` totals precede the 1..500 output limit.

Capture limits per supported document: 2 MiB source text and 4,096 retained
occurrences, with the 120-character / 480-byte decoded key limit. Placeholders are captured first,
then PHP AST calls. Input/occurrence limits have explicit incomplete coverage,
reason and retained counts. Dynamic/unsupported calls and oversized keys have
separate excluded counters; completeness means only the documented syntax subset,
never complete runtime usage. Existing AST parsing, corpus loading, source-sync
and total query memory are outside these capture-phase limits.

Cached documents must carry the per-document capture capability in addition to
the current global extraction format. Missing capture metadata is stale, not
verified empty. Source sync/index refresh repairs it. Stale source facts are
excluded from queries, with normal scoped diagnostics.

## Immutable history

Frozen revisions carry the translation-key capture and a separate capability
marker. Diff compares document/project, stable owner identity, key, syntax,
usage kind and occurrence count. Position-only shifts and equivalent literal
spelling do not create key changes. Counts preserve repeat additions/removals.

Legacy revisions missing the capability report unavailable comparison; no fake
addition/removal is produced. For documents present on both sides, captures must
be complete on both sides. Limited/unavailable captures are excluded explicitly,
with counts and bounded document IDs. Qualified added/deleted files can contribute
new/removed relations. Full diff output limits and project scopes still apply.
This freezes syntax facts, not catalogue resolution or runtime behavior.

[Qualification](qualification/translation-keys-2026-10-26/README.md) records native,
pinned-reference and actual MCP evidence. Translation catalogues/locale fallback,
framework binding, additional formats/languages, named/dynamic PHP forms,
broader CLI/HTTP surfaces and representative
precision/recall/scale remain open. Full RAGRAF parity remains unproven.

Shared key identities can now be discovered through the
[typed symbol inventory](SHARED_SYMBOL_NODES.md), with stable project-scoped IDs
and source proofs instead of a fabricated definition location.
