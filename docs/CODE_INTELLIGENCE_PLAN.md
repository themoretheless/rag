# Code intelligence implementation plan

User scope: implement the comparison plan for GitNexus, Codebase Memory MCP and
RAGRAF. Existing uncommitted work is part of the baseline and must be preserved.
An implemented subset does not close this plan.

## Requirements and acceptance

1. Cargo-aware Rust graph: captured workspace/package/target/dependency manifests,
   renamed path dependencies, nonstandard roots, module paths, explicit target and
   feature configuration identity; no cross-crate/target false binding. Prove
   manifest-only invalidation, stale boundaries, revision history and MCP behavior.
2. Rust receiver inference: return types, method chains, await, collection elements,
   pointer wrappers. Require exact positive and wrong-owner negative expectations.
3. Rust traits/generics: cross-file traits, bounds, UFCS, associated types and
   explicit ambiguity. Do not label candidate runtime targets as proven dispatch.
4. Reproducible quality corpus: multi-crate fixtures and fixed real repositories,
   independently labelled edges, precision/recall, false targets and coverage;
   run through native and actual MCP surfaces with machine-readable artifacts.
5. Git-diff impact: working-tree/staged/commit inputs, added/removed/modified symbol
   mapping, source identity/freshness and explanation paths, bounded results.
6. Affected tests: evidence paths from changed symbols to test entrypoints,
   transparent incomplete coverage and suggested executable test commands.
7. Symbol-to-symbol path: typed bounded paths, explicit ambiguity/stale results,
   project isolation and per-edge source evidence.
8. Index performance: dependency-scoped incremental resolution and measured cold,
   warm/no-op, body/signature/manifest-change workloads, end-to-end commit time,
   memory and correctness equivalence. Compare equivalent competitor workloads.
9. Semantic rename: preview exact bound occurrence edits, collisions and freshness
   checks, explicit unsupported cases, safe application and MCP authorization.
10. Infrastructure graph: captured service/deployment/container/executable/code
    relations with evidence; bounded cross-project linking and no runtime claims.

## Execution order

Quality fixtures and Cargo model → receiver inference → traits/generics → paths →
Git-diff impact → affected tests → incremental performance → rename → infrastructure
→ complete requirement-by-requirement qualification and comparative measurements.

## Status

2026-10-26 raw pointer extension traits: format 68 retains pointer const/mut and
primitive-pointee identity, explicit cast targets and existing Self return-chain
substitutions. Pointer receivers do not autoderef or cross mutability/pointee
owners. Unknown/known-unsized blanket pointees, fabricated local inherent impls
and known sysroot method-name collisions fail closed. Four native regressions,
five labelled actual-MCP targets and one passing compiler-valid Cargo test
qualify the bounded behavior. Fixed source/config/oracle/labels with exact
native/MCP parity improve 415/570 to 459/570 correct targets (memchr 313/466 to
357/466); 44 recovered direct calls, no lost correct sites and zero observed
false targets. Workspace: 1522 passed, zero failed, one ignored; focused Rust:
80 passed; strict Clippy and three Python metric tests pass. Frozen executable
identities, scored cases, transitions and limits:
`qualification/rust-raw-pointers-2026-10-26/README.md`, `checks.json`, `summary.json`
and `runtime.json`. Shared source is not frozen and concurrent unrelated work is
not attributed here. Full compiler/build/generic/sizedness semantics, remaining
111 corpus misses, broader negative quality and equal-work release competitor
speed/RSS keep the original full plan open.


2026-10-26 tuple struct projections: format 67 captures positional field types
with exact nominal owners and existing generic substitutions, including nested
numeric projections. Field visibility nodes do not consume positions; out-of-range
slots and enum payloads do not guess owners. Any attributed tuple layout remains
unverified because cfg removal can renumber slots. Two native regressions and a
compiler-valid actual-MCP fixture check four targets, warm no-op reuse and one
passing Cargo test. Same source/config/oracle/labels with native/MCP parity improve
410/570 to 415/570 correct targets (memchr 308/466 to 313/466), with zero observed
false targets and no lost correct sites. Focused Rust: 76 passed; strict Clippy and
three Python metric tests pass. Workspace: 1512 passed, zero failed, one ignored.
Frozen executable identities and raw evidence:
`qualification/rust-tuple-fields-2026-10-26/README.md` and `checks.json`,
`summary.json`, `runtime.json`, `transitions.json`. Shared source is not frozen;
unrelated concurrent changes are not attributed here. Remaining 155 corpus misses,
complete build/compiler semantics, broader negative quality and equal-work release
competitor speed/RSS keep the full original plan open.



