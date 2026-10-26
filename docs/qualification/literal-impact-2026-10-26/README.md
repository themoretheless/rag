# Literal key/URL impact and source-error guard — 2026-10-26

Implemented [literal target impact](../../LITERAL_TARGET_IMPACT.md) through the
existing native/MCP impact surface, with shared symbol `impact_target` navigation.
Full parity and runtime translator/router impact remain unproven.

**1,478 workspace tests passed, 0 failed, 1 ignored**. Clippy with `-D warnings`,
owned-source formatting, Python syntax and `git diff --check` passed. Eight new
native cases cover direct owners/ownerless sources, all-root seeding under
limits, multi-root file dependencies, URL source owners versus handler alternatives,
identity guards, stale bridges, schema-only SQL identity, invalid/mixed arguments,
a reproduced parse-error caller bridge and a 3,001-byte captured URL roundtrip.
The final workspace includes all eight; the earlier focused run contains seven.

`parse-bridge-initial.log` preserves the actual failure: the old symbol walker
reported broken.js at depth 1 and outer.js at depth 2, despite broken.js being a
parse-error document. The common walker now removes invalid source roots and
bridges for all relation modes; file traversal already enforced that boundary.
`initial-workspace.log` preserves an old expectation that a source path could
hide table ambiguity; it now verifies identity disambiguation before source
filtering, followed by a qualified zero-use result. `initial-runtime.log` retains
a runner-only stale symbol-name map after source byte positions changed; it was
fixed by refreshing the map after publication.

The final actual MCP scenario proves key direct-owner/file facts, separate code
and file dependencies, independent direct/result/depth windows, all seed roots,
route registration/client separation, handler alternatives not becoming seeds,
project ambiguity before source/kind filters, schema-only SQL ambiguity, error-
source bridge exclusion, shared symbol navigation, long URL selector alignment,
source revision/hash/byte/line anchors, deterministic results, invalid inputs,
stale resource/direct-owner exclusion and real sync replacement.

Six additional real MCP regressions passed on that same isolated executable:
SQL table impact, symbol-to-file summaries, single-file impact, relation modes,
shared symbol nodes and SQL/URL references. All fixtures/databases are temporary
and embeddings mocked; no ingested code or SQL executes and production data is
untouched. The reference revision `075caf86dc2f6274b9c48487e521915bcdfeaf2c` was
inspected for its generic incoming impact BFS and key/route facts; the full
reference application was not executed.

The executable reports store schema **25**, extraction format **60**. These query/
traversal changes require no extraction-format bump. `verification.json` records
runtime/source format observations separately, source/reference/log/executable
hashes and detailed outcomes. The concurrently active dirty checkout was not
frozen: this qualifies the checked build and isolated executable, not any later
entire shared-tree state.

Runtime impact, unsupported ownership/import forms, unassigned-project selection,
full selector/domain compatibility, representative quality/scale and whole-query
memory/work budgets remain open. A bounded complete traversal never proves
complete program impact or absence of other uses.
