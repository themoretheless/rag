# Non-code references — 2026-10-26

Implemented [SQL table and URL targets](../../NON_CODE_REFERENCES.md) in the
existing `references` MCP tool and native Store query. Pinned RAGRAF facade
`075caf86dc2f6274b9c48487e521915bcdfeaf2c` explicitly includes `queries` and
`routes` relation kinds; those contracts were inspected, not run as a complete
reference application. Translation keys and other remaining kinds stay open.

**1,445 workspace tests passed, 0 failed, 1 ignored**. Final focused five-case
suite, Clippy with `-D warnings`, owned-source formatting, Python syntax and
`git diff --check` passed. Initial four-case tests/Clippy/workspace preceded the
handler-alternative correction; `final-*` logs qualify the completed five-case
implementation. Native tests cover SQL owners/ownerless uses, schema-only targets,
independent result counts, schema/project ambiguity before occurrence filtering,
URL registration/client distinctions, handler alternatives as one occurrence,
legacy symbol behavior, stale exclusion and replacement.

The new actual MCP scenario verifies:

* SQL `queries` records, read/write/source filters and full counts before limits;
* project/schema ambiguity before source/access filters, quoted identifiers and
  known schema-only targets with no inferred negative runtime claim;
* separate registration/client relations and their distinct method evidence;
* one source-file occurrence with multiple separately anchored handler candidates;
* literal URL identity, body-free deterministic output and scoped symbol mode;
* protocol-level rejection of mixed targets, invalid SQL/modes and control URLs;
* exclusion of stale SQL/route facts and actual source-sync replacement.

Six additional MCP regressions passed on the same isolated executable: SQL table
impact, route queries, method relations, Perl references, PowerShell references
and file imports. All source/database fixtures are temporary and embeddings are
mocked. No ingested code or SQL runs; production data is untouched.

Runtime reports store schema **25**, extraction format **54**. The format advanced
in concurrent shared-tree work; this query over existing facts itself requires
no extraction-format bump. `verification.json` contains detailed reports and
executable/log/source/reference hashes. Source hashes after checks are observations,
not a frozen checkout reconstruction guarantee.

Physical database/router binding, nullable-project target selection, additional
non-code kinds (including translation keys), remaining CLI/HTTP surfaces,
representative precision/recall/scale and full RAGRAF parity remain unproven.
