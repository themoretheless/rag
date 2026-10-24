# C/C++ source index — 2026-10-24

Reference: RAGRAF `075caf86dc2f6274b9c48487e521915bcdfeaf2c`,
`extract/generic.py` C/C++ profiles (functions, structures/classes, calls and
includes). The local extension uses tree-sitter-c 0.24.2 and tree-sitter-cpp
0.23.4. It adds source prototype/body distinction, bounded same-project captured
include graphs, static boundaries, arity candidates, typed members and source
base lookup. This is not an executed full-reference superiority benchmark.

Eleven unit fixtures cover C function/function-pointer declarators and callback
values, relative guarded headers and external definitions, missing declaration
visibility, static/project boundaries, C-compatible versus C++ header parsing,
methods/namespaces, inherited members and C++ name hiding, same-arity overloads,
macro/conditional uncertainty, system include paths left unresolved, C unspecified
parameter lists versus C++ prototypes, default arguments, include/declaration order,
operator source symbols, field visibility, C++ raw SQL line fidelity, and scopes
of specialized/anonymous types that cannot become free functions.

The isolated actual stdio MCP fixture checks tool discovery, C/C++ parse reuse,
source prototypes/bodies, typed and inherited methods, static members, indirect
calls left unresolved, callback references, overload ambiguity, type creation
without fabricated bodies, structural bases and raw SQL table-use line evidence.
Changing a captured header first excludes its stale facts; sync then recomputes
bindings for unchanged callers. A missing prototype cannot be replaced by an
unrelated project-global definition. Restoring the header restores target IDs.

Independent compiler probes use the installed Clang: syntax-only C11/C++17 checks
on the positive fixtures, plus expected rejection of an undeclared C call and a
C++ hidden base overload. No compiled fixture code executes and no linker proof
is claimed. The actual compiler version is recorded in `runtime.json`.

Evidence: `runtime.json`, Java/Go/reference regression reports, `verification.json`
and `source-sha256.json`. Reproduce with:

```sh
cargo build --bin rag-mcp
cargo test --lib
python3 scripts/smoke_code_c_family.py target/debug/rag-mcp /tmp/c-family.json
```

Syntax-cache format 16 invalidates older derived parses; history freezes the new
source evidence. Local schema v24 and external capture format 4 are unchanged.
The gateway itself never invokes Clang, a preprocessor, linker or filesystem
include search. Compiler probes belong only to the disposable qualification
script. Existing user stores and running servers were not opened/restarted.

Limits: source includes do not establish a compiled translation unit. Header guard
recognition assumes a conventional source pattern; build macros may change it.
System/macro include search, build flags/compilation databases, preprocessing,
ABI/language linkage, ODR/inline rules, modules, using/type aliases, operator and
semantic template/conversion overload resolution, access rules, virtual/multiple
inheritance subobjects, initializers, lambdas, generated code and performance at
scale remain unqualified or unimplemented. Structural bases are not a complete
C++ hierarchy certificate. Statically selected definitions remain derived source
candidates, not runtime/linker evidence. Remaining RAGRAF features and languages
are still open in the feature matrix.
