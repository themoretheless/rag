# Source documentation candidates and measured coverage

`symbol` and `code.symbol` now expose optional `documentation` plus
`documentation_examined`. Syntax index format **27** refreshes older caches at
normal explicit indexing/source sync; database schema stays 25. Old immutable
revisions remain unchanged and their absent metadata is unmeasured by default.

Supported forms:

- Python function/class first literal expression: ordinary/raw/unicode string,
  parenthesised literal and static concatenation. Bytes/f-strings and expressions
  after another statement are not literal docstrings.
- Rust outer `///` / `/**` comments before captured declarations, including
  intervening ordinary comments and attributes. Inner `//!` / `/*!` do not become
  function docs. Explicit `#[doc ...]` styles are marked uninspected, not missing.
- Leading `/**...*/` syntax on JS/JSX/MJS/CJS/TS/TSX, Java and PHP/PHTML declarations.
  Export wrappers and single arrow-variable declarations are handled. Multi-variable
  declarations and known file/virtual-symbol tags are uninspected rather than
  guessed as function documentation.

This is **syntax attachment**, not compiler/rustdoc/JSDoc expansion, rendering or
runtime __doc__ evaluation. Other languages, embedded markup scripts without
captured definitions, generated docs, attributes/macros, richer attachment/tag
conventions and compiler/dialect equivalence remain unverified. Existing symbol
coverage limits still apply. The marker rules do not prove complete documentation
presence/absence in a language.

`documentation` includes kind, raw source text excerpt, raw character count,
content_hash and source segments. Text is **raw source segments joined with
newlines**: markers, quotes and escapes remain; literal concatenation is not
runtime string decoding. The hash is BLAKE3 over that complete assembled raw text.
Each segment records decoded UTF-8 half-open byte offsets and one-based source
positions; the enclosing symbol supplies document/path/revision/source hash.
Independent text and segment-list flags distinguish truncation:

- maximum excerpt: 4096 Unicode characters (never a partial UTF-8 character);
- maximum returned segments: 64;
- raw_chars/total_segments and the complete raw-text hash remain measured even
  when an excerpt/list is truncated.

These are per-fact output bounds, not a full response/indexing/I/O bound. Absence
of a documentation object with examined=true means no candidate under the supported
rules; examined=false means the declaration/style was not qualified. Parse-error
files never count as inspected, even if partial marker evidence was observed.

`index_status.quality.docblocks` reports only aggregate metrics over fresh scoped
symbols: examined/documented/uninspected counts, observed facts, truncation counts
and a ratio for the **examined supported syntax forms**. Empty/unavailable
measurements have null coverage. Unsupported/uninspected symbols are excluded
from the denominator. Status returns no doc text or doc hashes. Compiler/rendered
semantics and full-language coverage flags stay false; full_docblock_coverage
remains in unmeasured_checks. This is an observation, not a quality threshold or
a claim that undocumented code is invalid. Project selection scopes these counts.

Raw facts persist in derived/frozen symbol metadata and reuse with the cache.
Changed ingested source makes them stale and omits them with the other facts;
source sync/index_code refreshes them. No source applications, doc generators,
macros, includes, URLs or new filesystem reads are invoked.

PHP/PHTML callable symbols additionally expose [stated parameter contracts](PHP_PARAMETER_CONTRACTS.md), parsed independently of the display excerpt with explicit declaration matching and bounded shape evidence.
