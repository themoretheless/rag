# C# syntax index qualification — 2026-10-26

See the [contract](../../CSHARP_CODE_ANALYSIS.md). The reference is pinned RAGRAF
075caf86dc2f6274b9c48487e521915bcdfeaf2c, `extract/generic.py` C# profile.
This transfers declarations and source occurrences while extending the profile
with conservatively qualified namespace/owner/type/arity/base candidates.

Native regressions cover declaration kinds, scoped imports/aliases/global usings,
Unicode source spans, constructors, typed receivers, single-base candidates,
project isolation, overload ambiguity, shadowing and writes, optional/generic/
dynamic/conditional cases, namespace precedence, duplicate partial types, nested
receiver occurrence anchors, body-free metadata, unsupported bindings and captured
local-function shadows. Store-level checks exercise source refresh, stale
exclusion, cache equivalence, frozen history and legacy coverage markers.

The actual MCP scenario uses temporary source roots/databases and mock embeddings.
It checks symbols, exact bodies, namespace aliases, constructors, typed/inherited
calls, guards, partial slice, transitive impact, status capability, forced full
resolution equivalence, stale exclusion, source sync and frozen base removal.
Existing Ruby, Lua, SQL table-impact and file-import scenarios are regression gates.
No source application/compiler/SQL is executed and no production DB is touched.

The shared checkout is concurrently edited by another user-owned chat. The
isolated executable and logs identify the checked build. Source hashes captured
after checks are observations, not a frozen whole-checkout build attestation.
Final results and hashes are recorded in verification.json. Whole-language,
representative semantic quality/scale, compiler/runtime and full RAGRAF parity
remain unqualified.

Final gates: **1,367 workspace tests passed, 0 failed,
1 ignored**. All thirteen C# regressions passed in that workspace
run. Clippy with warnings denied, owned-file formatting and whitespace checks
passed. The isolated format-45 executable passed C#, Ruby, Lua, SQL table impact
and file-import MCP scenarios. Store schema remains 25.
