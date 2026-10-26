# Independent duplicate families — 2026-10-26

Implemented [exact/name family audit view](../../DUPLICATE_FAMILY_GROUPING.md),
covering reference groups that the earlier token-similarity view could omit.
No full-parity claim is made.

**1,429 workspace tests passed, 0 failed, 1 ignored**; all 39 selected native tests,
Clippy with `-D warnings`, owned-module formatting, Python syntax and
`git diff --check` passed. Four new native cases cover directory/case/Unicode
numbered-name keys, safe incomplete classification, independent exact partitions
and name families, redundant-member-set suppression, project/prefix guards,
budgets and preservation of the default view.

The new actual MCP scenario compares group member sets against five pure
functions extracted from the pinned reference revision
`075caf86dc2f6274b9c48487e521915bcdfeaf2c`: `to_posix`, `_norm`, `family_key`,
`family_groups`, `exact_groups`. Only those function definitions execute; no
reference application initialization or ingested source runs. The finite fixture
covers exact proper subgroups, dissimilar siblings, directory/case/project scope,
redundant-set suppression, exact-first ordering, disabled/budget unknowns,
member/group caps, body-free deterministic output and sync reclassification.
This validates the tested group contract, not representative duplicate quality.

Five additional actual MCP regressions passed against the same isolated binary:
primary comparisons, duplicate roles, literal path roles, search roles and SQL
table impact. All runtime fixtures use temporary sources/databases and mock
embeddings. Production data and ingested code/SQL are untouched.

The executable reports store schema **25**, extraction format **52**. The latter
advanced in concurrently active shared-tree work; this query/output extension
itself adds no extraction format. Detailed runtime results, source/reference/log
hashes and isolated executable hash are in `verification.json`. After-check source
hashes are observations, not a frozen checkout/build reconstruction guarantee.

Full RAGRAF parity, original binary fidelity, semantic merge/delete safety,
runtime roles and representative quality/scale remain unproven. The default
component view remains available; this audit view requires its explicit grouping
option. Query metadata/corpus work remains outside comparison-phase budgets.
