# Inherited declaration candidates — 2026-10-25

[Contract](../../METHOD_RELATIONS.md). RAGRAF's inspected graph/query relation
allowlists at pinned 075caf86dc2f6274b9c48487e521915bcdfeaf2c include overrides.
The local implementation previously resolved inherited calls but persisted no
separate method declaration relation. This milestone adds those candidates with
explicit limits and immutable-history coverage. It is not a runtime dispatch or
reference accuracy/throughput superiority claim.

Four new inheritance regressions cover nearest declarations in Python/JS/TS
without calls, C3 diamond order and blocked/unknown/cyclic bases, imported aliases
with independent pagination/lexical semantics, and exact document/project identity
when paths collide. One new history regression checks frozen removal and the
legacy missing-coverage contract. The focused suites also run existing inheritance
and history cases.

The isolated stdio MCP scenario checks imported methods, bounded relation pages,
provenance, class/method navigation, no synthetic calls, foreign-project exclusion,
lexical separation, warm reuse, stale omission, source-sync refresh and frozen
removal diffs independent of later filesystem text. Mock providers and temporary
sources/databases are used. No source application or production database runs.

Format 29; database schema 25. Signature/decorator/visibility/private/static rules,
compiler override semantics, runtime and remaining language declarations are
unverified. Full parity remains open. Final measured gates and source hashes are
in verification.json; actual MCP evidence is runtime.json.

Final gates: **1,228 workspace tests passed, 0 failed, 1 ignored**. All seven
inheritance and eight history focused tests passed (five new regressions total).
Clippy with `-D warnings`, formatting and `git diff --check` passed. Rebuilt binary
passed the new method-relation scenario and the existing live-hunk/SQL-output
MCP regressions. Source and binary hashes plus build/test logs are retained here.
These are controlled fixture results, not representative accuracy or scale proof.
