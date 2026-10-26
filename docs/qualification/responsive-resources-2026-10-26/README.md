# Responsive image resource qualification — 2026-10-26

Continues the srcset/image-set gap in the RAGRAF matrix. See the
[contract](../../RESPONSIVE_RESOURCE_RELATIONS.md) and its linked primary HTML/
CSS specifications. This is a structural file-resource extension, not a browser
selection engine or full CSS validity checker. Full RAGRAF parity remains open.

Seven new native regressions cover URL commas/data URLs and descriptor validation,
img/source/preload attributes, entity-decoding anchors, duplicate/template cases,
string/URL/MIME separation, literal/math resolutions, conditional/nested/rejected
option suppression, bounded/full-hash descriptors, inline CSS offsets, captured
SVG target impact/liveness, immutable descriptor diffs, position-only moves and
legacy classification guards. A preliminary compile identified a missing tree
lifetime annotation; it was corrected before these tests.

The real MCP corpus uses temporary files/databases and mock embeddings. It
exercises picture/preload srcset, CSS string/URL options, choice evidence,
invalid/data URL/MIME/base/project exclusions, call-mode separation, dead-file
protection, descriptor diffs, position-only moves, immutable history and stale
resource/sync replacement. Existing target-identity, parsed-resource, module-
import and SQL-impact scenarios are also rerun. No production database or source
applications/browser/network image resources are executed by these fixtures.

Runtime choice, raw binary/image decoding, full CSS option/type/property semantics,
JSX/dynamic/generated bindings and representative precision/recall/scale remain
unqualified. The checkout is shared with another active user-owned chat. Evidence
identifies a checked working-tree snapshot and an isolated executable, not a
frozen final checkout. Final gates and source/log/binary hashes are recorded in
verification.json.

Final gates: **1,328 workspace tests passed, 0 failed, 1 ignored**. All seven new native cases passed; workspace/all-target Clippy with `-D warnings`, owned-file formatting and `git diff --check` passed. The isolated executable passed all **six** MCP scenarios. Actual runtime uses extraction format **38**, store schema **25**. The feature began at format 37; concurrent configured-import work advanced the shared-tree extraction version. Runtime versions and the executable hash identify the qualified artifact.
