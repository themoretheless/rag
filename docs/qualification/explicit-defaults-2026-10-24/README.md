# Explicit DEFAULT assignment plans for migration schemas — 2026-10-24

CHECK evaluator version 3 accepts default_columns in migration mode without an external connection. DEFAULT targets enter the normalized assignment set and VALUES-count validation. Supported scalar defaults are used for explicit insert/update assignments, while omitted update columns remain unknown. A nullable column with no declared default has NULL default context; undeclared NOT NULL defaults, implicit server values and generated expressions remain unverified.

A literal and DEFAULT for the same normalized target are rejected. Duplicate DEFAULT targets and delete assignments are invalid. Explicit_default_columns and inferred_default_columns report column names only. An unknown implicit value is not reported as inferred.

Verification:

- Full library suite after the main implementation: 832 passed, 0 failed, 1 ignored.
- New regressions cover insert/update DEFAULT, conflicts, duplicates, NULL/implicit/generated contexts and unknown assignments.
- Server and gate CLI builds passed.
- Actual stdio MCP workflow: runtime.json. It verifies inserts/updates, VALUES counts, mismatch rejection, nullable NULL defaults, unknown implicit/generated values, delete/conflict handling and refresh after ALTER DEFAULT.
- Existing scalar default regression: literal-regression.json.
- Existing migration CHECK regression: checks-regression.json.

All inputs/databases are owned temporary fixtures with mock embeddings. No migration SQL, functions or live writes are executed. Unsupported expressions/types, implicit server behavior and generated values remain unverified. Source snapshot format stays 20; interpretation is identified by check_evaluator_version=3. The full transfer goal remains open.