2026-10-26 conditional members/extern namespaces: format 66 retains named-field
alternatives with exact owner/source identity, projects conditional initializers,
filters known-disabled owners/macros and preserves hoisted namespace declarations
with original cfg anchors. Standard extern alias tags and namespace-shadow guards
prevent fabricated wrapper types; ordinary use paths are not extern bindings.
The frozen server/helper pair passes all three fixed-source/oracle/label corpus
runs with native/MCP parity: 358/570 to 410/570 correct targets, zero observed
false targets, 52 recovered sites and zero lost correct sites. Memchr improves
256/466 to 308/466; direct calls 333/486 to 364/486. Actual MCP checks eight targets
across two configurations and executes two passing Cargo tests. Final workspace:
1510 passed, zero failed, one ignored; focused Rust: 65 passed; strict Clippy and
three Python adapter/metric tests pass. Shared source checkout is not frozen and
unrelated concurrent changes are not attributed here. Evidence:
`qualification/rust-cfg-members-2026-10-26/README.md`, `checks.json`, `summary.json`,
`runtime.json` and scored cases. Remaining 160 corpus misses, broader independent
negative coverage, full compiler/build semantics and equal-work release/competitor
speed/RSS qualification keep the original full goal open.


2026-10-26 impl Self/declaration contexts: format 60 captures impl-owner AST types
instead of parsing generic/lifetime display names for Self. Declaration lookups
use their original inline module and normalize explicit self/super paths there.
Explicit Self substitutions retain precedence, and nested functions do not inherit
an enclosing method's Self. Four native positive/wrong-owner/unknown regressions
pass with the 56-test focused Rust suite. Actual MCP resolves six independently
labelled targets and reuses a no-op index; a compiler-valid fixture executes one
passing test distinguishing lifetime/root/inline/super-relative owners. Fixed
sources/config/oracle/labels and native/MCP parity pass. Memchr improves 164/466
to 256/466; combined corpus 266/570 to 358/570; direct calls 241/486 to 333/486,
with zero observed false targets in both phases. Final workspace: 1470 passed,
zero failed, one ignored; strict Clippy and three Python label/metric tests pass.
Unrelated concurrent work is not attributed here. Contract and exact evidence:
`RUST_RECEIVER_TYPES.md` and
`qualification/rust-self-context-2026-10-26/README.md`, `checks.json`, `runtime.json`,
`summary.json` and all scored cases. Remaining 210 memchr misses stay explicit.
The full original goal remains active: remaining semantic/configuration coverage,
representative corpus/external negatives and comparative release performance/RSS
are not yet qualified.

2026-10-26 Rust re-exports: formats 56/59 add captured module import/visibility
facts and bounded named/grouped/multi-hop/module-glob lookup, cyclic glob handling,
cfg states, original target identities and conservative access/exposure checks.
Inline module imports no longer leak to crate scope. Associated/function-local
items cannot be re-exported as free namespace symbols; module/type aliases and
glob paths cannot expose private bases or nested modules. Unknown competing
imports and work exhaustion never publish a partial unique target. Contract:
`RUST_REEXPORTS.md`. Final 1461 workspace tests passed (zero failures, one ignored),
52 focused Rust tests, seven hardened namespace tests, strict Clippy and three
Python label/metric tests passed. Actual MCP/compiler fixture checks eight exact
targets across two feature configurations, original method identity through a
type alias, manifest-only rebinding/history and a cross-crate private import
rejected by both the graph and rustc E0603. Fixed-corpus targets improve 264/570
to 266/570, with zero observed false targets; ryu improves 91/95 to 93/95 and its
same-rustc HIR-corroborated source-position subset improves 66/68 to 68/68.
Original SCIP labels and denominator are preserved. Exact evidence:
`qualification/rust-reexports-2026-10-26/README.md`, `checks.json`, `runtime.json`,
`summary.json` and `compiler-anchor-audit.json`. Concurrent unrelated source work
is not attributed here. The complete goal remains open: remaining namespace/
typing/cfg/compiler semantics, representative corpus/external negatives and
equal-work release/competitor speed/RSS measurements are unqualified.

