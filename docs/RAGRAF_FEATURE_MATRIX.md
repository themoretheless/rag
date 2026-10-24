# RAGRAF transfer and improvement acceptance matrix

Goal: all reference features transferred and improved. This goal is **not yet
achieved**. A local tool name or passing unit test is not full parity evidence.

Reference inspected: `paulmann/RAGRAF`, commit
`075caf86dc2f6274b9c48487e521915bcdfeaf2c`. The published facade at that revision
contains 39 `op_*` functions (including `task_set`); prose counts elsewhere may
lag. Recheck upstream changes before the final audit and record whether they are
part of the original reference scope or newly added upstream features.

| Reference operation | Local entrypoint | Current status / remaining acceptance work |
|---|---|---|
| search | search | Hybrid retrieval exists; qualify code navigation evidence/ranking against reference tasks. |
| search_multi | search_multi | Literal/regex all/any over whole documents; independent per-query windows/counts/truncation and project-isolated filename families. Fresh static entrypoint/call/value-reference evidence ranks retained families; runtime/markup usage and scale benchmark remain. File ingestion captures BOM/strict UTF-8 or explicit heuristic Windows-1251/1252 decoder evidence and byte hashes; other encodings and detection ambiguity remain. |
| duplicates | duplicates | Exact/near text groups and static call/entrypoint evidence; markup/import/dynamic file liveness remains partial. |
| symbol | symbol | Definition evidence for Rust/Python/JS/TS/Go/Java/C/C++/Bash/sh and basic PHP declarations; remaining language coverage and contract suite required. |
| references | references | Bound imports/aliases, lexical scopes, annotated receivers/inherited member values, callbacks and type uses; explicit lexical mode. Static ambiguity/staleness preserved. Remaining languages, compiler namespaces, dynamic alias chains, explicit override links and reference non-code relation kinds remain incomplete. |
| callers | callers | Static candidate links with import/type/inheritance support; Go package/own-module/receiver binding and Java package/import/receiver/arity/hierarchy candidates added, including interface contracts. Remaining languages/compiler/runtime cases unqualified. |
| callees | callees | Same static-analysis qualification limits as callers. |
| impact | impact | Bounded candidate-call traversal; full reference proven/heuristic relation coverage remains. |
| slice | slice | Structural call subgraph; not a full data-flow/program slice or optional CPG backend. |
| code | code | Explicit fresh symbol body reads; language/overload coverage needs qualification. |
| repo_map | repo_map | Budgeted symbol map; compare reference ordering/compression with representative repositories. |
| entrypoints | entrypoints | Conventional/export/test/route candidates; remaining extractors/framework patterns unqualified. |
| dead_code | dead_code | Conservative reachability candidates, no deletion claim; broader reference extraction remains. |
| routes | routes | Initial Python/Express/Axum patterns; framework/language expansion remains. |
| schema | schema | SQL DDL replay subset, explicit default/generated expressions and captured table/column constraints; ADD/DROP CONSTRAINT metadata. ORM migration DSLs, ordering, dependency rewrites and full dialect/constraint validation remain. |
| table_usage | table_usage | Static SQL strings/SQL files and basic DML; indirect ORM/dynamic SQL and some dialect targets remain. |
| write_check | write_check | Migration schema or explicit frozen native capture; required/null/generated/identity/default assignments, VALUES counts, supported literals, PG enum/domain and shared bounded offline CHECK evaluation (including migration literal rows, typed scalar default context and explicit DEFAULT assignments on insert/update). Unknown expressions/coercions stay indeterminate; live constraints/data/permissions are unverified. |
| db_schema | db_schema / db_refresh | Native PostgreSQL/MySQL read-only snapshots with explicit role/schema scope; enum/domain/set and used-type chains captured; composite/custom bodies, functions and broad server/scale qualification remain. |
| db_table | db_table | Captured columns, indexes, CHECKs, views/materialized views and conservative composite/individual key traits; domain chains/defaults/CHECKs and ordered enum/set labels; composite/custom bodies and fuller reference detail remain. |
| db_relations | db_relations | Declared composite FK/view links plus separately frozen key/type-evidenced naming hypotheses, with ambiguity diagnostics; algorithm 2 resolves known scalar domains and matching enum IDs; broader recall, custom types, functions and dependency coverage remain. |
| db_sample | db_sample | Explicit owner allowlist plus administrative refresh flag, bounded stored samples with heuristic redaction; domains/distributions and broader privacy/scale qualification remain. |
| db_drift | db_drift | Versioned immutable snapshots and scope-checked column/FK/index/CHECK/view diffs; broader failure/DDL concurrency and semantic equivalence qualification remain. |
| db_cross | db_cross | Code ↔ database static SQL/migration-name comparison with evidence and ambiguity; ORM/type/dialect resolution and full comparative qualification remain. |
| policy_trace | policy_trace | AST boolean/computed decisions, syntactic fallback, static callers for Rust/Python/JS/TS/PHP; full control-flow, PHP binding/framework and runtime role qualification remain. |
| class_usage | class_usage | CSS/HTML/JSX/DOM static class evidence, isolated project groups, missing/unused observations; CSS imports/modules, dynamic/framework/preprocessor coverage and benchmark qualification remain. |
| freshness | freshness | Allowlisted bounded byte reads use the ingestion decoder (BOM UTF-8/UTF-16, strict UTF-8, heuristic Windows-1251/1252); decoded checksums and separate observed-byte evidence, explicit stored mode and raw-byte gate budgets. More platform/encoding/scale qualification remains. |
| index_status | index_status | Static coverage/revision counts; unified versioned capability/quality envelope remains. |
| catalog | catalog | Immutable source manifest; complete corpus-registry/integration contract and pagination refinements remain. |
| diff | diff | Frozen file/definition/candidate-call changes plus non-call candidates, inheritance and symbol-entrypoint sets. Format 18 freezes derived SQL table/column metadata for schema diff; old missing snapshots are explicit. Format 19 captures explicit default/generated expressions and clauses with legacy-coverage flags. Format 20 adds captured constraint sets/column constraints with legacy-coverage flags. Constraint semantics, overload identities, unresolved relation kinds and regression interpretation remain. |
| live_read | live_read | Bounded live range plus checksum/changed-range summary; richer edit hunks/encoding qualification remain. |
| drift_report | drift_report | Persistent source observations with stable pagination; exhaustiveness is not claimed. |
| drift_ack | drift_ack | Atomic, idempotent acknowledgement with separate write permission; integration qualification remains. |
| note | note | Separate append-only unverified task notes with author/revision context; portable export/replication and broad reference contract qualification remain. |
| notes | notes | Literal filtering, JSON tags, stable sequence pagination and explicit task/all scopes; cross-surface qualification remains. |
| gate | gate / rag_gate | Shared bounded index-usage rules, real current-revision usage evidence, durable task decisions and opt-in HTTP/offline hook; client activation/platform/scale qualification remains. |
| task_set | task_set | Durable session labels with TTL/generation CAS; explicit HTTP scopes avoid cross-client attribution; broader CLI/hook integration remains. |
| task_stats | task_stats | Durable measured MCP index-call outcomes and notes, explicit unfinished/unmeasured records; gate effectiveness, CLI/REST attribution and quality/savings measures remain. |
| calibrate | calibrate / calibration_reports | Labelled training/holdout threshold reports, explicit unvalidated parameter assertions and stable report history; runtime application, representative labels and comparative quality qualification remain. |

