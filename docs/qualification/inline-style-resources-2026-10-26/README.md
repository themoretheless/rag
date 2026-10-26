# Inline style resource qualification — 2026-10-26

Continues the style-attribute gap in the RAGRAF matrix. See the
[contract](../../INLINE_STYLE_RESOURCE_RELATIONS.md) and linked W3C declaration-list
specification. CSS boundaries use the already available cssparser dependency;
resource facts reuse the existing AST URL/responsive collectors. This is not a
browser/cascade engine, full CSS validator or complete RAGRAF parity claim.

Six new native cases cover decoded entities/multibyte original source anchors,
no synthetic class/code facts, responsive option evidence, malformed declaration
recovery and rejected nested rules/imports, duplicate/binding/template attributes,
base/element-base guards, explicit byte/depth/property limits, captured SVG impact/
liveness, stale source exclusion, sync/index replacement, immutable descriptor/
property diffs, position/entity-spelling moves and legacy coverage. A supplemental
assertion initially called the fact counter on StyleDocument rather than the
StyleSnapshot API; that compile failure is retained and the assertion corrected.

The real MCP fixture uses temporary source files/databases and mock embeddings.
It exercises inline URL/image-set links, exact original attribute byte spans,
declaration recovery and false-link exclusions, project/base/duplicate guards,
transitive impact, depth/call-mode separation, dead-file protection, immutable
choice diffs, position/entity-spelling stability, stale bridge exclusion and sync
replacement. Existing responsive-image, target-identity, parsed-resource and
SQL-impact scenarios are rerun. Production data and source applications/browser/
network image resources are not executed by these fixtures.

Escaped/dynamic CSS, property/cascade validity, framework/JSX style expressions,
namespace/deployment URI semantics, raw binary/image fidelity and representative
precision/recall/scale remain open. The checkout is shared with another active
user-owned chat; logs identify a checked working-tree snapshot and isolated
executable, not a frozen final checkout. Final gate counts and source/log/binary
hashes are recorded in verification.json.

Final gates: **1,335 workspace tests passed, 0 failed, 1 ignored**; all six new native cases passed. Offline workspace/all-target Clippy with `-D warnings`, owned-file formatting and `git diff --check` passed. The isolated executable passed all **six** MCP scenarios. Its observed runtime uses extraction format **40**, store schema **25**. At report capture the independently edited checkout declared format **41**; after-check source hashes do not claim a frozen whole-checkout build.