2026-10-26 conditional Rust expressions: formats 53/54 admit cfg-filtered call
arguments, statement expressions and blocks while preserving unknown-mutation
invalidations and field-source anchors. Permission syntax remains suppressed when
its conditional body is not projected. The test overlay preserves feature import
facts until actual build configuration evaluation. Five new native regressions
and a compiler-valid two-feature MCP fixture qualify active/inactive targets,
body paths, test discovery and both suggested Cargo commands (one passing test
each). Current focused Rust suite: 45 passed; strict Clippy passed. Final full
workspace: 1445 passed, zero failed, one ignored external-DB fixture; the dirty
shared checkout includes concurrent unrelated changes, not attributed here.
The same-source/oracle/label controlled corpus improves 172/570 to 264/570 correct
targets, with zero observed false targets in both phases; direct calls improve
156/486 to 239/486. Ryu improves 56/95 to 91/95 and memchr 107/466 to 164/466.
An independent same-rustc HIR path-anchor audit corroborates 68/71 ryu direct-call
sites, of which 66 are resolved. Two missing anchors are in disabled small-feature
branches and a third is an unmodeled HIR method representation; the original
SCIP denominator is preserved. Evidence and limits:
`qualification/rust-cfg-expressions-2026-10-26/README.md`, `summary.json`,
`runtime.json` and `compiler-anchor-audit.json`. Compiler-wide re-exports/typing,
remaining nested cfg forms, broader independent corpus/external negatives and
equal-work release/competitor performance remain open. This advances requirements
1/2/4 and does not complete the original ten-item goal.

2026-10-26 independent real-Rust corpus: pinned itoa, ryu and memchr sources now
have independently derived rust-analyzer SCIP/CST labels, explicit rustc/default
library configurations, byte-identity controls and actual native/MCP comparison.
Format 50 to 52 improves 107/570 to 172/570 admitted callable-reference targets,
with two to zero false targets. The direct-call subset improves 106/486 to
156/486. cfg_attr handling, legal empty cfg values, bounded larger target-feature
sets and macro/module-call false-target guards are implemented; conditional
public imports now pass their existing declaration-level filter instead of
blocking all binding. Feature/unknown-cfg wrong-target regressions pass. Evidence:
`qualification/rust-real-corpus-2026-10-26/README.md` and `summary.json`.
Final Rust tests: 40 passed; strict library/test/example Clippy and three Python
adapter/metric tests passed. The earlier workspace checkpoint passed 1417 tests,
one ignored. This is a small source-definition correspondence corpus with zero
external-negative labels, excluded ambiguous/generated anchors and RA error logs,
not universal precision or runtime proof. Remaining nested cfg expressions and
typing omissions yield low recall; requirement 4, equal-work release/competitor
performance (8) and the complete original goal remain open.

2026-10-25: audited the working tree. Existing structural code graph, lexical/type
binding, source sync, immutable history, index diagnostics/timing ledger, route/SQL
analysis and provenance are baseline capabilities, not missing feature claims.
All ten requirements above remain open until corresponding evidence is recorded.

2026-10-25 implementation progress: added captured Cargo TOML inputs, independent
target/module lookup, renamed/inherited path dependencies, custom roots/names,
stale-manifest barriers and bounded target evidence in `index_status`. Added a
compiler-checkable three-package/five-target fixture with independent call labels,
native regressions and real MCP lifecycle checks. See `RUST_CARGO_GRAPH.md` and
`qualification/cargo-workspace-2026-10-25/README.md`. This advances requirements 1
and 4; feature/cfg configurations, full Cargo semantics, real-repository quality
and comparative performance remain open. Requirements 2, 3 and 5–10 are pending.

