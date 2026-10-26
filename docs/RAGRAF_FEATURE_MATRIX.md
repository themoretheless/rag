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
| search_multi | search_multi | Literal/regex all/any over whole documents; independent per-query windows/counts/truncation and project-isolated filename families. Fresh static entrypoint/call/value/type, module-import and HTML/CSS/resource evidence now ranks retained families, including same-project referrers outside the target prefix and search matches. Scoped index decoding and corpus reads share one transaction; zero-hit queries skip role-index reads, and selected bodies load in 512-ID batches. A separate bounded literal-path phase now includes endpoint constants and other exact stored-text mentions with source anchors, project-wide basename disambiguation, weaker unbound roles, explicit budgets and typed-pair suppression. Global ranking, runtime/framework usage and representative quality/scale remain. File ingestion captures BOM/strict UTF-8 or explicit heuristic Windows-1251/1252 decoder evidence and byte hashes; other encodings and detection ambiguity remain. |
| duplicates | duplicates | Exact/near text groups now use contract-2 entrypoint, bound call/value/type, file import and HTML/CSS/resource evidence, with same-project identity/fingerprint guards and source evidence outside target prefixes, now observed with corpus/group data in one scoped read transaction. Published non-code targets, count-based ties, deterministic independent evidence caps and primary retention under member limits are qualified. Exact stored-text path mentions now supplement uncovered source-target pairs with weaker unbound evidence, project-wide basename guards, independent scan budgets and complete counts under 20-fact retention. Returned members now expose bounded normalized unique-line Jaccard against the primary, directional line deltas, exact stored-text equality and identity provenance; null metrics explicitly mark unchecked pairs. Grouping remains a separate metric. The explicit exact_and_name_families audit view now retains independent exact partitions and directory/case/project-scoped numbered sibling families, including dissimilar same_name groups; overlapping proper subgroups, identical-set suppression and budget-limited not_checked classification are qualified. Qualified captured source-byte digest/count partitions now provide a separate source_byte_copies view, with independent byte-vs-text member evidence and explicit legacy/invalid/fingerprint-mismatch availability. Text, PDF and OOXML captures bind hashing and extraction to the same input; sync upgrades legacy metadata and replaces byte-only changes. Runtime/dynamic liveness, arbitrary unregistered binary coverage, representative ranking quality/scale and remaining reference contract remain open. |
| symbol | symbol | Definition evidence for Rust/Python/JS/TS/Go/Java/C/C++/Bash/sh, Lua named/assigned/anonymous function bodies, Ruby module/class/instance/singleton/endless-method syntax, C# namespace/type/member/local-function/property/field syntax, Perl package/sub/class/method/lexical-variable declarations, PowerShell function/class/method/property/enum syntax and basic PHP declarations. The symbol MCP/native query now also inventories shared translation-key/table/URL identities with stable project-scoped IDs, null fabricated definition locations, independently bounded source proofs, full counts and reference-target roundtrips. PHP/PHTML callable symbols now expose bounded AST-bound PHPDoc parameter contracts, nested/optional/tuple/open shapes, original source spans and explicit ambiguity/limit states; compiler/dialect/inherited contract resolution remains open. Code remains the default; prefixed FQNs, a name alias and explicit literal contains mode supplement typed all-domain views. Remaining language/domain coverage, broader surfaces, reference ranking and contract suite remain open. |
| references | references | Bound imports/aliases, lexical scopes, annotated receivers/inherited member values, callbacks and type uses; explicit lexical mode. Lua adds lexical local-function call/value links with parameter/block/loop shadows and write invalidation; dynamic tables/env/imports remain unresolved. Static ambiguity/staleness preserved. Ruby adds captured owner/constant/local-constructor references, bare local versus method classification and require-relative scopes; runtime mixins/alias/eval/visibility/load-order remain unverified. Perl adds lexical/package variable bindings with source sigils, direct container normalization, initializer visibility and function-value occurrences; dynamic/ambiguous/coderef forms retain unverified states. The references MCP/native query now also accepts SQL table and literal URL targets, retains queries/read-write owners and ownerless files, requires project/schema disambiguation before occurrence filters, and separates route registrations from endpoint mentions with handler alternatives and source-bound provenance. Translation-key targets now join PHP/PHTML translator-name literal candidates and PHP/HTML placeholder occurrences with decoded key identity, source byte anchors, project disambiguation, PHP quote/escape semantics, capture budgets and explicit non-runtime coverage. Frozen uses_key relations retain occurrence counts, ignore positions/equivalent literal spelling and guard legacy/incomplete captures. Remaining languages, compiler namespaces, dynamic alias chains, translation catalogue/framework binding and other non-code kinds remain incomplete. Python/JS/TS nearest inherited declaration candidates now appear separately in references/inheritance with provenance and immutable diff; semantic override compatibility and other languages remain unverified. |
| callers | callers | Static candidate links with import/type/inheritance support; Go package/own-module/receiver binding and Java package/import/receiver/arity/hierarchy candidates added, including interface contracts. Lua local function candidates, recursion, shadowing and reassignment exclusions added. Ruby owner/instance/singleton/constructor and single-superclass candidates add project/require scope, shadow/reassignment/mutation guards and explicit reopening ambiguity. C# adds captured namespace/alias/global-using/owner/declared-type/arity/single-base candidates with overload ambiguity, shadow/generic/conditional guards and separate compiler/build/runtime limits. Perl adds explicit same-project package/lexical-sub and single-parent method candidates, guarded import effects and independent package-file relations; actual INC/Exporter/MRO/runtime remain unverified. PowerShell adds ASCII case-insensitive captured function scopes, literal PSScriptRoot dot-source functions, using-module type scope, static/this/single-base method and explicit-constructor candidates; overload/load-order ambiguity and alias/dynamic/stale boundaries remain explicit. Remaining languages/compiler/runtime cases unqualified. |
| callees | callees | Same static-analysis qualification limits as callers. |
| impact | impact | Bounded deterministic incoming traversal: calls (default), bound value/type references, or those plus inheritance/method candidates. Shortest discovered relation witnesses, project identity checks, independent depth/result limits and freshness diagnostics are qualified. Runtime/proven impact, unresolved module/resource imports, broader relation kinds, calibrated confidence/stated summaries and full reference contract remain. Affected-file summaries group returned symbols by exact document identity with captured source provenance and bounded scope. SQL table targets preserve direct read/write evidence and ownerless SQL, seed indirect code traversal from all matched owners, and require disambiguation across project/schema identities. Exact file/document targets add bounded incoming file traversal including ownerless bound occurrences, project guards, and exclusion of stale/unindexed/parse-error bridges. Python and JS/TS static module import/re-export candidates now preserve dependencies independently of symbol export lookup, with frozen relation diffs; loader/runtime and broader language/resource coverage remain open. HTML literal local-file attributes and CSS imports/declaration URLs now add separate resource candidates with base-URL/root/encoding guards, dead-file protection and frozen diffs; Captured non-code source-file identities now extend those links to JSON/XML/SVG or extracted-text resources with independent published path/project/revision/hash validation, explicit unavailable states, status diagnostics and legacy diff guards. Responsive srcset/imagesrcset and CSS image-set now add separate choice-qualified resource evidence and descriptor-aware frozen diffs, with data-URL/base/invalid-descriptor/MIME guards. HTML style attributes now contribute declaration-list-scoped URL/image-set candidates with original entity-aware anchors, recovery/size/depth limits, element-base guards and frozen property/descriptor comparisons. Literal key/URL targets now retain direct owner/file facts and run separate code and multi-root file dependency traversals from all seeds before output caps, with explicit route-kind/source filters and no URL-to-handler ownership shortcut. Common symbol impact now excludes invalid-source bridges; SQL identity guards also include schema-only candidates before source/access filtering. Raw binary fidelity, unregistered assets, runtime binding, browser/deployment resolution and broader quality/scale remain open. |
| slice | slice | Structural call subgraph; not a full data-flow/program slice or optional CPG backend. |
| code | code | Fresh stored source by exact symbol/path/document or validated current chunk/symbol IDs, project/path identity guards, inclusive line windows, Unicode-safe shared character/segment budgets, independent span limits and non-overlapping nested/partial spans. Single-symbol output compatibility and one-transaction snapshot/source observation are qualified. View-checked cursors now resume record/character/segment limits without cross-page byte repeats, including huge single-line bodies; UTF-8 byte windows, changed-view conflicts and array/delimited ID equivalence are qualified. Formatting/source-mode and transformed chunk mapping, retained historical reads, language/overload and representative scale/reference contracts remain open. |
| repo_map | repo_map | Deterministic scoped file-graph PageRank, fair compact outlines, exact project/component-prefix isolation (including identical paths), scoped coverage and strict map-text character/estimated-token budgets. Controlled reference boundary/oversized-entry comparisons and real MCP qualification passed; representative ordering, information density, scale and full-response budgets remain open. |
| entrypoints | entrypoints | Conventional/export/test/route candidates; remaining extractors/framework patterns unqualified. |
| dead_code | dead_code | Possible static reachability with exact bound value IDs, same-project unresolved call/value roots and their callees, plus nearest inherited method candidates. Deterministic bounded output and explicit incomplete/dispatch/deletion limits. Files view adds protected incoming/entrypoint/shebang files, same-project literal path refutations with source evidence, and explicit not_checked results for budget/stale/unavailable cases. Static Python/ESM file imports additionally protect targets even with missing exports. HTML/CSS relative resource candidates additionally protect parsed and independently freshness-validated non-code targets; unqualified target identity prevents negative liveness classification in that project. Responsive image and inline-style declaration candidates additionally protect captured possible targets without claiming browser/cascade selection. Broader file/import/resource extraction, alias/encoding/platform coverage, runtime/framework/external usage and representative precision/recall/scale remain unverified. |
| routes | routes | Python/Express/Axum/PHP candidates; Python route/api_route decorators now retain explicit literal method sets, deduplicate methods and bind the exact handler span without keyword-argument fake handlers. Dynamic/invalid methods remain ANY (unknown). Markup form/navigation/resource and JS/TS/embedded fetch root-relative URLs are separate unbound endpoint references with document/revision/hash/offset provenance and immutable target/explicit-request-method diff. PHP facade/member registrations and literal match method sets are inventoried without guessing handler bindings; unbound registrations are frozen in diff. Query contract v1 adds project/address/kind/separate-method filters before pages, kind counts and captured-view conflict tokens (including staleness without a new revision). JS/TS and executable inline HTML additionally capture jQuery/Axios literal-call and settings-object URL candidates, separate explicit-option versus method-name evidence, ambiguous-option guards and legacy-capability-aware frozen diffs. Framework identity/defaults, PHP namespaces/container/group prefixes, URL/router composition, runtime and broader language coverage remain. |
| schema | schema | SQL DDL replay subset, explicit default/generated expressions and captured table/column constraints; ADD/DROP CONSTRAINT metadata. ORM migration DSLs, ordering, dependency rewrites and full dialect/constraint validation remain. |
| table_usage | table_usage | Static SQL strings/files, basic DML plus MERGE action-aware target writes, explicit TRUNCATE targets and PostgreSQL-style COPY FROM/TO/query direction. Owner/line, CTE exclusion, dynamic-literal and side-effect limitations retained. Named OUTPUT INTO secondary sinks and SELECT INTO candidates are extracted with explicit uncertainty for projection schemas and variable targets. Indirect ORM/dynamic SQL, TRUNCATE descendants/CASCADE and broader dialect effects remain. |
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
| index_status | index_status | Contract v1: project-scoped coverage, explicit extractor/backend qualifications, measured bounded chunk/vector diagnostics, null unverified health, single read transaction and real MCP evidence. Successful native phase timings now persist atomically with revision/cache work, separate publication and latest/no-op runs, global scope and explicit commit/embedding/FTS exclusions. Supported Python/Rust/JS/TS/Java/PHP documentation syntax now has persisted source-bound facts and project-scoped, body-free coverage/truncation counts; unknown styles are excluded. Failure/commit/per-project timing, full documentation/typed-signature quality, representative calibration and scale remain open. |
| catalog | catalog | Contract v1: lean immutable file pages pinned across later publications; exact scope/cursor guards. Captured project registry compares current vs selected-revision corpus hashes (including preserved markup), current sync roots, explicit registered/drift/unknown states and guarded project pages. Live filesystem/config hashes, empty-project registration/settings, independent project revisions/storage and broader registry/integration/scale qualification remain. Corpus-global committed run history now has bounded anchor-pinned pages, revision filters, separate publication/no-op records and validated/unavailable metric states; failed attempts and per-project attribution remain unavailable. |
| diff | diff | Frozen file/definition/candidate-call changes plus non-call candidates, inheritance and symbol-entrypoint sets. Format 18 freezes derived SQL table/column metadata for schema diff; old missing snapshots are explicit. Format 19 captures explicit default/generated expressions and clauses with legacy-coverage flags. Format 20 adds captured constraint sets/column constraints with legacy-coverage flags. Format 29 freezes same-name inherited method candidates with explicit historical availability and no fabricated comparisons against legacy missing data. Formats 34/35 additionally freeze module imports and HTML/CSS file-resource candidates with separate historical availability flags. Format 36 freezes non-code source attachment identities and independently guards their relation comparisons against legacy absent inventory. The format 38 qualification adds bounded/hash-backed responsive descriptor/type comparisons, ignores source-position-only shifts, and reports explicit cross-capability responsive/CSS-URL comparison gaps. Format 40 additionally freezes inline-style property identity while excluding source-position/entity-spelling churn and guarding legacy absent inline coverage. Format 42 adds separately guarded Ruby import/superclass snapshots, alongside existing call/symbol/body history; format 44 qualifies local-binding and constructor-hook corrections. Format 45 separately guards C# base-candidate history against legacy missing coverage. Format 46 separately guards Perl parent/package-file history against legacy absent coverage. Format 47 separately guards PowerShell base and source-load history against missing legacy coverage; format 48 adds a byte-preserving constant using-module quote adapter verified against the unchanged MCP case. Constraint semantics, overload identities, unresolved relation kinds and regression interpretation remain. |
| live_read | live_read | Bounded live range plus checksum/enclosing summary and deterministic range-only insertion/deletion/replacement hunks. Exact bounded LCS, explicit coarse fallback, independent hunk truncation, newline/CRLF fidelity and frozen-index comparisons are qualified; character/semantic edits and broader encoding/scale qualification remain. |
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

