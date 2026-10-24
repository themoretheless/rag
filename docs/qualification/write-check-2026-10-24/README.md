# Frozen schema write checks — 2026-10-24

This slice extends `write_check` beyond the existing source-migration mode to an
explicit local external-database capture. It does not complete all RAGRAF parity
or establish general superiority; see the feature matrix for remaining scope.

The public MCP request accepts literal values or the reference-compatible VALUES
count, exact schema selectors, historical capture IDs, DEFAULT assignments and
PostgreSQL identity override. Migration requests keep their existing meaning.
An ordinary NOT NULL primary key without a default remains required, unlike a
blanket primary-key exemption. Multiple independent errors are returned together.

The offline interpreter evaluates only a bounded subset of CHECK expressions.
Unsupported syntax, function calls, coercions, missing update-row values, custom
casts and unavailable defaults stay unverified. SQL null CHECK semantics are
separate from NOT NULL. PostgreSQL NOT VALID and MySQL NOT ENFORCED are treated
separately. No request values are echoed, and no SQL is sent to a server.

Qualification includes ordinary required keys, nullability, generated DEFAULT,
computed assignment rejection, identity ALWAYS/BY DEFAULT and historical capture
selection, VALUES count errors, PostgreSQL enum/domain checks, MySQL enum/SET
uncertainty, NOT VALID/NOT ENFORCED, unsupported arrays, SQL null logic, bounded
casts, unknown functions, request limits and checking after MySQL is stopped.
The actual MCP fixture additionally checks source migration extraction and read /
write table-use lookup immediately after source synchronization.

`runtime.json` records disposable PostgreSQL 16/MySQL 8.4 and stdio/HTTP checks.
`verification.json` records test totals and the executable digest;
`source-sha256.json` identifies the relevant saved source files. These are local
qualification results, not a deployed server update. Existing production stores
and servers were not opened or restarted. Containers owned by the fixture are
removed in its finalizer.

A positive check does not prove a live write succeeds: actual uniqueness/FK/data
conflicts, exclusions, permissions, triggers, generated expression results,
arbitrary SQL functions, full type coercion, character encodings, temporal ranges
and timezone semantics remain outside this interpreter. External capture format
4, structural format 3 and local schema version 24 remain unchanged.
