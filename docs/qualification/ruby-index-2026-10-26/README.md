# Ruby syntax index qualification — 2026-10-26

Transfers the Ruby grammar/profile gap from the pinned RAGRAF reference, extending
it with distinct instance/singleton ownership and conservative local/constructor/
namespace guards. See [contract](../../RUBY_CODE_ANALYSIS.md). Full Ruby semantic
coverage, all reference languages and whole-product RAGRAF parity remain open.

Ten native cases exercise class/module/ordinary/singleton/eigenclass/endless
syntax; implicit/self/constant/local constructor receivers; source argument/body
separation; parameter/block shadows; tuple/loop/rescue/pattern locals; constructor hooks and instance-state assignments; reassignment/conditional typing; nested and
absolute constant lookup; literal require-relative scopes; inherited instance/
singleton candidates and project isolation; reopening/mutation/constant/constructor
guards; bare values versus parenthesized calls; dynamic loaders/unknown receivers/
parse errors; exact body navigation; source refresh; frozen/legacy history; and
full binding recomputation versus cache equivalence.

The actual MCP corpus uses isolated temporary sources/databases and mock
embeddings. It checks Ruby capability, body spans, owners/constructors/inherited
calls, project/mutation/parameter guards, partial slices, file imports/impact,
cache equivalence, stale exclusion, source-sync replacement and frozen diffs.
Existing Lua, module-import, SQL-impact and resource scenarios are rerun. No Ruby
VM, source application, source SQL or production database is executed/touched.

Adding the cached grammar caused Cargo's offline resolver to move three unrelated
platform package pins; their prior versions were restored with precise updates
(redox_syscall 0.9.1, schannel 0.1.30, wayland-backend 0.3.16). Source/clippy checks
use the resulting lockfile. No platform/runtime support claim follows from that.

The shared checkout is concurrently edited by another user-owned chat. Logs and
the isolated executable identify the checked snapshot; after-check source hashes
are observations, not a frozen whole-checkout build attestation. Final gate counts
and source/log/binary hashes are in verification.json. No representative semantic
precision/recall, performance/scale, VM/framework or cross-platform qualification
is claimed by these fixtures.

Final gates: **1,349 workspace tests passed, 0 failed, 1 ignored**.
Clippy with warnings denied, owned-file formatting and diff whitespace checks
passed. The isolated format-44 executable passed all five actual MCP scenarios.
Ten Ruby native regressions include local-binding classification and implicit
constructor-hook protection. SQL table impact passed again at this format.
