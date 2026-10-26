# Captured non-code resource identities — 2026-10-25

Continues the unparsed-resource gap in the RAGRAF matrix. See the
[target contract](../../RESOURCE_TARGET_IDENTITIES.md). The implementation
preserves code/parser coverage semantics while extending HTML/CSS file-resource
links to published source attachments without symbol extractors. It validates
current stored source identity against both graph inventory and frozen file
metadata, with explicit unavailable states and negative-liveness guards.

Five native target regressions cover JSON/SVG incoming/transitive links without
code symbols, preserved source provenance, source attachment-only changes with
unchanged content/revision, stale and archived targets, corrupted frozen file
identity, missing original source blobs, project isolation, immutable diff and
legacy comparison guards. A sixth native ingestion case proves SVG discovery
preserves XML/script text without rendering/execution. The initial corruption
fixture left its intentionally broken revision-file hash in place before a new
publication; its failure is retained. The fixture restores that hash before its
independent missing-source test. No integrity guard was relaxed.

Actual MCP fixtures exercise source sync, SVG ingestion, JSON resource links,
transitive impact, project/parser separation, stored-versus-disk identity,
stale exclusion/status diagnostics, incomplete negative liveness, sync refresh,
link removal/dead-file candidates and immutable historical comparisons. Existing
parsed-resource, module-import and SQL-impact scenarios are rerun. All fixtures
use temporary sources/databases and mock embeddings; production data and source
applications/SQL are not executed.

No raw binary asset fidelity, unregistered assets, browser/deployment behavior,
representative precision/recall/scale or end-to-end RAGRAF parity is claimed.
Source hashing/metadata IO is outside the literal dead-files scan budget.
The checkout remains shared with another active user-owned chat. Logs describe
the checked working-tree snapshot and isolated executable, not a frozen final
checkout. Final gate results and source/log/binary hashes are in verification.json.

Final gates: **1,316 workspace tests passed, 0 failed, 1 ignored**. Five new target cases and the SVG ingestion case passed. Clippy for workspace/all targets with `-D warnings`, owned-file formatting and `git diff --check` passed. The isolated executable passed all **five** MCP scenarios. Actual runtime: extraction format **36**, store schema **25**.
