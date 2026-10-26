# HTML/CSS file resource qualification — 2026-10-25

Continues the file-resource gap in the RAGRAF acceptance matrix. The pinned
reference `075caf86dc2f6274b9c48487e521915bcdfeaf2c` extracts markup file references
in `extract/css_html.py`. This implementation adds AST-based, source-bound,
project-isolated resource candidates with explicit unresolved URL/base cases,
file-impact traversal, dead-file protection and immutable relation diffs.
See the [contract](../../FILE_RESOURCE_RELATIONS.md).

Six native regressions cover HTML attributes/entities, CSS quoted/unquoted
imports and declaration URLs, transitive graphs without symbols, whole-document
base URL guards including earlier attributes/inline CSS, full-source offsets,
comments/duplicate attributes/conditions/namespace exclusions, URL encodings and
directory guards, literal quote preservation, form actions, project/ambiguity/
parse-error cases, stale exclusion, refresh and frozen/legacy diffs. The first
fixture used modern import supports syntax outside the current grammar; its
failure is retained and that limitation is documented explicitly. A subsequent
check encountered the other chat's unfinished git_impact module; the live file
was preserved and the check retried after that work advanced.

Real MCP qualification uses a temporary database, source fixtures and mock
embeddings. It checks chained page → stylesheet → imported stylesheet links,
script dependencies, entity/query handling, source provenance, base/project
exclusions, separate call mode, depth limits, dead-file protection and sync
replacement. Existing module-import and SQL table-impact scenarios are retained
as regressions. Source applications/SQL, networks and production data are not
executed by the resource analyzer or qualification fixtures.

Nonindexable/binary assets, browser deployment mapping, dynamic/template URLs,
preprocessors, inline attributes and representative precision/recall/scale are
not qualified. These are static candidates; deletion safety and runtime effects
are never claimed. Full RAGRAF transfer-and-improvement remains incomplete.

The checkout is shared with another active user-owned chat. Logs and the isolated
binary identify a checked working-tree snapshot, not a frozen final checkout.
Final gates, actual runtime versions and source/binary/log hashes are recorded
in verification.json.

Final gates: **1,304 workspace tests passed, 0 failed, 1 ignored**; all six new native resource cases passed in that run. Workspace/all-target Clippy with `-D warnings`, owned-file formatting and `git diff --check` passed. The isolated executable passed all four MCP scenarios. Actual runtime uses extraction format **35** and store schema **25**. The workspace run preceded a behavior-preserving if/else Clippy fix in the concurrently developed Git capture helper; final Clippy and the MCP executable include it.
