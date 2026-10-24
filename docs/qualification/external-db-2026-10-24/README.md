# External database block: local qualification, 2026-10-24

This report covers the new native PostgreSQL/MySQL metadata snapshot subset. It
does not close the full RAGRAF transfer/improvement goal. The source working tree
contains earlier unrelated work; this qualification neither commits nor deploys
it. See [the operation contract](../../EXTERNAL_DATABASES.md) and
[the feature matrix](../../RAGRAF_FEATURE_MATRIX.md).

## Actual evidence

- `cargo test --lib`: **747 passed, 0 failed, 1 ignored**. The ignored test requires
  explicitly provisioned external fixtures and was separately executed below.
- `python3 scripts/smoke_external_db.py report.json`: passed with cached official
  Docker images `postgres:16-alpine` and `mysql:8.4`, local Docker host. The script
  built the actual `rag-mcp` binary, created uniquely owned containers and random
  temporary credentials, and removed its own containers and volumes afterwards.
- Native fixture test: both SELECT-only accounts could capture metadata, including
  composite foreign-key column order, generated columns and quoted table names.
  Metadata-only captures contained no fixture data marker; explicit samples contained
  only allowlisted nonsensitive columns. NULL survived separately from empty text.
- The same connector's read-only transaction rejected INSERT even with an admin
  account, with SQLSTATE `25006` on both servers. No production database was used.
- Default verified TLS rejected the local untrusted/no-TLS fixture endpoints,
  rather than downgrading. Successful trusted-CA TLS is **not** qualified.
- Immutable captures survived store reopen. Equal timestamps were ordered by
  insertion sequence. No baseline returned `compared: false`; identical metadata
  and changed samples produced zero schema changes; a column change was reported;
  a changed target fingerprint was refused as incomparable.
- Code/source synchronization produced static SQL and migration comparisons in
  the configured project, including an unobserved table and a missing live column.
  Live tables without observed static uses were explicitly not safe to delete.
- Real stdio MCP listed and called `db_refresh`, `db_schema`, `db_table`,
  `db_relations`, `db_sample`, `db_drift`, `db_cross`. Samples required owner policy,
  allowlist and refresh flag; historical captures without samples stayed empty.
- Real HTTP MCP allowed snapshot reads with Read, denied refresh to Read and Write,
  and allowed refresh to Admin. URL fields in caller requests were rejected;
  a SQL-like table filter was treated as an exact data value.
- After stopping only the owned MySQL container, live refresh failed and the
  previously published snapshot remained available through MCP with no network.
  URL credentials were absent from captured server stderr.

Machine-readable MCP results: [runtime.json](runtime.json). Source hashes:
[source-sha256.json](source-sha256.json). A final native and MCP rerun also checks
SQL-escaped quotes inside migration column names, so a real `a"b` column is not
misreported as missing from the live table.

## Limits still open

These tests cover two local server versions, a small fixture, and selected
failure boundaries. They do not qualify indexes, CHECK constraints, view
metadata/dependencies, inferred relations, full types/domains, ORM/type mappings,
all dialect casing behavior, privilege/concurrent-DDL exhaustiveness, snapshot
export/replication, trusted-CA TLS interoperability, large-scale budgets/latency,
other operating systems or superiority to RAGRAF on representative corpora.

Sampling name/value heuristics do not guarantee anonymity. Owner allowlists and
explicit sampling permission are required. No owner database configuration was
installed and no existing RAG process was restarted.
