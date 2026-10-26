# Duplicate member comparisons

`duplicates` now returns `similarity_to_primary`, `lines_only_here` and
`lines_only_in_primary` for each returned member, plus `primary_comparison`
provenance and a query-level phase report. The primary compares with itself as
1 / 0 / 0 without scanning lines. This is identity, not measured line counts.

The metric matches the pinned RAGRAF normalized unique-line Jaccard contract:
Python-compatible line breaks, Unicode surrounding whitespace removed, empty
lines discarded, order and repeated lines ignored. Deltas count unique lines.
Similarity rounds to three decimal places; the nested record also gives the
unrounded fraction, intersection and union counts. Empty/empty is 1, empty/nonempty
is 0. `exact_stored_text_equal` independently compares decoded stored text.
No original binary equality, semantic equivalence or deletion safety is implied.

Grouping remains token-trigram similarity within filename families and exact
stored-text groups, with connected-component semantics. Its `min_similarity`
threshold does not filter by this new line metric. Comparisons only cover the
returned group/member windows, after limits and primary selection.

Optional `primary_comparison` options (defaults / maximum):

| Option | Default | Maximum |
| --- | ---: | ---: |
| enabled | true | boolean |
| max_pairs | 1,000 | 100,000 |
| max_total_bytes | 16 MiB | 128 MiB |
| max_file_bytes | 2 MiB | 8 MiB |
| max_lines | 50,000 | 500,000 |
| max_unique_lines | 20,000 | 200,000 |
| max_line_bytes | 64 KiB | 1 MiB |

Numeric limits clamp to at least 1. Files are normalized once per query, sharing
the primary cache across members. Whole decoded file bytes are reserved before
normalization, even if a later line limit fails. A failed file is cached too.
These limits bound this comparison phase, excluding corpus loading, grouping,
role extraction, database IO and total process memory. Inputs are already stored
text; this phase executes no source code or SQL.

Disabled or budget-limited pairs have `status: not_checked`, an explicit reason,
null compatibility metrics, and no partial line counts. Query `complete` only
means all requested comparisons within returned windows were checked; it says
nothing about omitted groups, corpus/index completeness or duplicate quality.
Source document IDs, paths, revisions and hashes anchor results to the same
corpus observation as grouping. Bodies and differing line text are never output.