2026-10-25 receiver implementation: index format 31 adds captured AST return/field/
binding/expression facts; Rust member calls now infer declared-return chains,
async await, collection elements, known standard pointer wrappers and explicit
generic return arguments in declaration context. Native wrong-owner/shadowing
regressions and an independently labelled compiler-checkable runtime fixture were
added. See `RUST_RECEIVER_TYPES.md` and `qualification/rust-receivers-2026-10-25/`.
Qualification passed: 1268 workspace tests (one ignored), strict library/test
Clippy, positive-fixture compiler check and all 12 labelled real-MCP call targets
plus stale/reuse/rebinding/history assertions. This is fixture qualification for
requirement 2 and progress on requirement 4, not representative Rust quality or
whole-plan completion. Cargo build configurations (1), full traits/generics (3),
real-corpus/comparative evaluation (4/8) and requirements 5–10 remain open.

2026-10-25 trait implementation: format 32 captures cross-file impl ownership,
generic parameter/bound facts, UFCS, trait declaration targets, default methods
and associated assignments/projections. Generic impl substitutions flow to
returns and fields; inherent precedence and explicit trait ambiguity are tested.
Known Cargo targets guard impl ownership; work exhaustion never publishes a
partial unique target. `RUST_TRAIT_BINDING.md` defines the bounded rules and
remaining compiler limitations. Native and positive compiler fixtures were
added. Qualification passed: workspace regression 1276 tests (one ignored) before
final hardening, followed by 20 current receiver/trait tests, strict Clippy,
compiler fixture checking and all 10 labelled MCP targets/lifecycle assertions.
The final binary also passes the earlier 12-call receiver and 5-call Cargo MCP
fixtures. Evidence is under `qualification/rust-traits-2026-10-25/`. This advances
requirement 3; Cargo configurations, representative evaluation and requirements
5–10 remain open. Full goal completion remains unproven.

2026-10-25 configuration implementation: format 33 adds opt-in captured
`package.metadata.rag.graph` selection of one compilation target per package,
local features/defaults, three-valued cfg predicates, contextual symbol/reference
IDs, raw-cache reuse and configured frozen revision evidence. Invalid/ambiguous
configuration blocks derived binding and permission/SQL facts. See
`RUST_BUILD_CONFIGURATIONS.md`. Multiple compilation units, dependency feature
unification and remaining Cargo/compiler semantics are still open. Qualification
evidence is recorded in `qualification/rust-cfg-2026-10-25/`; no full-plan or
competitor-superiority claim follows from these fixtures.

2026-10-25 symbol paths: `trace_path` adds typed calls/reference/all relation
selection, forward/back/both simple paths, explicit endpoint ambiguity and
project checks, original per-step evidence and shared candidate/traversal work
limits. Stale/unindexed/parse-error bridges are excluded. Native tests and an
independently labelled real-MCP cross-file fixture cover paths, exact occurrence
provenance, limits, reverse traversal, values, projects and refresh. See
`SYMBOL_PATHS.md` and `qualification/symbol-paths-2026-10-25/`. This implements
requirement 7 for supported captured graph edges; representative extraction
quality remains requirement 4. Git inputs/tests/performance/rename/infrastructure
and remaining Cargo/semantic work are still open; the complete goal is active.

Qualification update: the current workspace regression, strict library/test
Clippy, offline positive fixture compiler check, real-MCP path fixture and
configuration regression all passed. Exact counts, commands and binary/fixture
hashes are in `qualification/symbol-paths-2026-10-25/checks.json` and runtime
reports. Earlier format-33 Cargo/receiver/trait regression artifacts are retained
under `qualification/rust-cfg-2026-10-25/`. No whole-plan completion is claimed.