Additional reference surfaces requiring audit: MCP tools/resources/templates and
prompts, CLI query operations, admin diagnostics/configuration/integration flows,
optional Java/Joern capability and its explicit degraded behavior, 16 tree-parsed
languages plus text-only formats, provenance/confidence and index-revision
contracts, installers/platform behavior and published quality/performance gates.
New wrappers must not silently overwrite similarly named existing wiki tools.

## Evidence and improvement gates

Current implementation evidence is in `src/code_index/`, corresponding module
unit tests, source-sync tests and the MCP access/router tests. Documentation:
[RAGRAF_PARITY.md](RAGRAF_PARITY.md), [SQL_CODE_ANALYSIS.md](SQL_CODE_ANALYSIS.md),
[CODE_HISTORY.md](CODE_HISTORY.md), [EXTERNAL_DATABASES.md](EXTERNAL_DATABASES.md), [CSS_POLICY_ANALYSIS.md](CSS_POLICY_ANALYSIS.md), [ANALYSIS_TASKS.md](ANALYSIS_TASKS.md), [GATE_CALIBRATION.md](GATE_CALIBRATION.md). Test totals alone do not close rows above. The CSS/policy subset has a reproducible
[local qualification report](qualification/css-policy-2026-10-24/README.md), including
real stdio MCP evidence; broader comparison and integration gates remain open.

Before claiming the goal complete:

1. Map every reference tool/operation/resource/prompt/CLI contract to an owning
   implementation and requirement-specific test or runtime artifact.
2. Run equivalent reference/local tasks with golden expectations for each parsed
   language and relation, including ambiguity, stale data and degraded capability.
