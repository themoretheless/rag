# PowerShell index qualification — 2026-10-26

See the [contract](../../POWERSHELL_CODE_ANALYSIS.md). The pinned RAGRAF profile
was inspected. The selected tree-sitter-pwsh 0.38.1 grammar covers using directives
missing from the older cached crate; parser/source mapping is verified locally,
not promoted to runspace or language conformance.

Thirteen native regressions cover ASCII case-insensitive names, nested captured
scope, literal top-level PSScriptRoot dot sourcing, project isolation, explicit
constructors, static/this/single-base methods, using-module class scope without
invented command exports, overload ambiguity, alias/scoped/conditional/dynamic
guards, deferred script-block occurrences, enum/property/body spans, imported
name collisions, function-local loads, body-comment metadata exclusion, literal using-module quote adaptation with byte/CRLF/comment/interpolation
boundaries, source refresh, full-resolution equivalence, frozen history and legacy coverage guards.

The real MCP corpus uses temporary roots/databases and mock embeddings. It checks
actual symbols, command case, literal loads, class inheritance, static and instance
calls, dynamic/alias guards, exact Unicode byte bodies, metadata separation,
transitive symbol/file impact, partial slice, entrypoint/status capability,
cache equivalence, stale exclusion, source sync and frozen base/load diffs.
Existing Perl, C#, Ruby, Lua, SQL-impact and file-import scenarios are regression
gates. No source script, module/manifest or interpreter is executed and no
production database is touched.

The checkout is concurrently edited by another user-owned chat. Logs and the
isolated executable identify the checked build. After-check source hashes are
observations, not a frozen whole-checkout build attestation. Final gate results
and hashes are recorded in verification.json. Complete language/runtime, scale,
representative semantic quality and whole-product RAGRAF parity remain open.

The first format-47 MCP attempt exposed a grammar error on double-quoted literal
using-module paths. A raw parser probe reproduces that error. Format 48 adds a
bounded header-only quote adapter, retaining original source bytes and addresses.
The original MCP fixture is retained unchanged for the final rerun. Initial logs
are retained separately from final gates.

Final gates: **1,397 workspace tests passed, 0 failed,
1 ignored**. All thirteen PowerShell regressions passed. Clippy
with warnings denied, owned-file formatting and whitespace checks passed. The
isolated format-48 executable passed all seven actual MCP scenarios; the new
scenario kept its original double-quoted using-module fixture. Store schema 25
is unchanged. Initial parser/runtime logs are reproduced failures preceding the
adapter, not final failing gates.