Repository-map subset: [qualification](qualification/repo-map-2026-10-24/README.md),
project/path-isolated graph identities, strict first-entry budgets, deterministic
round selection and real MCP stale/unindexed isolation. See [contract](REPO_MAP.md).
Representative corpus ranking/scale and whole-goal acceptance remain open.

Index-status subset: [qualification](qualification/index-status-2026-10-25/README.md),
versioned capabilities and measured quality in one read transaction, with explicit
empty/unavailable/unmeasured states and bounded samples. [Contract](INDEX_STATUS.md).
Full build/commit/failure/per-project timing, semantic quality and broader acceptance remain open.

Python HTTP-method extension: [qualification](qualification/python-routes-2026-10-25/README.md),
explicit method sets, conservative dynamic fallback, exact handler spans and
source-sync refresh. Cache format 21 forces older extraction to be refreshed;
framework identity/defaults and whole-goal comparative acceptance remain open.

Markup endpoint references: [qualification](qualification/markup-endpoints-2026-10-25/README.md),
syntax-only root-relative targets, separate kinds, no handler/liveness inference,
occurrence identity, stale exclusion and project-isolated historical diff.
Format 22 refreshes extraction caches; dynamic/relative/external URL resolution,
broader request-client resolution and full reference parity remain open.

Fetch endpoint references: [qualification](qualification/fetch-endpoints-2026-10-25/README.md),
literal JS/TS and inline-script candidates, separately captured request methods,
script-type/source exclusions, stale/sync and method-diff evidence. Format 23
refreshes caches. Global identity, alias/data-flow resolution, dynamic/relative/
external targets, other clients and representative comparative quality remain open.

