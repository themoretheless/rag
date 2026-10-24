# SQL code analysis qualification — 2026-10-24

Existing migration extraction, table usage and write checking were verified together through the actual stdio MCP binary. A reproducible regression script was added at `scripts/smoke_code_sql.py`.

Verification:

- `cargo test --lib code_index::sql::`: 9 passed, 0 failed.
- `cargo build --bin rag-mcp`: passed.
- `python3 scripts/smoke_code_sql.py target/debug/rag-mcp docs/qualification/sql-code-analysis-2026-10-24/runtime.json`: passed.

The isolated fixture verifies CREATE plus ALTER migrations, read/write locations with owning functions, accepted insert/delete plans, rejected unknown columns, missing required columns, NULL and empty updates. It also verifies unchanged-cache reuse, an indeterminate verdict after direct ingestion makes the index stale, and automatic schema refresh through source synchronization.

The process uses a temporary database and mock embeddings. No SQL is executed and the production database is untouched. This is source-derived validation: migration execution order is not established, and a valid plan does not prove live database success. Full coverage and limitations are documented in `docs/SQL_CODE_ANALYSIS.md`.
