# Offline SQL code analysis

The full MCP surface (`RAG_TOOLS=full`) exposes `schema`, `table_usage` and
`write_check`. These inspect ingested sources and never connect to or execute SQL
against a live database. The existing `get_schema` tool is the agent/wiki
conventions document; `schema` is the separate SQL schema operation.

## Indexing and migration inputs

Use `sync_sources` to ingest a repository. Its existing finalization now also
indexes `.sql` / `.SQL` files and static SQL literals in Rust, Python, JS/TS, C/C++ and ordinary Java quoted strings, plus constant Bash/sh quoted literals.
Shell expansions and unmodeled quoting emit `SHELL_SQL_VALUE_UNVERIFIED`;
no script is executed.
C++ raw literals preserve delimiters, backslashes and physical source lines.
Java SQL text blocks emit `JAVA_SQL_TEXT_BLOCK_VALUE_UNVERIFIED` instead of
publishing names from unmodeled indentation/escape processing.
After direct document ingest or source changes outside sync, run `index_code`.
Older syntax-cache formats are invalidated; stale facts are omitted and reported.

DDL contributes to the schema from SQL files, code under `migration` /
`migrations` directories and conventional `schema.rs`, `schema.py`, `schema.js`,
`schema.ts`, `migrations.rs` and `migrate.rs` files. These are source conventions,
not discovery of a production migration runner. Within a project (`wing`),
files replay in lexicographic source-path order and statements in source order.
Numeric migration order, database connections, transactions and applied versions
are not inferred. Conflicting creates and unknown alterations are diagnosed.
Separate databases should have separate project scopes or qualified table names.

Supported schema changes:

- `CREATE TABLE` with explicit column definitions, types, `NOT NULL`, defaults
  and generated-column/default metadata. Format 19 retains explicit
  `default_expression`, `generated_expression` and `generation_clause` as
  canonical SQL AST text; they are not executed. SET/DROP DEFAULT refreshes
  the captured default expression. Implicit serial/identity defaults are not
  fabricated as explicit expressions.
- `ALTER TABLE ADD/DROP COLUMN`, `RENAME COLUMN`, `RENAME TO` (preserving an
  existing schema qualifier), `SET/DROP NOT NULL`, `SET/DROP DEFAULT`, column type
  changes, and `DROP TABLE`.
- `IF NOT EXISTS` for create/add and `IF EXISTS` for dropped columns.

Format 20 captures table and column `CHECK`, `UNIQUE`, primary-key and
foreign-key declarations as canonical AST text with names/kinds. `ADD CONSTRAINT`
and named `DROP CONSTRAINT` replay this metadata; `NOT VALID` is retained
separately. Constraint arrays use deterministic ordering. Duplicate declared
names, unknown drop targets and unsupported CASCADE effects are diagnosed.

Constraint capture and evaluation are separate. Migration `write_check` uses the
shared bounded offline interpreter for supported CHECKs when a literal row is
supplied. It does not enforce key, uniqueness or foreign-key data. Implicit primary-key nullability
and generated server names are not inferred. Destructive/rename DDL involving
constrained or generated-column schemas emits
`SCHEMA_DEPENDENCY_REWRITE_UNVERIFIED`: dependency text is not rewritten as if a
database had applied the migration. The project stays incomplete and cannot
receive a positive write-check verdict based on that uncertain schema.

`.down.sql`, `.rollback.sql`, `/down/` and `/rollback/` inputs do not alter the
forward schema. SQL scripts stop at `-- migrate:down`, `-- +goose Down` or
`-- +migrate Down`; literals owned by `down` / `downgrade` functions are excluded
from schema replay. Literal DDL inside migration code is visible but marked
`CODE_MIGRATION_EXECUTION_ORDER_UNVERIFIED`, because AST/source order does not prove
runtime execution order. Recognized ORM migration-builder calls such as
`op.create_table` / `knex.createTable` are diagnosed as unsupported; their schemas
are not fabricated. Arbitrary migration DSLs are outside the current coverage.

Schemas retain document IDs, paths, source lines, revisions and content hashes.
Quoted identifiers preserve significant case; ordinary identifiers use SQL-standard
lowercase folding. Dialect-specific identifier semantics are not inferred.

