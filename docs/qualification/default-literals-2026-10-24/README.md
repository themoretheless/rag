# Typed scalar insert defaults in offline checks — 2026-10-24

CHECK evaluator version 2 constructs literal-row context from omitted scalar insert defaults. It decodes NULL, booleans, text and signed integers using the SQL AST without running expressions. Literal values are admitted only for supported compatible column types: TEXT, BOOLEAN/BOOL, SMALLINT, INT/INTEGER and BIGINT (NULL is representation-independent). Numeric ranges are checked before using a default as an integer value.

Parentheses/unary numeric signs are accepted; casts, arithmetic, functions, ambiguous coercions and unsupported types remain unverified. Known NOT NULL DEFAULT NULL is rejected for literal insert plans. Explicit values override defaults; assigned columns without literal values stay unknown, and update plans never acquire insert defaults. Inferred_default_columns returns names only; default literals are not echoed.

Context construction and CHECK interpretation share 4096 nodes and a 64 KiB literal-value context budget. Exhaustion yields indeterminate instead of a partial positive verdict. Captured schema format stays 20 because source facts are unchanged; evaluator_version identifies the interpretation change.

Verification:

- Full library suite: 830 passed, 0 failed, 1 ignored.
- New regressions cover scalar/sign/NULL parsing, rejected function/cast/arithmetic input, integer compatibility, NOT NULL DEFAULT NULL, unknown assignments, update scope, context budget and default-value non-disclosure.
- Server and gate CLI builds passed.
- Actual stdio MCP default/check/source-sync workflow: runtime.json (scripts/smoke_default_literals.py).
- Existing migration CHECK regression: checks-regression.json.

All fixtures/databases are temporary and embeddings are mocked. Migration SQL, arbitrary functions and live writes are never executed. No input/default values are returned by write_check. Generated values, implicit server defaults, general constant folding, coercions, unsupported types and live state remain outside this qualification. The full transfer goal remains open.
