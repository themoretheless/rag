# Go syntax and package graph — 2026-10-24

This slice extends the source index with tree-sitter-go 0.25.0. The pinned RAGRAF
reference (`075caf86dc2f6274b9c48487e521915bcdfeaf2c`,
`extract/generic.py`, Go profile) extracts functions, methods, type declarations
and calls; its profile explicitly does not resolve parameter/return types.
The local extension adds project/package/module binding, typed receiver methods,
interface-contract targets and reference evidence for the qualified cases below.
This is source-level comparison, not an executed full-reference benchmark or
proof of general superiority.

Functions and methods retain distinct qualified identities across files. Package
binding includes only the same directory/package/project and preserves source
IDs even when other projects have identical paths. Default import names come
from package declarations, while explicit aliases remain file-local. Captured
`go.mod` roots resolve own-module imports and stop at nested modules. No ambient
filesystem reads, dependency downloads, Go commands or source execution occur.

Nine unit fixtures cover same-package calls, pointers and typed receivers,
interface declaration targets, own-module/default/aliased imports, export guards,
external dependencies left unresolved, local shadowing including types/constants,
composite literals, pointer-only method-expression exclusion, aliases/cycles,
test/build isolation, nested modules, identical paths across projects, anonymous
ownership diagnostics and bounded module-declaration parsing. The module reader
rejects block comments/duplicate/missing declarations and accepts line comments,
quoted and raw module paths. It does not validate the entire module graph.

The actual stdio MCP fixture uses an isolated mock-embedding store. It verifies
source-sync discovery of `go.mod`, symbols/method bodies, references and call
edges, actual package-name imports, parameter shadowing, test/build isolation,
interface-contract provenance, entrypoints and warm parse reuse. Direct ingest of
a changed `go.mod` makes it stale and removes dependent import targets before
reindex. New/unindexed and stale nested manifests preserve a blocking boundary;
parent-module fallback is forbidden until the boundary is explicitly removed.
Sync publishes a new revision and recomputes bindings while reusing Go
parses. Restoring the module path restores the original target IDs. The MCP fixture
also verifies boundary preservation before indexing and deletion-only boundary
removal with source synchronization.

Evidence is in `runtime.json`, `verification.json` and `source-sha256.json`.
Reproduce with:

```sh
cargo build --bin rag-mcp
cargo test --lib
python3 scripts/smoke_code_go.py target/debug/rag-mcp /tmp/go-index.json
```

Syntax-cache format 14 invalidates older derived payloads. Captured source and
immutable history retain module changes. Local schema v24 and external capture
format 4 remain unchanged. Existing user stores and servers were not opened or
restarted, and these results do not establish deployment.

Remaining Go requirements include generic instantiation, anonymous function
ownership, embedded member promotion, arbitrary callback/return inference,
pointer alias/expression coverage, local-type methods, compiler-selected build
contexts, cgo, dot imports, dependency versions/replacements, vendoring/workspaces,
import initialization order, full compiler validation and performance at scale.
Interface targets identify a static contract, not its runtime implementation.
Other RAGRAF languages and the full feature/quality matrix remain open.

Semantic rules checked against primary references:
[Go specification: method sets](https://go.dev/ref/spec#Method_sets),
[Go scopes](https://go.dev/ref/spec#Declarations_and_scope), and
[Go module syntax and roots](https://go.dev/ref/mod#go-mod-file).