PHP route registry: [qualification](qualification/php-routes-2026-10-25/README.md),
AST static/member candidates and match arrays, exact source evidence, conservative
handler identity, stale/sync refresh and project-isolated unbound-registration
diffs. Format 24 refreshes extraction. Namespaces/aliases/framework identity,
group composition and full PHP binding/representative quality remain open.

Route-query contract: [documentation](ROUTE_QUERIES.md),
[qualification](qualification/route-queries-2026-10-25/README.md), exact scoped
selection, literal filters and optimistic read-transaction pages. Retained snapshot
cursors, representative scale and full route/framework acceptance remain open.

Catalog extension: [contract](CODE_CATALOG.md),
[qualification](qualification/catalog-2026-10-25/README.md), retained-revision
file pages and guarded current project summaries. Captured matches explicitly
do not establish filesystem/config/source integrity; full registry acceptance
remains open.

Native indexing measurements: [contract](INDEX_TIMINGS.md),
[qualification](qualification/index-timings-2026-10-25/README.md), schema-25
successful-run ledger, cold-publication/no-op identities, rollback and migration
evidence. Commit/failure/per-project/end-to-end and representative performance
acceptance remain open.

Lua extension: [contract](LUA_CODE_ANALYSIS.md),
[qualification](qualification/lua-index-2026-10-25/README.md), native grammar
0.5.0, lexical function candidates, explicit dynamic exclusions, source-body
provenance and stale/sync qualification. Syntax format 25 refreshes older caches;
compiler/dialect/runtime/import/table and representative quality gates remain open.

