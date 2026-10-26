# Impact relation traversal — 2026-10-25

[Contract](../../IMPACT_RELATIONS.md). The previous local implementation traversed
only calls. Pinned RAGRAF 075caf86dc2f6274b9c48487e521915bcdfeaf2c op_impact uses
incoming BFS plus stated/derived and affected-file summaries, and supports table
targets. This milestone transfers broader symbol-dependency traversal while
preserving candidate evidence; remaining file/table/summary contracts are open.
This is not a representative reference accuracy or performance comparison.

Five new native regressions cover bound values feeding into callers without
synthetic calls; inherited method/class dependencies and type users; cycles,
depth/result caps, invalid mode and stable ordering under reordered edge arrays;
project-isolated bindings; and rejection of a cross-project structural edge with
identical paths. The preexisting legacy call-only impact/body-map regression also
runs explicitly before the complete workspace suite.

The actual stdio MCP fixture checks legacy defaults, value-to-caller chains,
relation witnesses, no call fabrication, override candidates, depth/item caps,
invalid mode, project isolation, warm reuse, stale exclusion and refreshed source
sync. Fixtures use mock embeddings and owned temporary sources/databases. No
production database or source application is touched/executed.

Index format 29; database schema 25. Full runtime impact, exhaustive relation
coverage and full RAGRAF parity remain unproven. Final gates and source hashes
are in verification.json; runtime.json contains actual MCP evidence.

Final gates: **1,233 workspace tests passed, 0 failed, 1 ignored**. All five new
impact regressions and the existing call-only impact regression passed. Workspace
Clippy with `-D warnings`, formatting and `git diff --check` passed. The rebuilt
binary passed the new impact MCP scenario and existing method-relation scenario.
Logs and SHA-256 source/binary hashes are retained alongside this report. These
controlled fixtures do not prove exhaustive impact recall or representative scale.