## Tools

```json
{"name":"schema","arguments":{"project":"demo","table":"public.users"}}
{"name":"table_usage","arguments":{"project":"demo","table":"users","mode":"write","limit":50}}
{"name":"write_check","arguments":{"project":"demo","table":"public.users","operation":"insert","columns":["name","email"]}}
{"name":"write_check","arguments":{"project":"demo","table":"public.users","operation":"update","values":{"name":null}}}
```

`schema` can list all tables or filter by project/table. An unqualified table name
matches qualified names only as a lookup convenience; `write_check` requires one
unambiguous table/project match and never guesses a database search path.

`table_usage` supports `all`, `read` and `write`. A SQL AST visitor finds relation
references in joins and subqueries; targets of INSERT, UPDATE and DELETE produce
write records. UPDATE/DELETE targets can also be read relations. Static code
literals retain the owning indexed symbol when available. Actual multiline SQL
retains relation line offsets; escaped literals use their source literal start
instead of invented physical source lines. Comments are not queries. Ordinary
CTE aliases are excluded from physical table results; CTE name shadowing and
recursive visibility remain explicitly diagnosed coverage limitations.

Dynamic/interpolated strings, unsupported literal escapes, SQL parse failures,
unsupported write forms (e.g. MERGE/TRUNCATE and multi-target DELETE) are diagnosed
rather than turned into inferred targets. ORM-generated queries and indirect
runtime query construction are not covered. `schema` / `table_usage` return
`analysis_incomplete`, diagnostics, stale/unindexed coverage and truncation.

`write_check` accepts `insert` (default), `update` or `delete`, assigned `columns`
and optional literal `values`. Values' keys are included among assigned columns;
values themselves are not echoed. Unknown request fields are rejected, so a typo
such as `value` cannot silently skip validation of the supplied row. It checks:

- Known table and column names, duplicate columns and nearest-name suggestions.
- Omitted explicit `NOT NULL` columns without defaults/generated values on insert.
- Explicit NULLs against explicit `NOT NULL` declarations.
- Empty updates and assigned columns supplied to a delete plan.
- Supported CHECK expressions against literal rows: integer comparisons, SQL
  null/boolean logic, bounded IN/BETWEEN predicates and supported length/cast
  forms, using the same interpreter as captured external schemas.

Named column/table CHECKs are parsed as declarations, never executed. NOT VALID
constraints still apply to new rows; explicit NOT ENFORCED declarations are
reported as skipped. Evaluator version 2 decodes scalar literal defaults for
omitted insert columns: NULL, booleans, TEXT strings and range-compatible
SMALLINT/INT/INTEGER/BIGINT integers. It accepts parentheses and unary numeric
signs, not arithmetic, functions or casts. `inferred_default_columns` reports
only column names, never default values. NOT NULL with a literal DEFAULT NULL
is rejected for literal insert plans. Explicit unknown assignments override the
omitted-default path, and updates never receive insert defaults.

Unknown explicit default expressions, unsupported types/coercions or CHECK
syntax, generated values and unavailable update-row context produce `unverified` records
and an indeterminate verdict. Omitted nullable insert columns without defaults
are SQL NULL; assigned columns with no literal value remain unknown. `schema_complete`
describes schema coverage independently of missing row context. Column-only
plans retain their narrower scope and set `literal_row_requested: false`.

Default/context construction and CHECK evaluation share a 4096-node budget;
context literal values also have a 64 KiB budget. At most 512 predicates are
checked per request. Literal rows are limited to 64 KiB, 4096 nodes and depth 32. Literal
values are never echoed. Deletes do not run new-row CHECKs; their live referential
and trigger effects remain unverified.

Evaluator version 3 accepts `default_columns` without an external connection.
These are explicit SQL DEFAULT assignments and join the target-column set;
optional VALUES counts include them. Supported literal defaults can therefore
be checked for insert or update. An explicit DEFAULT on a nullable column with
no declared default uses NULL. Undeclared NOT NULL defaults, implicit identities
and generated values stay unverified rather than being invented.

