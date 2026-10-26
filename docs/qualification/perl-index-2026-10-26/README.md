# Perl index qualification — 2026-10-26

See the [contract](../../PERL_CODE_ANALYSIS.md). The pinned RAGRAF Perl profile
and its scalar/array/hash declaration/use extension were inspected. The actual
maintained grammar `ts-parser-perl 2.0.0` was used; the incompatible older cached
crate was not introduced. Reference execution, full interpreter semantics and
whole-product parity are not claimed.

Ten native regressions cover package/block restoration, explicit calls, forward
versus implemented and lexical subs, duplicate ambiguity, parent/package file
candidates, project isolation, sigil normalization, lexical shadowing, initializer
visibility, reference dereferences, parameters/anonymous subs, modern class/method
inventory, function values, coderef guards, imports/eval/mutation guards, nested
receiver anchors, metadata/body separation, source refresh, cache equivalence,
frozen history and legacy absence markers.

The actual MCP corpus uses temporary source roots/databases and mock embeddings.
It checks symbols, package and inherited calls, project isolation, normalized
variable references, exact Unicode body spans, body-free metadata, package-file
impact independent of filename, ordinary static SQL owner/table impact, partial
slices, status capability, forced full-resolution equivalence, stale exclusion,
source synchronization and frozen parent removal. Existing C#, Ruby, Lua, SQL
impact and module-import scenarios are regression gates. No interpreter,
source application, module import or source SQL is executed; production DBs are
not touched.

A one-character `str::replace` argument-type error in the concurrently developed
infrastructure module initially blocked compilation and was corrected without
changing its intended replacement behavior. The shared checkout remains active
in another user-owned chat. Logs and an isolated executable identify the checked
build; after-check source hashes are observations, not a frozen checkout build
attestation. Final gate results and hashes are in verification.json.

Final gates: **1,382 workspace tests passed, 0 failed,
1 ignored**. All ten Perl regressions passed in that workspace
run. Clippy with warnings denied, owned-file formatting and whitespace checks
passed. The isolated format-46 executable passed all six MCP scenarios. Store
schema remains 25. The retained initial-coderef-regression log records the
reproduced false variable-as-callee link; the workspace log proves its corrected
regression passes. It is not a final failing gate.
