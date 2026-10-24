# Offline CHECK validation for migration schemas — 2026-10-24

Migration write_check now shares the pure bounded CHECK interpreter with captured external-schema validation. Named column/table declarations are parsed locally; supported expressions use supplied literal JSON values. Explicit SQL NULL follows CHECK truth semantics, while NOT NULL remains separate. Omitted nullable insert values become NULL only when there is no default/generated/unknown assignment. Default/generated expressions and missing update-row context are not fabricated.

Unknown functions/syntax/context and exhausted budgets produce unverified records and indeterminate verdicts. NOT VALID constraints still check new literal rows; explicit NOT ENFORCED is reported as skipped. Column-only plans preserve their narrower validation scope; deletes do not evaluate new-row predicates. Schema completeness is reported independently of row-context uncertainty. Values are never echoed.

The shared interpreter also fixes PostgreSQL/standard unquoted identifier folding while preserving case-sensitive quoted names. A regression covers distinct legal columns a and "A".

Verification:

- Full library run after implementation: 827 passed, 0 failed, 1 ignored.
- Final SQL targeted rerun after separating schema/row uncertainty: see verification.json.
- New regressions cover accepted/rejected rows, SQL NULL, unknown assignments/defaults/functions, partial updates, quoted identifiers, NOT VALID, 512-predicate/node/row budgets and literal non-disclosure.
- Server and gate CLI builds passed.
- Actual stdio MCP workflow: scripts/smoke_migration_checks.py, runtime.json.
- Existing captured-constraint/diff/write-check workflow: constraints-regression.json.

All stores and inputs are temporary and embeddings are mocked. Migration SQL, source scripts, arbitrary SQL functions and live database writes are never executed. Keys/foreign-key data, type coercions, unsupported CHECK forms, generated/default expression results and live state remain unverified. This does not complete the full transfer goal.