A literal value and DEFAULT for the same normalized column are rejected;
duplicate DEFAULT targets and assignments on delete plans are invalid.
`explicit_default_columns` and `inferred_default_columns` identify the two scopes
without returning values. Omitted update columns still receive no default.

```json
{"name":"write_check","arguments":{"project":"demo","table":"users","operation":"update","default_columns":["age"]}}
```

The verdict is `valid`, `invalid` or `indeterminate` (`valid: null`). Missing or
ambiguous schemas, stale/unindexed inputs, parse failures or unsupported schema
changes prevent a positive verdict. Missing tables in an otherwise known complete
derived schema are invalid within that derived schema. `schema_evidence` and
`checked_columns` make the result reviewable. At most 256 column/value entries
are accepted; identifiers are parsed, not concatenated into executable SQL.

A `valid` result means compatible with the **derived table/column plan and explicit
NOT NULL/default metadata and supported offline CHECKs for supplied literal rows**. It is not permission or proof that a live INSERT will
succeed. Types/coercions, implicit primary-key/identity behavior, uniqueness,
foreign keys, unsupported CHECK semantics, permissions, triggers, actual data and live schema
state are listed in `not_checked` and are not verified by this tool.

## Checking an immutable external capture

Pass `connection` to select an explicitly named captured database instead of the
source-migration schema. Optional `snapshot_id` selects history; `schema` and
`table` are exact catalog names. The check reads local metadata only, including
when the database is unavailable, and never refreshes it or executes a write.
Omit `connection` to retain the migration behavior described above.

```json
{"name":"write_check","arguments":{"connection":"orders","schema":"public","table":"users","values":{"id":17}}}
{"name":"write_check","arguments":{"connection":"orders","table":"events","operation":"update","values":{"state":"new"}}}
{"name":"write_check","arguments":{"connection":"orders","table":"users","columns":["id","name"],"values":2}}
{"name":"write_check","arguments":{"connection":"orders","table":"users","default_columns":["id"],"values":{"name":"Ada"}}}
```

`values` accepts a literal object or a VALUES expression count; `value_count` is
an alternative count argument. Conflicting counts are rejected. A count without
explicit target columns is indeterminate. `default_columns` distinguishes SQL
DEFAULT from JSON null. PostgreSQL insert plans can explicitly request
`overriding_system_value` for GENERATED ALWAYS identities; this is not allowed
on update or for other dialects.

Captured checks distinguish ordinary required primary keys from identities and
MySQL auto increment, computed columns from DEFAULT_GENERATED defaults, and
GENERATED ALWAYS from BY DEFAULT identities. They check integer ranges, supported
literal types and PostgreSQL enum labels. MySQL enum/SET spelling outside the
captured members is indeterminate because collation and SQL mode can change
coercion. Arrays and unsupported types remain unverified.

A bounded, offline CHECK interpreter supports integer comparisons, SQL null and
boolean logic, BETWEEN, IN, null predicates, limited integer/text casts and
character-length functions. It checks applicable scalar-domain constraints as
well as table CHECKs. NOT VALID PostgreSQL constraints still apply to new rows;
MySQL NOT ENFORCED constraints are skipped. Missing update-row context, unknown
defaults, arbitrary functions, unsupported syntax and exhausted budgets produce
an unverified result. It never evaluates SQL on a server. Input literal rows are
limited to 64 KiB, 4096 nodes and depth 32; CHECK evaluation has a shared node
budget and at most 512 predicates per request.

`valid` covers the captured declarations and supported assignment checks only.
`live_write_success` stays null, `executed` and `live_query` stay false. Data
conflicts, actual unique/foreign keys, exclusions, permissions, triggers, generated
expression results, server coercions, encoding, temporal ranges and timezone
behavior are not proven. Literal values are never echoed in the result.

## Reproducible MCP regression

Run `python3 scripts/smoke_code_sql.py target/debug/rag-mcp` after building the binary.
It uses an isolated temporary database with mock embeddings and exercises schema
extraction, table read/write locations, write checks, stale-input rejection and
automatic schema refresh through `sync_sources`. No SQL is executed.
