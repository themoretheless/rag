# Impact affected-file summary — 2026-10-25

The pinned RAGRAF op_impact at 075caf86dc2f6274b9c48487e521915bcdfeaf2c provides
an affected_files summary. The local result previously exposed affected symbols
only. This extension groups returned symbols by exact document identity and adds
captured source provenance, minimum depth and retained-witness relation counts.
See [contract](../../IMPACT_RELATIONS.md#affected-file-summary).

Two native regressions cover a multi-file imported callback/caller chain with
counts/provenance/depth and a capped lower-bound result, and identical paths in
different projects remaining separate. The focused suite also exercises five
existing impact relation cases. The actual stdio MCP scenario uses imports across
core/callback/wrapper sources and a separate class hierarchy, checks per-file
provenance and bounded counts, and retains project/stale/sync/depth/override
checks. Mock providers and temporary sources/databases isolate the fixture.

No calibrated confidence or stated execution impact is claimed. File/table seed
contracts, ownerless module relations and representative scale remain open. Full
RAGRAF parity remains open. Schema 25 and extractor format 29 are unchanged.
Final gates and source/binary hashes: verification.json. Runtime: runtime.json.

Final gates: **1,235 workspace tests passed, 0 failed, 1 ignored**. All seven
focused impact tests passed (two new). Clippy with `-D warnings`, formatting and
`git diff --check` passed. Rebuilt binary passed the multi-file summary MCP scenario
and existing impact-relation regression. Logs and source/binary SHA-256 hashes
are retained alongside this report. These are controlled fixture results.
