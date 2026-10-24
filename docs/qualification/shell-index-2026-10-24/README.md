# Bash/sh structural index — 2026-10-24

Added `.sh` / `.bash` function symbols, command references, candidate call binding, exported function values and captured source dependencies. Registration replay respects source order, redefinition, explicit function removal and subprocess boundaries. Functions overriding ordinary builtin command names are resolved before applying builtin namespace effects. SQL constants use shell-specific decoding; dynamic/concatenated literals are not fabricated as table references.

Verification:

- `cargo test --lib`: 809 passed, 0 failed, 1 ignored.
- Five dedicated shell fixtures cover declaration order/removal, captured source/project/CWD boundaries, subshell isolation/dynamic commands, builtin overriding/export references and constant/dynamic SQL.
- `cargo build --bin rag-mcp`: passed.
- Actual stdio MCP regression: `scripts/smoke_code_shell.py`, report `runtime.json`.
- SQL MCP regression: `scripts/smoke_code_sql.py`, report `sql-regression.json`.
- Python/Rust/TS bound-reference regression also passed (`references-regression.json`).

No shell script, SQL statement or production database was executed/modified. Index format is 17; old caches are invalidated. See `docs/SHELL_CODE_ANALYSIS.md` for coverage limits. Relative source paths, expansion, runtime registration, external environment, full alias semantics and arbitrary execution effects remain unverified. This qualifies a subset of shell source analysis and does not complete the broader RAGRAF transfer goal.