Bulk SQL usage: [contract](SQL_BULK_USAGE.md),
[qualification](qualification/sql-bulk-usage-2026-10-25/README.md), MERGE action
classification, explicit TRUNCATE writes and COPY direction/query reads with
source provenance and stale/sync evidence. Syntax format 26; bulk write_check,
secondary targets, data/permissions and full dialect/quality acceptance remain open.

Documentation facts: [contract](CODE_DOCUMENTATION.md),
[qualification](qualification/code-documentation-2026-10-25/README.md), supported
source-syntax attachment, bounded excerpts/segments, Unicode provenance and scoped
coverage with unknown forms excluded. Format 27; compiler/rendered/full-language
documentation and representative quality acceptance remain open.

SQL secondary writes: [contract](SQL_OUTPUT_USAGE.md), [qualification](qualification/sql-output-2026-10-25/README.md). Full SQL and RAGRAF parity remain open.

Live-read edit ranges: [contract](LIVE_READ_DIFF.md), [qualification](qualification/live-hunks-2026-10-25/README.md).

Inherited method candidates: [contract](METHOD_RELATIONS.md), [qualification](qualification/method-relations-2026-10-25/README.md).

Impact relation traversal: [contract](IMPACT_RELATIONS.md), [qualification](qualification/impact-relations-2026-10-25/README.md).

