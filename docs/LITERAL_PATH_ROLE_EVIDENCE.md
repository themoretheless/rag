# Literal path role evidence, contract 1

The pinned RAGRAF literal_refs helper supplements code/markup links with exact
stored-text path mentions, including endpoint constants in JavaScript that never
appear as imports or fetch calls. search_multi and duplicates now have that
separate evidence category.

A literal_text_path_mention_candidate records the exact needle, source document,
path/project/revision/hash, line and decoded UTF-8 byte interval, and the current
target identity. It is an observed substring, not a resolved file dependency:
binding_proven and runtime_usage_proven remain false. It scores 2, below resolved
typed incoming candidates (3), and never proves a primary or deletion safety.
Comments, documentation and data can match. No source is executed.

Targets are the retained search-window files or actual duplicate-group members,
not unrelated singletons. Sources can lie outside the target prefix or search
matches, but must share the project and match fresh published parsed/non-code
source identity. Stale, unindexed, unavailable and parse-error identities do not
create positive text roles. The scan shares the existing corpus/index read
transaction and rechecks the decoded source hash after loading.

Needles include the exact stored path, a path relative to the project's common
stored-source parent, its "./" spelling, and a basename only when unique across
all active stored source-file metadata in that project. A duplicate basename
outside the target prefix still prevents the basename shortcut. Path-component,
identifier, variable and URI-prefix boundaries reject embedded names, unrelated
directory suffixes and ordinary external-URL tail matches. Matching is case
sensitive; this does not infer deployment roots, Windows filesystem identity,
string escape decoding, HTML entity decoding or arbitrary URL normalization.
Duplicate metadata identities at the same path remain explicitly ambiguous.

Existing typed file-import/resource source-target pairs suppress supplemental
literal facts for that pair. This avoids counting an import both as an AST edge
and as the same path string. Within remaining text evidence, a source-target pair
counts once even if the path repeats or matches several needle forms. Counts are
captured mentions, not runtime execution or loader counts.

Both query DTOs expose optional literal_mentions settings: enabled, max_targets,
max_sources, max_bytes and max_source_bytes. Defaults are enabled, 500 targets,
2,000 source reads, 8 MiB decoded text, 2 MiB per source. Upper clamps are 5,000
targets, 100,000 reads, 64 MiB total and 8 MiB per source. Pattern bytes are capped
at 256 KiB and matcher/DFA budgets at 2 MiB. Pattern-budget failure leaves typed
roles available and reports the text phase unqualified.

At most 20 literal facts per target are retained in deterministic source-path/ID
order; complete unique source-target counts still contribute to evidence_total.
The enclosing 100-fact role window remains independent. literal_path_analysis
reports target/source work, skipped/stale identities, basename ambiguity,
suppressed typed-pair mentions, total/retained facts, budgets and truncation.
Complete means the eligible bounded scan completed, not universal path semantics
or a proof of absence. Disabled, budget-exhausted and unqualified cases never
report absence_proven.

These are text-phase limits, not total query IO/RSS/time limits: metadata
inventory/byte-length inspection and index freshness validation are excluded.
Representative ranking quality, query-wide response budgets, performance/scale,
dynamic bindings and full RAGRAF parity remain open. The extraction format and
store schema do not change.

Qualification: [literal-path-roles-2026-10-26](qualification/literal-path-roles-2026-10-26/README.md).
