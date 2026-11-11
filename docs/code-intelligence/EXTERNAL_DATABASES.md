# External PostgreSQL and MySQL metadata

`RAG_TOOLS=full` exposes `db_refresh`, `db_schema`, `db_table`, `db_relations`,
`db_sample`, `db_drift` and `db_cross`. These are separate from `schema` and
the default migration mode of `write_check`. Passing `connection` to
`write_check` checks an explicitly selected frozen capture without connecting or
executing writes; see [SQL checks](SQL_CODE_ANALYSIS.md#checking-an-immutable-external-capture).

## Owner configuration

Set `RAG_EXTERNAL_DATABASES` to a JSON object of named targets. Credentials belong
in separate URL environment variables; tool arguments accept only an alias.
There is no arbitrary query, URL, WHERE, ORDER BY or SQL expression argument.

```json
{
  "orders": {
    "dialect": "postgresql",
    "url_env": "ORDERS_DATABASE_URL",
    "project": "orders-service",
    "schemas": ["public"],
    "tls": "verify_full",
    "samples_enabled": false,
    "sample_columns": {}
  }
}
```

Use `mysql` for the other dialect. Schemas are explicit and bounded (PostgreSQL
schemas within the connection database; MySQL database names). The project must
match the source corpus wing for `db_cross`. Aliases, projects, schema names and
column allowlists are case sensitive. Unknown configuration and request fields
are rejected. No connections are configured by default.

TLS verifies the certificate and hostname by default. An owner can explicitly set
`tls: "disable"`, as used by the disposable local test fixtures. There is no
automatic plaintext fallback. Connection/driver errors are sanitized; credentials,
URLs and driver statements are not published in snapshots or tool errors.
Schema metadata itself (e.g. column defaults) can contain confidential application
information and is retained locally for authorized readers.

Use a SELECT-only database account. The collector additionally starts a read-only
transaction, with a 30-second total timeout, 10-second statement timeout and a
PostgreSQL lock timeout. It queries native metadata through SQLx; it does not
invoke an external database CLI or execute caller SQL. MySQL uses a consistent
InnoDB read snapshot, but this is not a guarantee of transactional DDL capture.
Remote schema and source files are not one distributed atomic snapshot.

## Operations

```json
{"name":"db_refresh","arguments":{"connection":"orders"}}
{"name":"db_schema","arguments":{"connection":"orders","limit":50}}
{"name":"db_table","arguments":{"connection":"orders","schema":"public","table":"orders"}}
{"name":"db_relations","arguments":{"connection":"orders","table":"orders"}}
{"name":"db_sample","arguments":{"connection":"orders","table":"orders"}}
{"name":"db_drift","arguments":{"connection":"orders"}}
{"name":"db_cross","arguments":{"connection":"orders"}}
```

`db_refresh` requires Admin over HTTP. Other operations require Read and perform
no network access. Refresh publishes a new immutable local snapshot only after
collection and rollback succeed; an error leaves previous captures available.
The local schema is version 24. Snapshot insertion sequence, rather than wall
clock or random UUID order, defines latest/previous. Read tools accept
`snapshot_id` for history, even if the target's credentials are no longer set.

`db_schema` lists visible tables, views and PostgreSQL materialized views.
`db_table` gives columns (native type/length/precision, nullability, defaults,
identity/generation metadata), declared FK mappings and structural `metadata`:

- Indexes preserve ordered keys, expressions, direction, uniqueness and method.
  PostgreSQL adds INCLUDE columns, partial predicates, validity/readiness and
  NULLS NOT DISTINCT; MySQL adds prefix lengths and optimizer visibility.
  Unknown or inapplicable attributes remain NULL. GiST/HASH keys are not assigned
  a fabricated B-tree sort direction.
- Constraints preserve primary/unique/FK/CHECK and PostgreSQL exclusion metadata,
  referenced column order, validation/enforcement and deferral flags. PostgreSQL
  `NOT VALID` and MySQL `NOT ENFORCED` are distinct from fully validated checks.
  NOT NULL is represented in column nullability. Capturing a constraint does not
  evaluate it against arbitrary proposed row data.
- View metadata contains a bounded definition, a SHA-256 digest of the full
  definition, server options and direct relation dependencies from native catalogs.
  PostgreSQL materialized views include their columns without reading their rows.
  Function dependencies, recursive self-reference analysis and dependency expansion
  through arbitrary dynamic SQL are not collected.

`column_traits` separates ordered composite primary/unique keys from individually
unique columns. Composite membership does not imply individual uniqueness.
Partial/expression/prefix/invalid indexes are excluded from derived unconditional
named uniqueness; the complete raw index evidence is still available. Incomplete
metadata produces unknown uniqueness/key summaries. Generated expression,
identity and auto-increment are separate traits. PostgreSQL identity mode
(`BY DEFAULT` versus `ALWAYS`) and generated storage kind are retained, so a mode
change is visible in drift. MySQL `DEFAULT_GENERATED` does
not classify a column as computed. These are structural observations, not proof
of write permission or a successful live insert.

The PostgreSQL catalog preserves composite FK mappings under SELECT-only accounts.
For MySQL view definitions the account also needs the read-only metadata privilege
[`SHOW VIEW`](https://dev.mysql.com/doc/refman/8.4/en/information-schema-views-table.html).
The tested MySQL 8.4 server returned an empty definition string without it; empty,
whitespace and NULL definitions are marked unavailable with a diagnostic and
`complete: false`. Empty definition hashes are not presented as real definitions.
Other grants can also restrict catalog visibility; absence is not proof that an
object does not exist.

`db_relations` keeps declared and inferred information in separate arrays:
`items` is the backward-compatible declared list, also named `declared`;
`inferred` contains only frozen naming hypotheses. `total`/`truncated` refer to
declared records, while `total_inferred`/`inferred_truncated` refer to hypotheses.
Each array has its own request `limit` (1–200). `relation_kind: "all"` is the
default; `declared` or `inferred` hides the other array without mixing them.
Filters select source schema/table, not target objects.

Declared records use `kind: foreign_key` or `view_dependency`,
`provenance: declared` and confidence kind `catalog_declaration`. Naming records
use `kind: name_inference`, `provenance: inferred` and an explicitly **uncalibrated
heuristic score**, at most 0.4. A score is not an empirical probability, an FK
constraint, proof of referential integrity or permission to delete/change a table.
The response contains the source/target identifiers and types, name rule, a native
single-column unique-key observation, inference version and unverified categories.
`db_table` also includes that source table's captured `inferred_relations`.

Naming inference recognizes `user_id`, `UserID`, `User_ID`, role-prefixed
`AuthorUserID`, compact lowercase `userid`, and ordinary singular/plural forms,
including `company` → `companies`. PostgreSQL folds unquoted CamelCase identifiers;
quoted names retain the boundaries used by the CamelCase rule. A lost role boundary
in fully lowercased metadata is not reconstructed speculatively.

Resolution is within the source's captured schema and base tables. Same-name case
collisions or multiple full-name/role-tail candidates produce ambiguity diagnostics,
never first-match selection. An actual target `id`/`ID` must be unambiguous,
individually unique and backed by complete, valid native index metadata. Composite,
partial, expression and prefix indexes do not establish this single-column key.
Recognized scalar/enum identifier families must match (integer width/signedness,
UUID, textual/numeric attributes and known collation identity). Complete domains
can resolve to these base families, retaining their domain chain and explicitly
unevaluated constraints. PostgreSQL enums match the same qualified type; MySQL
enums require the same ordered labels/charset/collation. SET and arrays are not
identifier-key candidates. Unknown custom types remain unsupported. Already declared source FK columns are not inferred
again. Views and self-links are not inferred. Bare `id` and plain words such as
`paid` do not become suffix matches. These are candidate-generation rules, not
full language morphology, SQL type coercion or runtime search-path resolution.

Rejected candidates retain bounded diagnostic reasons, including ambiguous names,
missing/compound keys and type mismatch/unknown types. Incomplete table catalogs
withhold inference because an unseen name could change resolution. At most 1,000
hypotheses and 200 diagnostic records are frozen per capture; counts and analysis/
diagnostic truncation are explicit. This bounds the analysis, not evidence that
all physical or runtime relationships have been found. The hypothesis set is an
explicit supported subset; broader relationship recall requires further labelled
qualification.

Unqualified ambiguous tables require an explicit schema. Filters are exact names,
not SQL. `db_table` does not return row samples implicitly.

Captures are capped at 200 tables globally, 256 columns/FK mappings per table,
512 index-key/constraint-column records, 256 constraints/dependencies per object,
2,048 characters per default/generation expression, 8,192 per index/constraint
text, 16,384 per view definition and 8 MiB per serialized snapshot. SHA-256 digests
retain changes beyond the displayed index/constraint/view text prefix; truncation
still marks collection incomplete. Limit/permission diagnostics prevent a claim
of completeness. Completeness means **within metadata visible to the configured
role, selected schemas and supported collected categories**, not all physical
objects or all server metadata. Empty results do not prove a database is empty.
Query result limits are 1–200. Pagination beyond the capture cap is not available.

The SQL catalog now uses `capture_format: 4` and inference algorithm 2.
Formats 1/2 lack frozen analysis and report unavailable inference until refresh;
format 3 retains its original frozen algorithm/results. Historical facts and
hypotheses are never reanalysed or rewritten on a read. Format 4 requires the new
declared-type catalog in addition to structural metadata and frozen analysis.
Unknown future formats or structurally incomplete new-format payloads are rejected
without deleting them. The local database schema remains version 24; this is a
version of the JSON capture payload, not a DDL migration.

## Declared enum, domain and array types

`db_table.items[].metadata.types` contains type definitions used by that table.
PostgreSQL names are exact `(schema, name)` pairs; base domains and array elements
are followed, including definitions in a referenced type's own schema. OIDs are
not stored or compared. Enum labels retain logical order, not internal sort
numbers or OIDs. Domain definitions retain their base type/modifier, NOT NULL,
default SQL/hash, CHECK text/hash/validation state and collation identity. Arrays
retain element references and declared dimensions; basic built-in range metadata
retains subtype, canonical/difference function names and related type names.
Function implementations, composite/custom type bodies remain explicitly outside
this catalog's collected categories or mark their definitions incomplete.

Columns expose `resolved_type` and PostgreSQL `effective_default_sql`. Domain
chains retain whether a constraint applies to the value or an array element.
A NOT NULL/default on an element domain does **not** make an array column NOT NULL
or give the array that element default. Domain CHECKs are captured declarations,
not evaluated conditions; type-based naming hypotheses still do not prove valid
rows or a possible live write. Native column nullability and the effective domain
observations are schema evidence, not runtime data validation.

MySQL enum/set definitions are column scoped. Ordered labels, charset and collation
are retained as structured data; member lists are not split on commas. The
collector parses canonical `COLUMN_TYPE` metadata, independently of the reader's
SQL mode. This matches the native MySQL renderer
([Field_enum/Field_set](https://github.com/mysql/mysql-server/blob/mysql-8.4.0/sql/field.cc))
and is tested with normal and NO_BACKSLASH_ESCAPES creation/read sessions. Quotes,
commas, empty strings, backslashes and Unicode parser cases remain distinct.
Unsupported nontext/hex metadata literals are diagnosed rather than fabricated.
Enum/set column type names are normalized separately from their members so mode
rendering does not masquerade as a schema change.

Bounds are explicit: 256 used definitions/columns per table, 16 recursive type
edges, 512 enum/set members, 256 domain constraints, 2,048 displayed default
characters, 8,192 constraint characters and 65,536 MySQL type-text characters.
Truncation, missing definitions and unsupported bodies mark collection incomplete;
constraint/default hashes preserve longer-text evidence. Each metadata query is
streamed with an 8 MiB received-text budget; table captures are checked against
the 8 MiB snapshot budget incrementally. A budget failure retains the previous
published capture. These are bounded observations, not exhaustive collection of
all server types.

`db_drift` compares table kinds, columns, declared FKs, indexes, constraints, view
options, definitions, direct relation dependencies and declared type catalogs to the previous capture or
`base_snapshot_id`. No baseline returns `compared: false`. Different target/scope
or incompatible structural formats also return `compared: false`: first-time
richer collection must not be presented as a database change. Formats 2 and 3
share structural format 2 and can be compared: adding a frozen hypothesis cache
is not a physical schema change. The response explicitly reports
`inference_compared: false`; hypotheses, scores and algorithm versions do not
participate in SQL drift. Format 4 uses structural format 3 and is not compared
as physical DDL against older formats lacking the type catalog. Two comparable
new-format captures
can detect index/CHECK/view changes without any column change. Row samples,
statistics/OIDs and capture timestamps do not participate in schema comparison.
Removed objects are described as removed **from visible scope**, since grants can
also affect visibility. This is a structural metadata comparison, not semantic
SQL equivalence or proof of the exact migration applied. Drift covers the entire
capture; `schema`/`table` filters are rejected.

`db_cross` means **code ↔ database**, not cross-server SQL joins. It compares the
current indexed static SQL usages and migration table/column names in the
configured project with captured metadata. It returns source evidence, matched/
ambiguous/unobserved tables, migration columns not observed live, and live tables
without observed SQL use. The latter always carry `deletion_safe: false`. It
reports the current code revision separately from the revision at remote capture,
stale/unindexed/parse diagnostics and dynamic SQL/ORM limitations. Unqualified
names do not infer a runtime search path. SQL-standard name folding from the
source index is not full dialect-specific identifier resolution. Column comparison
checks names, not type compatibility. Filters are rejected; comparison covers the
entire configured project and captured scope.

## Optional samples

Samples require all three:

1. `samples_enabled: true` in owner configuration.
2. Explicit schema → table → column allowlists, e.g.
   `"sample_columns": {"public": {"orders": ["id", "status"]}}`.
3. `"include_samples": true` in an administrative `db_refresh` request.

Ordinary refresh does not query table rows. The dedicated `db_sample` reads only
previously captured samples; it cannot enable sampling. Historical samples can
still be read by authorized readers after the owner later disables collection.
Old snapshots without samples do not inherit samples from later captures.

Only complete base tables are sampled; views and binary/unknown user types are
excluded. Explicitly allowlisted known enums, SETs, scalar domains and supported
arrays can be displayed; this does not enable sampling by itself. At most 16 allowlisted columns, 5 unordered rows per table, 256 display
characters per cell, 16 KiB per encoded sample and 64 KiB total samples per refresh
are retained. SQL reads only selected metadata-verified columns. Sensitive column
name heuristics override the allowlist, and email-like/token-like values are
redacted. NULL, empty text, redacted and truncated cells remain distinct.
These heuristics are **not** proof of anonymity; the owner allowlist is the main
control. Samples are partial unordered display strings, not distributions, typed
write plans or complete domains. They are stored in the local snapshot and are
available to readers authorized to use the database tools.

## Current limits and verification

Broader inferred-relation recall/type coverage, composite/custom type bodies,
function dependency metadata, sequence parameters, ORM mappings and migration/live type coercion
comparison remain open. Snapshot portability/export and representative scale/
performance/superiority qualification remain open. No production database or
running RAG server is configured or restarted by this change.

`python3 scripts/smoke_external_db.py [report.json]` builds the actual binary,
creates uniquely named disposable PostgreSQL 16/MySQL 8.4 containers with random
credentials and accounts with SELECT-only data access (plus SHOW VIEW metadata
permission for the full MySQL view fixture), and removes only its own containers/volumes.
It tests native captures/read-only enforcement, composite FKs, ordinary/functional/
partial/included/prefix indexes, CHECK validation/enforcement, view dependencies,
materialized views, missing SHOW VIEW, composite uniqueness, capture format/history,
quoted names, samples, drift and code comparisons, then
actual stdio/HTTP operations and role enforcement. Real fixture DDL changes an
index, CHECK and view and is observed through MCP without column changes. A
separate PostgreSQL identity-mode alteration is also detected. Naming hypotheses
are verified separately from declared FKs, including frozen-history reads, mode
filters, per-array limits and key/type rejection diagnostics. Separate real DDL
steps exercise enum label, SET member and domain CHECK/default changes. Known-type
row samples still require the same owner allowlists and explicit refresh flag. It stops its own MySQL fixture
to verify failed refresh preserves the last good capture and offline queries work.
It also checks that default verified TLS rejects the untrusted local fixtures;
it does not establish successful trusted-CA TLS interoperability or other server
versions/platforms. Unit tests run with `cargo test --lib external_db::`; the
fixture test is ignored without explicit fixture setup.
