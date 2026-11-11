# SQL secondary write evidence

`table_usage` records named `OUTPUT INTO` sinks on INSERT, UPDATE, DELETE and
mutating MERGE as `insert_output_into`, `update_output_into`, `delete_output_into`
and `merge_output_into`. A target column list is accepted only as a plain list
of identifiers. OUTPUT without INTO and RETURNING create no secondary sink.
MERGE with exclusively DO NOTHING actions creates no write.

`SELECT INTO` records a named syntactic candidate as `select_into`. Source tables
remain reads; sinks do not become reads merely because they are INTO targets.
Supported static literals retain owner and physical source line. Normal project
filtering, stale-document exclusion and synchronization refresh apply.

These are static candidates, not verified physical tables or executed writes.
Prefix variables (`@`, `:`, `$`), multiple targets and computed expressions are
unresolved and emit SQL_INTO_TARGET_UNRESOLVED. Unprefixed local variables in
routines cannot be bound without dialect/scope information. SELECT INTO emits
SELECT_INTO_SCHEMA_UNVERIFIED: projection-created columns are not inferred, and
migration coverage becomes incomplete. Quoted names retain canonical case.
Existing CTE/dialect limitations, triggers and indirect effects remain open.
No SQL, source program, file endpoint or external data is executed or read.

`write_check` still validates only its supported insert/update/delete assignment
subset. Secondary write operations are not silently accepted as schema-validated.
Syntax index format 28 refreshes prior caches on normal indexing/source sync;
database schema remains 25. Historical revisions retain their original facts.
