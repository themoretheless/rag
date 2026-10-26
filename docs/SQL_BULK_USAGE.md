# Bulk SQL table-use subset

`table_usage` now distinguishes the following parsed operations in raw SQL and
supported static source literals. These are static mentions/roles, not proof a
statement ran, matched rows, passed permissions or was valid on a live server.
No ingested SQL, COPY endpoint, file or program is executed/read by this feature.

| Syntax | Explicit role |
|---|---|
| MERGE target USING source | target/source query factors are reads; target is also a write if a clause contains INSERT, UPDATE or DELETE. |
| MERGE with only DO NOTHING | reads only; no mutating target inferred. |
| TRUNCATE TABLE targets | each explicitly named target is a write, not a read. |
| COPY table FROM ... | named table is a write (operation copy_from). |
| COPY table TO ... | named table is a read (operation copy_to). |
| COPY (query) TO ... | underlying query tables are reads; CTE alias names are excluded. |

`MERGE`/`TRUNCATE` operations are named merge/truncate. COPY directions describe
PostgreSQL-style parser AST fields, not a guess from endpoint text. Existing CTE
scope limitations remain diagnostic. Mutating MERGE predicates are not evaluated:
a syntactic action records a possible write even if its condition is false.
Input selection reads for COPY FROM are not invented from file/program names.

MERGE non-plain target factors remain explicitly unverified. Named OUTPUT INTO
secondary sinks are now covered by [the secondary-write contract](SQL_OUTPUT_USAGE.md);
computed and unresolved sinks remain unverified. TRUNCATE without ONLY,
explicit descendant syntax or CASCADE records
TRUNCATE_DESCENDANTS_OR_CASCADE_UNVERIFIED: other affected tables, inheritance,
foreign keys and trigger effects are not guessed. COPY query-FROM is unsupported.
COPY INTO/cloud-stage dialects, indirect ORM/generated SQL, dataflow, functions,
triggers and live permissions/data remain outside this subset.

Supported static literals preserve owning symbol and source evidence; dynamic
interpolation is never evaluated. Dynamic/unsupported literals beginning with the
new keywords are diagnosed through the existing extractor. Bare `copy` labels do
not trigger SQL parsing; COPY(query) without whitespace is supported. Raw SQL
still follows the explicit SQL parser error contract.

`write_check` is unchanged: only insert/update/delete assignment checks are
supported. MERGE/TRUNCATE/COPY are not silently accepted as validated writes.
This extension is table-use evidence, not bulk-write execution/schema validation.

Syntax index format **26** refreshes older extraction caches through normal
index_code/source sync. Database schema remains 25. Old immutable revisions keep
their original facts; analysis-format changes remain visible in diff. No
production index was migrated or service restarted for this qualification.