Impact affected files: [contract](IMPACT_RELATIONS.md#affected-file-summary), [qualification](qualification/impact-files-2026-10-25/README.md).

SQL table impact: [contract](IMPACT_RELATIONS.md#sql-table-targets), [qualification](qualification/table-impact-2026-10-25/README.md).

File impact: [contract](IMPACT_RELATIONS.md#file-targets), [qualification](qualification/file-impact-2026-10-25/README.md).

Committed run history: [contract](CODE_CATALOG.md#committed-index-run-history), [qualification](qualification/run-history-2026-10-25/README.md).

Dead-code possible reachability: [contract](DEAD_CODE_REACHABILITY.md), [qualification](qualification/dead-reachability-2026-10-25/README.md).

File dead-code candidates: [contract](DEAD_FILES.md), [qualification](qualification/dead-files-2026-10-25/README.md).

Duplicate member comparison: [contract](DUPLICATE_PRIMARY_COMPARISON.md), [qualification](qualification/duplicate-comparison-2026-10-26/README.md).

Independent duplicate families: [contract](DUPLICATE_FAMILY_GROUPING.md), [qualification](qualification/duplicate-families-2026-10-26/README.md).

Captured source-byte copies: [contract](SOURCE_BYTE_DUPLICATES.md), [qualification](qualification/source-byte-copies-2026-10-26/README.md).

Non-code references: [contract](NON_CODE_REFERENCES.md), [qualification](qualification/non-code-references-2026-10-26/README.md).

Translation-key references: [contract](TRANSLATION_KEY_REFERENCES.md), [qualification](qualification/translation-keys-2026-10-26/README.md).

Shared symbol nodes: [contract](SHARED_SYMBOL_NODES.md), [qualification](qualification/symbol-nodes-2026-10-26/README.md).

Literal target impact: [contract](LITERAL_TARGET_IMPACT.md), [qualification](qualification/literal-impact-2026-10-26/README.md).

Client endpoint candidates: [contract](CLIENT_ENDPOINT_REFERENCES.md), [qualification](qualification/client-endpoints-2026-10-26/README.md).

Stored source reads: [contract](CODE_READS.md), [qualification](qualification/code-reads-2026-10-26/README.md).

Source-read continuation: [contract](CODE_SOURCE_PAGINATION.md), [qualification](qualification/code-cursors-2026-10-26/README.md).
