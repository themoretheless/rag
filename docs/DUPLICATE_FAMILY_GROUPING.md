# Independent exact and numbered-name duplicate groups

`duplicates` accepts `grouping: "exact_and_name_families"` as an explicit audit
view. Its default `similarity_components` remains the existing connected-component
view. The audit view inventories number-suffixed sibling files even when their
text has no similarity, covering the reference RAGRAF `same_name` behavior.

Two independent group sets are built within the selected project and component
path-prefix scope:

* `exact_stored_text`: partitions by stored decoded-text content hash, independent
  of filename or directory. This does not claim original binary equality.
* Numbered-name families: preserve directory, case and extension; strip only a
  trailing optional underscore/hyphen and decimal digits from the filename stem.
  `a/Files2.php`, `a/Files_3.php` and `a/Files.php` share a family. `b/Files.php`,
  `a/files.php`, `a/Files_old.php` and `a/Files_stdin.php` stay separate. This is
  deliberately narrower than the default broad copy/backup filename heuristic.

An exact group and a larger name family can overlap. A name-family group with
exactly the same member set as an exact group is suppressed; proper exact
subgroups remain visible. Groups require at least two documents. No transitive
merge can absorb unrelated filename families through an exact-text bridge.
Exact groups sort first; usage score, primary identity and family key provide
deterministic remaining order. Existing role evidence, freshness guards and
primary retention apply. Bodies are never included.

After group and member limits, the existing bounded
[primary comparison phase](DUPLICATE_PRIMARY_COMPARISON.md) classifies returned
name families using normalized unique-line Jaccard:

* `family`: at least one checked non-primary member meets the threshold.
  This is an existential text-similarity observation, not proof that every member
  is a duplicate or a semantic replacement. `complete` can still be false.
* `same_name`: every member was returned and checked, and all similarities are
  below the threshold. Empty/empty has similarity 1 as documented by the metric.
* `not_checked`: no threshold match is known and missing/disabled/budget-limited
  comparisons prevent a complete negative classification.

The threshold defaults to **0.5** in this view, matching the pinned reference,
versus **0.8** for the existing component view. Both accept explicit
`min_similarity` in 0.5..1. Thresholding uses the unrounded fraction, an explicit
improvement over thresholding the reference's rounded display value. The nested
`family_classification` reports metric, threshold, best checked fraction, counts,
coverage and non-semantic/deletion limitations. The group-level `family_key`
retains identity when a classification is `not_checked`.

`groups_total` counts independent candidate groups before the output limit;
`members_total` counts the full group before its member window. Classification
is only for returned groups/members; global completeness is not implied. The
primary remains selected from the whole group by existing observed static role
ranking, then retained in the output window. `primary_comparison` options still
control all pair/byte/line budgets. A group limit does not change primary choice.
Pairwise token grouping is skipped in this view (`comparisons: 0` and
`pairwise_token_grouping_performed: false`); `max_comparisons` controls only the
default token-grouping phase. Metadata grouping and corpus loading are outside
comparison-phase budgets. This is not a whole-query memory or scale guarantee.

No runtime execution, merge/delete recommendation or original-byte equivalence
is inferred. [Qualification](qualification/duplicate-families-2026-10-26/README.md)
uses temporary data, a pinned pure reference oracle and a real MCP executable.
