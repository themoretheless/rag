# Lua-index subset — 2026-10-25

Reference RAGRAF `extract/generic.py` includes Lua function definitions/calls at
revision `075caf86dc2f6274b9c48487e521915bcdfeaf2c`. Previously Lua was readable
text but had no local structural index. The [new extractor](../../LUA_CODE_ANALYSIS.md)
adds source symbols and conservative lexical function/value links, while dynamic
globals/imports/table dispatch remain explicit unknowns.

Seven new parser/binder regressions cover local/named/assigned function bodies,
recursion and initializer visibility, parameter/block/member shadows, assignments
inside closures and non-local function statements, numeric/generic loops and
repeat conditions, computed-vs-named table keys, aliases/globals/method self,
comments/long strings and parse-error exclusions. No same-name cross-document
lookup is performed. Syntax format 25 makes older cached extraction stale.

The real stdio MCP scenario checks source sync, definitions, known local calls and
value references, unresolved aliases, foreign global calls not linked to local
functions, exact Unicode-preceded UTF-8 body spans, partial structural slice,
cache reuse, stale-source exclusion and rebinding after sync. It runs only a mock
embedding server/database on owned temporary sources; no Lua code is executed.
No compiler or Lua runtime is used as a validation gate here.

The grammar comes from the existing local Cargo cache (0.5.0); Cargo.lock changes
only add that package and its root dependency, with no unrelated version updates.
Capability counts are extractor entries, not a claim of all 16 reference languages.
Imports/alias data flow, global env/table dispatch, compiler/dialect coverage,
SQL/framework extraction, broader precision/recall/scale and full parity remain open.

```sh
cargo test --lib code_index::lua --locked --offline
cargo build --bin rag-mcp --locked --offline
python3 scripts/smoke_code_lua.py target/debug/rag-mcp /tmp/lua-index.json
```

Final verification: **1,206 workspace tests passed, 0 failed, 1 ignored**;
all seven new Lua tests passed. Workspace Clippy with `-D warnings`, formatting
and `git diff --check` passed. The rebuilt binary passed
[Lua MCP navigation](runtime.json), [status](status-regression.json),
[timings](timings-regression.json) and [Go](go-regression.json) regressions.
Status now exposes 15 extractor entries, including partial Lua; this is not
15-of-16 reference-language parity. [Verification manifest](verification.json)
records working-tree source/lockfile/binary hashes. Actual MCP evidence includes
exact UTF-8 body extraction after a non-ASCII comment, no false foreign global
link, warm-cache reuse, stale omission and refreshed local binding. These checks
exercise the MCP analysis runtime; they do not execute or qualify a Lua program.