2026-10-25 Git source impact: `detect_changes` captures index→working, HEAD→index,
HEAD→working and pinned commit→commit inputs without checkout or conversion
filters. It constructs two bounded native source graphs and maps added/removed/
modified, configuration/binding changes and explicitly unavailable/ambiguous
symbol groups. Two-sided incoming witnesses preserve old relationships to
removed definitions. Index/flags/untracked membership and working byte hashes
are revalidated; capture overflow refuses partial graphs. This implements
requirement 5 within supported captured graph semantics, with explicit
`persistent_index_used=false`. See `GIT_DIFF_IMPACT.md` and
`qualification/git-impact-2026-10-25/`. Native lifecycle, actual-MCP Git fixture,
path/configuration regressions, strict Clippy and baseline compiler checks passed;
exact current workspace results and hashes are recorded in qualification files.
Source coverage and comparison limits remain explicit; no whole-program impact
or competitive performance claim follows. Requirement 6 (affected tests) is
next. Full original goal remains active, including remaining Cargo/typing,
real-corpus quality, incremental performance, semantic rename and infrastructure.

2026-10-26 related tests: `affected_tests` reads a project-scoped captured test-cfg
view, disambiguates changed symbol selectors and returns bounded incoming paths
to test entrypoints with command argv suggestions. Git `include_tests=true`
attaches corresponding recommendations to each captured side. Rust inline
relative calls/imports, pure cfg(test) external modules, conditional private
imports, exact test/ignore attributes and raw Cargo target names are handled;
normal configured IDs map only from their current captured configuration.
The query explicitly does not authorize CI omission or claim runtime coverage.
See `AFFECTED_TESTS.md` and `qualification/affected-tests-2026-10-26/`.
The labelled real-MCP fixture found seven expected tests; all seven suggested
commands actually executed one passing test, including ignored and hyphenated
integration cases. Native correctness/negative regressions, strict Clippy,
baseline compiler and previous Git/path/configuration MCP checks accompany the
feature. Exact final workspace outcome and hashes are in qualification files.
This implements requirement 6 within supported captured static graph semantics.
Original real-corpus quality, dependency-scoped incremental performance and
comparative measurements, semantic rename, infrastructure, and documented
remaining Cargo/typing semantics stay open. No full-goal completion is claimed.

2026-10-26 incremental binding implementation: immutable graph snapshots now
carry versioned content-addressed reference/call caches. Configured Rust source
keys use captured transitive package dependencies and current provider inventory;
invalid/mixed-language boundaries conservatively fall back to project scope.
Empty-reference documents are counted separately. Bulk AST-cache reads retain
unchanged payloads and persist only changed/deleted rows. Other graph families
continue recomputation. `force_full_resolution` supplies a full-binding audit
against an existing immutable revision; runtime completion measures through the
successful commit, separate from the older pre-commit timing ledger.
See `INCREMENTAL_RESOLUTION.md` and
`qualification/incremental-resolution-2026-10-26/README.md`.

Qualification: 1347 workspace tests passed, zero failed, one ignored; strict
library/test Clippy and build finished. Four native cache tests and five actual
MCP runners passed. The latter share one frozen format-42 binary and cover
resolution/full equivalence, affected tests (seven executed commands), Git
impact, configuration and traits. A synthetic 79-document Cargo benchmark has
compiler-checked source states, retained old/new observations and three
alternating same-binary full/incremental pairs. Body-only reuse improves this
workload; signature/manifest cases still recompute the 288 app call sites.
Cold/RSS regressions and concurrent compiler activity remain explicit in the
artifacts. No production speed or competitor superiority is qualified.

This advances requirement 8 without closing it. Representative real-repository
quality/precision/recall, release-mode comparable time/memory and competitor
measurements, broader Cargo/typing semantics, semantic rename and infrastructure
remain open. The complete goal stays active. No commit, push or installed binary
replacement occurred; concurrent worktree changes remain preserved.

2026-10-26 reviewed Rust rename: `rename` previews exact AST-anchored definition
and bound occurrence byte edits across fresh captured normal/test-cfg views.
All captured project file/provider/manifest hashes guard the deterministic plan
token. Unknown identifiers, ambiguity, possible collisions and unsupported forms
block instead of text matching. `apply_rename` separately requires admin authority,
recomputes the reviewed plan, stages same-directory edits/original backups and
records a synced recovery journal. Ordinary failures attempt rollback while
preserving detected concurrent edits; crash-atomic multi-file application and
automatic crash recovery are not claimed. The index explicitly requires refresh.
See `SEMANTIC_RENAME.md` and `qualification/semantic-rename-2026-10-26/`.

