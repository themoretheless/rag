# Bounded live-read edit ranges

`live_read.diff.edit_hunks` contract version 1 adds separate source-line changes
to the existing enclosing changed-range summary. The existing fields remain.
Each item has `kind` (insert/delete/replace), `indexed_range` and `live_range`.
Coordinates are one-based half-open; an empty range represents an insertion or
deletion point. Line terminators are significant: CRLF and a missing final LF
are changes. Empty input contains zero lines. No hunk includes source text.
The requested live content still follows its existing line/character budget.

After trimming an identical prefix and suffix, a deterministic line LCS separates
unchanged islands. Equal-score ties consume indexed lines first. The computation
uses at most 1,000,000 LCS cells (u32, at most 4 MB for the matrix); input splitting
also uses memory proportional to source line count. Empty-side edits require no
matrix. Larger mixed regions return one coarse enclosing range, `exact=false`
and `fallback_reason=lcs_cell_budget_exceeded`. Exact means an optimal line edit
alignment for this comparison, not a unique semantic edit or character diff.

At most 64 hunks are returned. `total` counts all discovered hunks and `truncated`
indicates omitted items independently of `exact`. A truncated list cannot fully
reconstruct the new text. The complete, nontruncated range list can reconstruct
it using indexed unchanged ranges plus the corresponding live ranges. Ranges
address the complete decoded sources, independently of the requested body window.
No omitted body text is disclosed by the diff object.

Source allowlists, decoding evidence, byte limits, immutable indexed history,
project selection and drift-event identity retain their existing contracts.
Missing sources retain `diff=null`. Reads do not publish a new index revision;
normal synchronization updates subsequent comparisons. This is a read-contract
extension and requires no database schema or extractor-format migration.
