# Native indexes, constraints and views: local qualification, 2026-10-24

The external PostgreSQL/MySQL snapshot block now includes structural indexes,
constraints, view definitions and direct view-relation dependencies. The full
RAGRAF transfer/improvement objective remains open. See the
[contract](../../EXTERNAL_DATABASES.md), [matrix](../../RAGRAF_FEATURE_MATRIX.md)
and [earlier external DB qualification](../external-db-2026-10-24/README.md).

## Verified behavior

`cargo test --lib`: **751 passed, 0 failed, 1 ignored**. The external fixture test
is intentionally ignored without provisioning; it was separately executed by
`scripts/smoke_external_db.py` against uniquely owned PostgreSQL 16 and MySQL 8.4
Docker containers. The final expanded fixture run and actual stdio/HTTP MCP
exercise passed. Only temporary fixture data and SELECT/read-only transactions
were used; all owned containers/volumes were removed. No existing RAG process or
production database was configured, opened for this test, or restarted.

The final native fixture checks:

- Composite primary/unique keys retain actual key order. A `b DESC, a` index is
  returned in that order; derived individual uniqueness is not assigned to either
  column merely because they participate in `UNIQUE(a,b)`.
- PostgreSQL functional and partial index expression/predicate, INCLUDE membership,
  NULLS NOT DISTINCT and initially deferred unique constraints are preserved.
- MySQL functional expressions, 12-character prefix length and invisible-index
  status are preserved. Unsupported/inapplicable validity flags remain unknown,
  rather than being invented as successful validation.
- PostgreSQL `CHECK ... NOT VALID`, exclusion constraints and MySQL
  `CHECK ... NOT ENFORCED` retain distinct validation/enforcement metadata.
- PostgreSQL identity (including BY DEFAULT versus ALWAYS), MySQL auto-increment
  and stored computed expressions remain
  separate. A real MySQL `DEFAULT_GENERATED` timestamp remains noncomputed.
- PostgreSQL ordinary/materialized views expose native definitions, options,
  SHA-256 hashes and columns, including a materialized view created WITH NO DATA.
  A nested view names its direct view dependency instead of falsely linking only
  to all transitive base tables. Both dialects return relation dependencies.
- A MySQL account with SELECT but no SHOW VIEW returned blank view definitions.
  The collector normalized these to unavailable (`definition/hash: null`), emitted
  `VIEW_DEFINITION_UNAVAILABLE`, and marked the view/table/capture incomplete,
  while retaining available base-table metadata. The full MySQL metadata account
  has SELECT plus SHOW VIEW, with no data-write privilege.
- Old payloads with no capture-format tag remain readable as format 1. Comparing
  them to format 2 returns `compared: false / capture_format_changed`, not fake
  index additions. Unknown future formats and structurally incomplete new-format
  payloads are rejected without overwriting them.
- Snapshot reopen/order, source-to-database evidence, bounded opt-in samples,
  actual read-only INSERT rejection even using admin credentials, and rejection
  of untrusted TLS fixtures remain covered by the expanded native test.

Actual MCP exercises call all seven operations and enforce HTTP Read/Write/Admin
roles. The script performs real fixture DDL: add an index, change a CHECK and
replace a view without changing column definitions. After `db_refresh`,
`db_drift` reports exactly `child` and `child_view`, with the new index and changed
view definition hash. Row/sample-only refreshes produce no structural changes.
A separate actual PostgreSQL `ALTER COLUMN id SET GENERATED ALWAYS` is reported
as exactly one changed table, retaining BY DEFAULT before and ALWAYS after.
Stopping the owned MySQL server still leaves the last good snapshot readable and
failed refresh does not publish an empty replacement.

Machine-readable results: [runtime.json](runtime.json),
[verification.json](verification.json), [source-sha256.json](source-sha256.json).

## Narrow reference comparison

[key-traits-reference.json](key-traits-reference.json) records one controlled
comparison with RAGRAF commit `075caf86dc2f6274b9c48487e521915bcdfeaf2c`.
The unchanged reference `db_ops.schema` function was executed with controlled
metadata rows and the actual reference `db_marks` module. Given `UNIQUE(a,b)`
and `UNIQUE("a\"b")`, its `unique_columns` contains all participating columns.
The local `individually_unique_columns` explicitly contains only the single-column
key, verified by both native server fixtures and real MCP calls. The local response
also retains the ordered composite key separately. This demonstrates a narrower,
clearer column-trait contract; it is not a whole-product quality/performance claim
or an assertion that the complete reference server was executed.

## Remaining scope

Coverage is limited to these local server versions and fixtures. Full enum/domain
and function dependencies, sequence parameters, derived naming relations, broader dialects/versions,
all privilege/DDL concurrency behavior, high-cardinality budget pagination,
trusted-CA TLS interoperability, export/replication, scale/performance and broad
reference golden comparisons remain open. Text hashes detect changes beyond the
displayed prefix; truncation still marks collection incomplete. Definitions may
contain confidential schema literals; samples remain opt-in. Constraint capture
is not evaluation of proposed rows or proof of live write success.