Actual stdio MCP renamed four declaration/call/value/test occurrences, preserved
comments/strings, refused wrong/stale tokens, verified original backups and
re-ingested the renamed graph. Actual Cargo ran one passing test before and
after the change. Authenticated HTTP MCP verified read/write preview access,
hidden/forbidden application for those roles, and admin application-handler
access. Five native safety/failure tests and current workspace/strict-Clippy
outcomes are retained in qualification artifacts. The same frozen format-45
binary passes previous resolution and affected-test MCP regressions.

This implements requirement 9 for the supported captured Rust subset with explicit
unsupported cases, not representative or compiler-complete rename qualification.
Original real-corpus quality, comparable release/competitor performance,
infrastructure graph and remaining Cargo/typing semantics stay open. Preserve
the complete goal and all concurrent source work; no full-goal completion,
production binary replacement, commit or push is claimed.

Rename final qualification: 1367 workspace tests passed, zero failed, one ignored;
strict library/test Clippy and all five current native safety tests passed.
Actual MCP rename and authenticated role checks plus resolution/affected-tests
regressions passed on the same frozen format-45 binary. Exact artifacts and
hashes are in `qualification/semantic-rename-2026-10-26/checks.json`.

2026-10-26 captured infrastructure: `infrastructure` adds a read-authorized graph
over immutable source revisions. Compose build/image declarations, Kubernetes
namespace/selector/workload/container/image relationships, literal GitHub Actions
build/script commands and Docker stages/COPY/default startup form bounded paths
to captured source files and Rust entrypoint identities. Explicit `--bin` Cargo
builds in the supported literal subset retain COPY/manifest/RUN/inter-stage COPY
provenance; unknown operations invalidate prior container-source mappings.
Dynamic/duplicate/unsupported YAML and Docker forms stay explicit. Source
ingestion now includes Dockerfile/Containerfile variants. Cross-project links
require named additional projects and retain exact-image-name candidate semantics.

Default queries exclude stale stored inputs and diagnose unpublished sources;
explicit revisions retain frozen captured source views. No filesystem, registry,
cluster, shell or build execution occurs in the query. Read responses preserve
per-edge decoded source byte/line/hash evidence, original symbol IDs, ambiguity
and construction/traversal/result limits. Infrastructure edges are derived by
the current contract from frozen sources; persisted edge snapshots/diffs and
broader Helm/Terraform/runtime providers are not claimed.
See `INFRASTRUCTURE_GRAPH.md` and
`qualification/infrastructure-2026-10-26/README.md`.

The independently labelled actual-MCP fixture traces eight edges from Service
to Rust main, excludes a different namespace, checks supporting source anchors,
CI/script paths, stale/frozen behavior, explicit project linking and search
limits. Its positive Rust bin is compiled and executed on the host; Docker and
Kubernetes are not executed. Seven native infrastructure tests include invalid
YAML, source-mapping invalidation and the 128/129-stage boundary. Rename now keeps
recognized documentation/configuration files under hash guards without treating
their text as Rust name collisions; other code languages stay unsupported.
The extended actual-MCP rename fixture exercises guarded README/YAML/Dockerfile
inputs. Exact current workspace/Clippy/build and frozen binary identities are
recorded in the qualification files.

This implements requirement 10 for the supported captured provider subset,
without representative or live-system proof. The complete original goal stays
active: real-repository quality/precision/recall, equivalent release/competitor
performance, remaining Cargo/typing semantics and documented provider/rename
coverage limits remain open. Concurrent language work is preserved and not
attributed to infrastructure. No installed binary replacement, commit or push
occurred.

Infrastructure qualification update: workspace checkpoint 1398 passed, zero
failed, one ignored; final current code-index suite 351 passed and strict
library/test Clippy passed after localized style repairs. Infrastructure,
metadata-aware rename and full-binding resolution actual-MCP runners passed on
the same frozen format-48 binary. `checks.json` records exact hashes and the
nonfrozen source-checkout boundary. The complete original goal remains active.
