# Java declarations and candidate graph — 2026-10-24

The pinned RAGRAF Go/Java reference is commit
`075caf86dc2f6274b9c48487e521915bcdfeaf2c`; its Java generic profile extracts
methods/constructors, class/interface/enum declarations, calls and inheritance.
It explicitly does not resolve parameter/return types. The local Java extension
uses tree-sitter-java 0.23.5 and adds source package/import/type/arity candidate
binding, constructor/method references, records and annotation declarations.
This source comparison is not an executed full-reference quality benchmark.

Thirteen Java unit fixtures cover captured imports, constructor/overload arity,
same-arity ambiguity, inherited overloads, overrides, super calls, private base
methods, typed own fields, static receiver restrictions, interface contracts,
static/wildcard import ambiguity, missing imports, cross-project isolation even
with identical paths, hierarchy cycles, anonymous/lambda degradation, compact
record constructors, method/constructor references, class/interface priority,
comments in qualified names/imports, enhanced-for scopes, record components and
annotation values that cannot spoof method modifiers.
An SQL boundary fixture also checks that ordinary quoted Java SQL is captured
while unmodeled Java text blocks do not produce table identifiers.

The actual stdio MCP fixture uses mock embeddings and an isolated store. It
checks constructors, arity/ambiguity, inherited declarations, `this.field`, static
imports, `super`, interface contracts, `service::method`, `Type::method`,
`Type::new`, compact records, entrypoint candidates and explicit method bodies.
Unchanged sync reuses parses. Direct ingest of a changed base excludes its stale
facts, and source synchronization recomputes the hierarchy for unchanged child /
caller documents. Restoring the base restores its captured targets.

`runtime.json` records Java MCP checks. `go-regression.json` checks the earlier Go
module/receiver/boundary behavior on the updated server. `verification.json`
records the actual library results and executable digest; `source-sha256.json`
pins saved sources. Reproduce with:

```sh
cargo build --bin rag-mcp
cargo test --lib
python3 scripts/smoke_code_java.py target/debug/rag-mcp /tmp/java-index.json
python3 scripts/smoke_code_go.py target/debug/rag-mcp /tmp/java-go-regression.json
```

Syntax-cache format 15 invalidates earlier derived parses. Immutable revision
graphs store Java call/reference/inheritance evidence. Local schema v24 and
external database capture format 4 are unchanged. Existing production stores and
servers were not opened or restarted. Neither javac nor source Java execution is
part of this qualification; these results do not establish deployment.

Limits: static source candidates are not compiler-approved invocation targets.
JDK/dependency/classpath/JPMS selection, full access rules, type arguments/erasure,
conversion-based overload selection, target SAM typing, most-specific interface
defaults, outer-instance/inherited/chained field lookup, pattern scopes, factories,
reflection, anonymous/lambda ownership, generated members, initializer order,
Joern/data flow, Java SQL text-block values and performance at scale remain
unqualified or unimplemented. Missing/ambiguous hierarchy inputs prevent a unique
verdict. Interface targets are declarations, not runtime implementations. Main and
test markers are candidates. Full RAGRAF parity and superiority remain unproven.

Primary semantic references:
[JLS 21 invocation and method references](https://docs.oracle.com/javase/specs/jls/se21/html/jls-15.html#jls-15.12),
[JLS inheritance/overriding](https://docs.oracle.com/javase/specs/jls/se21/html/jls-8.html#jls-8.4.8),
and [JLS imports](https://docs.oracle.com/javase/specs/jls/se21/html/jls-7.html#jls-7.5).