3. Prove the improvements on representative datasets: retrieval correctness,
   candidate precision/recall, source/citation fidelity, cold/warm latency, memory,
   indexing/refresh throughput, context size and failure/recovery behavior.
4. Verify immutable history, scopes, read/write/admin permissions, path policies,
   pagination/budgets, cancellation and crash/failure atomicity end to end.
5. Build and exercise the actual CLI/MCP/HTTP/native surfaces required by the
   implementation; do not promote library tests to runtime/deployment proof.
6. Audit current sources and actual external state again. Any missing, partial or
   unverified requirement keeps the original full goal active.

Task/notes and durable MCP attribution have a [local qualification report](qualification/analysis-tasks-2026-10-24/README.md) with actual stdio/HTTP role and session-isolation evidence. Gate/calibration and broader surface/scale acceptance remain open.

The reproduced `fts_main_chunks` overwrite/WAL failure is fixed for new rebuilds:
leaf-first owned-catalog teardown, persistent dirty generation, creation without
extension overwrite, and bounded signal shutdown. [Recovery qualification](qualification/fts-recovery-2026-10-24/README.md)
includes seven rebuild crash boundaries and twelve actual process/signal cases.
This closes that specific regression, not all platform/hardware failure cases;
automatic salvage of already-malformed legacy WAL files is not implemented.

Gate/calibration subset: [qualification report](qualification/gate-calibration-2026-10-24/README.md), actual stdio/HTTP/hook evidence and a concrete still-open FTS WAL recovery failure. These do not close whole-product acceptance.

External PostgreSQL/MySQL subset: [local qualification](qualification/external-db-2026-10-24/README.md), native SELECT-only fixtures and real stdio/HTTP role/failure evidence. All six reference operation names now have local implementations, with the partial coverage listed above; this is not whole-feature or superiority acceptance.

Indexes/constraints/views extension: [local qualification](qualification/external-metadata-2026-10-24/README.md), including actual index/CHECK/view DDL observed through MCP, capture-format compatibility and a controlled composite-vs-individual key contract comparison. This closes the earlier wholly missing structural categories for the tested subset, not their broader server/scale/reference acceptance.

Inferred database relations: [qualification](qualification/inferred-relations-2026-10-24/README.md), separate frozen guesses with versioned evidence, native/MCP tests and controlled reference-rule cases. Broader relationship recall/type coverage, calibration and representative precision/recall remain open; this is not full-goal acceptance.

Declared types extension: [qualification](qualification/declared-types-2026-10-24/README.md), native enum/domain/set catalogs, independent real type-DDL steps through MCP, mode-safe literal decoding, type-aware frozen hypotheses and streaming budgets. Composite/custom implementations, broader types/versions/scale and full-goal comparative acceptance remain open.


Bound reference extension: [qualification](qualification/bound-references-2026-10-24/README.md),
source occurrence extraction, explicit lexical fallback, frozen revision evidence,
callback transitive reachability and actual MCP stale/sync checks. This improves
symbol-specific evidence for the qualified subset; broad compiler/language,
non-code references and representative precision/recall acceptance remain open.


Go extension: [qualification](qualification/go-index-2026-10-24/README.md),
functions/receiver methods/interfaces/types, source-scoped own-module imports,
package file binding, shadowing, test/build isolation and actual MCP module-only
change/staleness/reuse checks. Embedded/generic/runtime dispatch, external module
selection and remaining languages are still incomplete; full-goal acceptance is
not established.


Java extension: [qualification](qualification/java-index-2026-10-24/README.md),
source declarations/constructors, package/import/type/arity candidates,
inheritance and method/constructor reference evidence. Actual MCP staleness,
source-sync hierarchy rebinding and Go regression checks accompany unit fixtures.
Compiler/classpath/module/argument-conversion and dynamic dispatch semantics,
framework/data-flow extraction and the remaining languages are still incomplete.


C/C++ extension: [qualification](qualification/c-family-index-2026-10-24/README.md),
declarators, source prototypes/bodies, bounded captured includes, static boundaries,
namespace/typed member/base candidates and raw SQL evidence. Actual MCP and Clang
syntax-only probes cover the qualified subset. Preprocessing/build/ABI, complete
C++ semantic lookup and the broader quality/language gates remain open.

Shell subset qualified on 2026-10-24: `.sh`/`.bash` definitions, ordered
registrations, captured absolute source dependencies, subprocess isolation and
exported function references. Relative CWD, expansions and runtime execution
remain unresolved. See [shell qualification](qualification/shell-index-2026-10-24/README.md).
