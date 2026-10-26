# Source-documentation subset — 2026-10-25

The earlier status always marked documentation coverage unavailable. The new
[contract](../../CODE_DOCUMENTATION.md) records raw source-bound candidates and
measures only qualified syntax forms, leaving full language/compiler coverage
unverified. Syntax format 27 refreshes cached facts; schema stays 25.

Six native regressions cover Python class/function literal-docstring rules,
Rust outer/inner/attribute distinctions and exact raw span/hash, exported/single
arrow ownership versus ambiguous/file tags, Java/PHP classes/methods, Unicode
excerpt and 64-segment truncation, and a scoped-compatible ratio that excludes
unsupported Lua declarations and never returns doc text. Empty measurements stay
null rather than false 0% coverage.

The real stdio MCP scenario checks source sync and persisted/reused doc facts,
symbol/code agreement, UTF-8 source spans, project-scoped coverage, unsupported
language exclusion, body-free status, unknown stale coverage and refreshed
source/hash after sync. It uses isolated mock-provider databases; no source
applications/doc generators/macros are executed and no production state changes.

Reference scope is docblock presence/coverage in RAGRAF `op_index_status` at
revision `075caf86dc2f6274b9c48487e521915bcdfeaf2c`. The local metric is explicitly
supported syntax coverage, not full reference/language coverage or representative
quality superiority. Go/C/C++/shell/Lua and other doc styles, attributes/macros,
compiler rendering, tags, semantic completeness and scale remain open.

```sh
cargo test --lib code_index::documentation --locked --offline
cargo build --bin rag-mcp --locked --offline
python3 scripts/smoke_code_documentation.py target/debug/rag-mcp /tmp/code-documentation.json
```

Final verification: **1,215 workspace tests passed, 0 failed, 1 ignored**;
all six new documentation regressions passed. Workspace Clippy with `-D warnings`,
formatting and `git diff --check` passed. The rebuilt binary passed
[documentation MCP evidence](runtime.json), [status](status-regression.json),
[bulk SQL](sql-regression.json) and [Lua](lua-regression.json) regressions.
[Verification manifest](verification.json) records working-tree source/binary
hashes. The owned fixture has scoped syntax coverage 0.5, with unsupported Lua
excluded; that number is not documentation coverage of the user's repository or
a representative corpus. Actual MCP evidence includes body-free status, stale
coverage becoming unknown and refreshed doc text/hash after sync.
