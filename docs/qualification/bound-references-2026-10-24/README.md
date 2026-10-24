# Bound symbol references — 2026-10-24

`references` now defaults to occurrences bound to symbol IDs through the existing
static import/type/lexical resolver. `reference_mode=lexical` explicitly retains
name-only candidates. Flat source fields remain; bound results add candidate IDs,
resolution, basis and derived provenance. Empty results never prove absence.

Qualified non-call expressions are retained: `service.run` cannot accidentally
become a use of a bare unrelated `run` function. Type identifiers are indexed;
declaration/parameter/keyword names are excluded. Import sites link through the
local imported name, and the original name of an aliased import cannot bind to
an unrelated function in the importing file. Default/rest parameter shadowing
also applies to call bindings. Bound callback values seed transitive conservative
dead-code reachability, preserving their callees without inventing a direct call.

Syntax-cache format 13 invalidates older derived parses. Immutable revision
graphs freeze the bound occurrences together with their calls. Current source
reads still omit stale/unsupported cache inputs and retain diagnostics. No schema
DDL or external DB capture format change is needed.

The unit fixtures cover Python aliases, default/rest/typed parameter shadowing,
Rust function values/qualified methods/types, TypeScript default/namespace imports,
inherited member values, same-name isolation, project boundaries, ambiguous
bindings, keyword/import false positives and callback transitive liveness.
The actual stdio MCP binary checks tool schema, precise source evidence, bound /
lexical separation, deterministic limits, unchanged cache reuse, omission of a
changed document before reindex, and automatic refresh on source sync. Isolated
mock embeddings and disposable stores are used; no existing database or running
server is opened/restarted.

`runtime.json` and `verification.json` record observed checks and test totals;
`source-sha256.json` pins relevant saved sources. Reproduce with:

```sh
cargo build --bin rag-mcp
cargo test --lib
python3 scripts/smoke_code_references.py target/debug/rag-mcp /tmp/bound-references.json
```

This does not establish complete RAGRAF parity or general superiority. Runtime
dispatch, arbitrary callback alias chains, dynamic property access, destructured
bindings, compiler/package metadata and remaining languages are incomplete.
Explicit override relations are not yet merged into these reference results.
The reference facade also covers non-code relation kinds (translation keys,
package variables, SQL queries and routes); this slice does not unify those into
the code-symbol index. The existing history diff does not yet compare arbitrary
reference relations. Unresolved occurrences and lexical matches remain distinct
from bound evidence. Import-local token lookup is bounded to 4096 AST nodes.
