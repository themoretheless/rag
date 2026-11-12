# PHP name candidates qualification — 2026-10-26

Implemented [contract](../../PHP_NAMESPACE_BINDINGS.md), extending the pinned
RAGRAF comparison (`075caf86dc2f6274b9c48487e521915bcdfeaf2c`) with conservative
namespace/import tables and declared-method candidates rather than a namespace-wide
binding rejection.

- Ten focused native tests pass: import domains/groups/reopened scopes, case and
  absolute/relative names, fallback isolation, visibility/static guards, callable
  values, conditional/ambiguous declarations, source limits, legacy-cache repair
  and immutable import-only graph changes.
- Eight real stdio MCP scripts pass on one copied binary: PHP names, PHPDoc,
  PHP routes, SQL table impact, literal impact, non-code references, translation
  keys and source cursors. Reports and logs are retained here.
- SQL table impact passes all 13 scenarios, including separate readers/writers,
  ownerless SQL, indirect callers, independent limits, project/schema guards,
  warm reuse, stale exclusion and sync refresh.
- Workspace: 1,538 passed, zero failed, one ignored. Clippy with `-D warnings`, and debug binary build are recorded
  in the corresponding logs. The first workspace run exposed an obsolete policy
  test expecting namespace-wide rejection; the updated regression preserves
  policy evidence while checking the new candidate binding.

Runtime: index format 72, store schema 25. `verification.json` records the copied
binary and selected source hashes. This is a shared checkout with concurrent work;
hashes are observations, not proof of a frozen checkout or deployed gateway.
The isolated fixtures use temporary DuckDB stores and mock embeddings. PHP/SQL
applications and production databases are not executed. PHP CLI is unavailable;
compiler acceptance, runtime dispatch/load order, representative quality and full
RAGRAF parity remain unverified.
