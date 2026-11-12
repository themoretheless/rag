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

## Additional acceptance from the follow-up comparison plan

The later comparison plan also retains the following product milestones. They
are additional scope, not replacements for the ten requirements above. Existing
entrypoints or partial implementations do not by themselves qualify them:

- Optional compiler/LSP-derived Rust semantics integrated with AST indexing,
  including explicit evidence origin, configuration identity and disagreement.
  SCIP used only as a test oracle does not qualify this runtime backend.
- Execution-flow navigation and subsystem/community maps with source-backed
  edges; structural symbol paths alone do not qualify the complete map workflow.
- Rust test selection supported by admitted per-test runtime coverage, with
  broader-run fallback for missing/stale evidence. Static affected_tests remains
  report-only until this independent coverage gate is met.
- A unified Git-diff investigation workflow combining changed APIs, impact paths,
  related tests and unchecked scope, plus consistent evidence/freshness/ambiguity
  contracts across its surfaces.
- Data-flow slicing based on qualified def-use/argument/result relations, beyond
  structural reachability, and a visual source/graph explorer for subsystem and
  impact navigation. Existing wiki graph rendering is not full code-explorer
  qualification.

Each milestone needs implementation-specific runtime evidence and comparative
tasks before completion; none is closed by the narrow receiver milestones below.

## Status

2026-10-26 standard arch types: the qualified format81 runtime retains selected
imported core/std arch struct identities, with captured target, exact paths,
aliases/glob/cfg/dependency shadows and local-trait target guards. A generated
static rust-src catalog needs no toolchain I/O at lookup. Direct external member
lookup remains unverified; explicit UFCS can select a local trait impl. Glob
capture, inactive optional-core handling and a bounded-work trait prefilter fix
three observed barriers. Five semantic/one cfg regressions and102 focused Rust
tests pass. Actual MCP checks3 cross-file targets/warm reuse and Cargo executes
one passing aarch64 test. Same1183 labels/source/config/oracle and native/MCP
parity improve478/570 ->502/570 positives,24 recovered memchr wrappers,no lost
correct sites and zero false local targets;613 negatives unchanged. Workspace
1600 passed,zero failed,one ignored; strict Clippy,five protocol and two catalog
tests pass. Catalog regeneration/source hashes verify; compiler execution covers
two aarch64 types only. Before-fix failures and a preliminary no-gain478/570
cohort are retained. Evidence: `RUST_STDARCH_TYPES.md` and
`qualification/rust-stdarch-types-2026-10-26/README.md`,checks,summary,transitions,
triage,catalog provenance,raw cases/logs. Shared source not frozen; unrelated
schema/test work not attributed.68 positive misses and all remaining original/
follow-up requirements keep the full goal active; no performance claim.

2026-10-26 callback outputs and try carriers: format79 captures declared fn and
standard Fn/FnMut/FnOnce outputs, raw invocation arity and standard Option/Result
success projection through ?. Generic/where/borrowed/alias forms retain lexical
identity; wrong arity, HRTB, custom carriers and unverified shadows fail closed.
Legacy Call payloads remain decodable. Four new semantic regressions and96 focused
Rust tests pass. Actual MCP checks5 exact User/Other targets, four explicitly
unresolved callback bodies and warm reuse; Cargo executes one passing test with
successful outputs and None/Err returns. Same fixed1183 labels/source/config/
oracle and native/MCP parity improve473/570 ->478/570 positives,5 recovered
memchr calls,zero lost correct sites and zero false local targets;613 negatives
unchanged. Workspace1584 passed,zero failed,one ignored; strict Clippy and5 Python
protocol tests pass. Evidence: `RUST_CALLBACK_OUTPUTS.md` and
`qualification/rust-callback-try-2026-10-26/README.md`,checks,summary,transitions,
triage,raw cases/logs. Shared source not frozen; unrelated concurrent work not
attributed.92 positive misses and all remaining original/follow-up requirements
keep the full goal active; no release/performance/superiority claim.

2026-10-26 associated trait bounds: the qualified format78 runtime captures
nongeneric associated declarations with owner/source/cfg evidence and carries
generic Self/associated substitutions through member and static-call returns.
Unknown/duplicate alternatives, GAT/generic-trait and optional Sized cases retain
conservative exclusions. Four semantic and one cfg regressions pass;92 focused
Rust tests pass. Actual MCP checks8 targets across2 features, stale exclusion,
manifest-only rebinding/raw reuse, warm revision reuse and frozen edge changes;
Cargo executes2 passing tests. Same1183 labels/source/config/oracle and exact
native/MCP parity improve467/570 ->473/570 positives,6 recovered memchr calls,
zero lost correct sites and zero false local targets;613 negatives unchanged.
Final workspace1569 passed,zero failed,one ignored; strict Clippy and five Python
protocol tests pass. Before-fix regression, first MCP cfg failure and preliminary
concurrent-PHP compile failure are retained; final reruns pass. Shared source is
not frozen and unrelated work is not attributed. Evidence:
`qualification/rust-associated-bounds-2026-10-26/README.md`,checks,summary,
transitions,triage,raw reports and logs.97 positive misses and all remaining
original/follow-up plan requirements keep the full goal active.

2026-10-26 generic receiver parameters: format76 preserves lexical parameter
identity, trait declarations and conservative implicit-Sized evidence instead of
collapsing unbounded T to Unknown. Pointer blanket matching excludes ?Sized,
unknown and known-unsized pointees; distinct generic parameters do not unify.
Three new native regressions, 87 focused Rust tests, three actual-MCP exact
targets, one compiler-executed test and two compiler-rejected ?Sized controls
pass. Fixed source/config/oracle/1183 labels and native/MCP parity improve
465/570 to467/570 positives with two recovered memchr calls, zero lost correct
sites and zero false local targets;613 external negatives unchanged. Workspace:
1564 passed,zero failed,one ignored; strict Clippy and five protocol tests pass.
Evidence: `qualification/rust-generic-parameters-2026-10-26/README.md`, checks,
summary, transitions, frozen binary identities and logs. Shared source is not
frozen; concurrent unrelated work is not attributed. Production unchanged.
103 positive misses, full generic/compiler/Cargo semantics and equivalent-work
release competitor performance keep the original full plan active.

2026-10-26 frozen miss inventory: triage_rust_quality.py verifies all three
same-release runs' source hashes, oracle identities/order, per-case scores,
aggregate metrics and parity scopes before publishing all 105 missed sites.
Observed groups: 47 unresolved receivers, 38 unverified macro tokens, 18 absent
observations and two other unresolved bindings. Source/config inspection places
the 18 absent observations inside disabled test/architecture/feature branches;
they remain in the original denominator. Four corruption/path/byte-column guard
tests pass. No Rust implementation changed and no additional target was resolved;
quality stays 465/570. Evidence and next semantic/configuration acceptance work:
`qualification/rust-miss-triage-2026-10-26/README.md`, `triage.json`, `checks.json`.
This is requirement4 diagnostic evidence, not completion of requirements1-3 or8.

2026-10-26 same-release independent Rust quality: the exact frozen benchmark
server dab96adb738e01528410810122a0a7002edfa5ae1090761963be0201be3ea3fe
now passes all three fixed SCIP/CST corpus runs, with 465/570 correct local
targets and zero false local targets across those positives and 613 external
negative calls. All 1183 labelled native/MCP candidate sets match; every original
baseline target position and per-case score is retained. External observations
remain608/613, with five missing observations explicitly unqualified. The native
exporter links original optimized server Cargo library objects/full metadata with
the same compiler/release controls; 838 search inputs, package/features and source/
binary provenance are verified without Cargo dev-feature rebuild. Two partial-
metadata link attempts are retained as failures; final paired flags succeed.
Two selector and five protocol/metric tests pass. No Rust implementation edited;
full Rust workspace suite not repeated. Exporter/compiled inputs are backed up in
the ignored private artifact cache. Exact proof: `RUST_QUALITY_CORPUS.md`,
`INDEX_RELEASE_ARTIFACT.md` and
`qualification/same-release-rust-quality-2026-10-26/README.md`, `checks.json`,
`summary.json`, pair verification, raw scored cases and unchanged-site audit.
This qualifies requirement8's bounded same-release quality gate and strengthens4;
105 positive misses, broader/compiler semantics, stable repeated release controls,
representative scale/memory and equivalent-work competitors keep the original
full goal open. Shared checkout changes are not attributed here.


2026-10-26 frozen release provenance and larger workflows: build_index_release.py
captures 1880 working-tree files into an independently verified immutable build
snapshot, runs locked/offline optimized Cargo with explicit profile/flag controls,
and retains raw compiler events, source/dependency/tool identities and logs.
verify_index_release.py ties actual output/source/profile/command together; five
corrupted manifests are rejected. Two build guards and three benchmark guards pass.
Frozen release SHA256 dab96adb738e01528410810122a0a7002edfa5ae1090761963be0201be3ea3fe
(index format70) passes actual MCP workflows on itoa and memchr: two modes and five
phases each, twenty Cargo checks and twenty full reference-equivalence audits.
Mutation counts 3/0/1/2/1 and 46/0/1/2/1, same-input replay, ordinary no-op reuse and
release provenance pass. A genuine performance invocation with valid manifest
exits3 for competing desktop/application activity (no compiler in the final sample); no timing/memory advantage is accepted.
A verified ignored-cache archive preserves all source/binary/raw-Cargo evidence.
Production executable unchanged; no runtime implementation edited and full Rust
workspace suite not repeated for this build/harness milestone. Contracts and exact
evidence: `INDEX_RELEASE_ARTIFACT.md`, `REAL_INDEX_BENCHMARK.md` and
`qualification/index-release-provenance-2026-10-26/README.md`, `checks.json`,
`verification.json`, raw Cargo and cohort reports. This advances requirement8's
source/profile/workload gates. Stable quiet repeated controls, representative
memory/scale, same-release independent quality and equivalent-work competitors
remain open, as do 105 original positive misses and remaining compiler/build
semantics. The full original goal stays active.


2026-10-26 real-source index workload harness: a pinned itoa recipe now qualifies
cold/ordinary no-op/body/signature+caller/manifest-metadata phases through actual
isolated MCP with commit-inclusive ingest+index timing. Both incremental and
full-binding-control modes use identical mutation/input receipts; no-op is ordinary
in both. Ten out-of-window Cargo checks and ten full binding equivalence audits
pass. Replay auditing verifies 3/0/1/2/1 changed inputs, revision behavior, source,
recipe, client and frozen format-70 binary identity. Three guard tests pass.
Performance invocation correctly rejects competing compilation/missing release
manifest with exit 3; intentional compiler failure exits 1 and preserves partial
records/stdout/stderr and input integrity. Runtime semantics were not edited and
Rust workspace tests were not repeated. Evidence: `REAL_INDEX_BENCHMARK.md` and
`qualification/real-index-workloads-2026-10-26/README.md`, `checks.json`, `summary.json`
and accepted/rejected/fault reports. Debug/audit-warmed/busy-machine timings and
sampled RSS remain diagnostic only. Requirement 8 stays open for release compiler/
source provenance, stable alternating repeated controls, larger representative
workloads/memory and equivalent-work competitor adapters/runs. The complete
original plan remains active; this is instrumentation/workload qualification,
not a speed, memory or competitor-superiority claim.


2026-10-26 declared union field receivers: formats 69/70 add union nominal
identity/type scopes, generic parameter facts and exact-owner named-field
projection under cfg. Union types cannot be called as value constructors or hide
same-name functions. Three new semantic tests plus a new unknown-field cfg test
and expanded owner/feature cases qualify the subset. Actual MCP checks eight
exact targets across two features, stale exclusion, manifest-only rebinding,
warm revision reuse and history; Cargo executes two passing tests, including a
union and function sharing a name. Fixed sources/config/SCIP/CST/1183 labels and
native/MCP parity improve positive local targets 459/570 to 465/570 (memchr
357/466 to 363/466), with six recovered sites, no lost correct sites and zero
false local targets across positive and 613 external-negative labels. Focused
Rust: 84 passed; strict Clippy and five Python tests pass. A preliminary workspace
run failed in a concurrently changed cursor test; its failure is retained. The
updated workspace's isolated recheck and final full rerun pass: 1528 passed,
zero failed, one ignored; current workspace Clippy passes too. Cursor changes are
not attributed here. Frozen executable identities and raw evidence:
`qualification/rust-union-fields-2026-10-26/README.md`, `checks.json`, `summary.json`,
`runtime.json` and transitions. Shared source is not frozen. Declared union field
types do not prove active members, layout, initialization or unsafe validity.
Remaining 105 positive misses, complete compiler/build semantics, broader
third-party negatives and equal-work release competitor speed/RSS keep the full
original plan open.


2026-10-26 independent external-negative expansion: opt-in oracle contract 2
strictly parses complete SCIP external Method descriptors and requires independent
CST direct-call anchors when fine-grained Kind metadata is unavailable. Default
contract 1 and all 570 original labels/observations/results are unchanged. The
same frozen format-68 server/helper, sources/config/SCIP and native/MCP parity
qualify 613 additional core/alloc negatives: zero false local candidate sites,
608 extracted observations and five missing observations. These are negative
abstentions, not extra correct local targets; positive recall stays 459/570.
Five Python adapter/metric tests pass. No runtime semantics changed and the Rust
workspace suite was not repeated. Exact preservation, final-adapter oracle audit,
source/binary/baseline hashes and raw results:
`RUST_QUALITY_CORPUS.md` and
`qualification/rust-external-negatives-2026-10-26/README.md`, `checks.json`,
`summary.json` and scored cases. This strengthens requirement 4 but broader
third-party negatives/real repositories, compiler/build semantics, the original
111 unresolved sites and equal-work release competitor speed/RSS keep the complete
plan open. Concurrent unrelated work is not attributed here.


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

2026-10-26 PHPUnit discovery: capture format 6 adds bounded direct public instance
TestCase candidates and imported Test attributes with source provenance and
legacy-qualified frozen history. See [contract](PHPUNIT_ENTRYPOINTS.md).
Follow-up implements context-aware affected_tests witnesses and enrolls PHPUnit
direct candidates in recommendations. Shared trait consumers preserve separate
states, witness depth 20 and a shared candidate/context-work budget; missing
frames fail closed. Inherited/trait tests, runner configuration and runtime
coverage remain open. SQL impact is requalified alongside this step.

2026-10-26 effective PHPUnit candidates now include captured base/trait methods,
separate body/host/effective-name identities, aliases/visibility/override guards,
per-state invocation witnesses and marker-2 immutable history normalization.
See [contract](PHPUNIT_EFFECTIVE_TESTS.md). Compiler/framework execution,
runner/configuration, datasets and exhaustive discovery remain open.

2026-10-26 PHPUnit host changes: separately labelled availability witnesses now
join effective tests to changed classes/bases/nested traits. Alias-only Git edits
produce before/after recommendations without fabricated calls or source methods.
Opt-out and independent output/depth/shared-work guards retain the report-only
coverage policy. See [contract](PHPUNIT_HOST_DEPENDENCIES.md).

2026-10-26 first native Python value slices: format 82 captures bounded local
versions and uniquely bound positional argument/return links, queried with
call-site frames and source witnesses through data_slice. This advances the
separate data-flow milestone; full control/heap/alias flow, other languages,
normalized history diff and representative quality/scale remain open.
See [contract](PYTHON_DATA_FLOW.md).

Python binding follow-up (format 83): NFKC identifier keys, function-body local scope and class-body/default evaluation scope are implemented with conservative unsupported-binder guards. See [contract](PYTHON_BINDINGS.md). Remaining: control-flow value merges, broader binding/type/alias semantics, whole-program/CPG qualification and representative corpus acceptance.

Python branch value-flow follow-up (format 84/capture 4): bounded if/elif/else merges and explicit condition dependencies implemented; returned paths cannot supply later locals, incomplete initialization remains unknown. See [contract](PYTHON_DATA_FLOW.md). Remaining: predicate feasibility/correlation, loops/exception/heap/alias flow, complete value-history diff and whole-program/CPG qualification.

Python loop value-flow follow-up (format 85/capture 5): native while/for header/backedge/exit candidates with zero iterations, break/continue/else, abstract items and source-bound checked edge contexts implemented. Plain for-target scope capture upgraded to format 2. See [contract](PYTHON_DATA_FLOW.md). Next gaps: exceptions/finally, complete CFG, path/iteration feasibility, heap/alias flow, value-history diff and broader corpus/CPG qualification.

Frozen Python value-history follow-up: normalization format 1 compares compatible capture-5 immutable graphs, separating node syntax fingerprints, local/cyclic links and uniquely qualified positional call/return links. Source offsets/comments/NFKC spelling normalization, unsupported/incomplete/inventory/provenance guards, scope isolation and bounded collections are implemented. See [contract](PYTHON_VALUE_DIFF.md). Remaining: semantic/runtime diff proof, minimal edit matching, complete CFG/exceptions/alias flow, broader languages and full CPG/corpus acceptance.

Python finally follow-up (extraction 86/value capture 6): explicit completion routing, saved-return versus updated-local separation, conservative protected error prefixes, distinct finalizer invocation/call contexts and one-physical-definition context-union selectors implemented. Frozen normalization 2 accepts capture 5/6. See [contract](PYTHON_FINALLY_FLOW.md). Remaining: except/type matching, try-else, bare/chained raises, full CFG and exceptional uninitialized reads, heap/alias/type semantics, predicate/iteration feasibility/termination, broader corpus/language/CPG acceptance.

Python exception follow-up (extraction 87/capture 7): try/except/else/finally candidates, bare/typed handler routing, captured re-raise, exception alias cleanup and normal-state joins implemented with explicit unproven type/filter semantics. Scope capture 3 shares the alias AST adapter and cleanup masks. Normalization 3 accepts captures 5..7 with contextual model-change exclusions. See [contract](PYTHON_EXCEPTION_FLOW.md). Next gaps: exception class/filter matching and dynamic/grouped raises; exceptional uninitialized reads; Python assignment-RHS evaluation-order/type refinement; complete CFG/heap/alias/compiler semantics; broader languages and representative CPG/corpus acceptance.

Python assignment-order follow-up (extraction 88/scope capture 4): RHS-before-write activation, prior receiver preservation, annotation-only classification and augmented-result unknown bindings implemented and reproduced against the prior failing case. See [contract](PYTHON_ASSIGNMENT_ORDER.md). Next: branch/loop-sensitive receiver type alternatives, general alias/destructuring and heap effects, exception type/filter proof, broader language/corpus/CFG/CPG qualification.

Python receiver-origin follow-up (extraction 89/scope capture 5): bounded branch/loop origin joins, returned-arm exclusion, local aliases, inherited target deduplication and partial-known uncertainty integration implemented. See [contract](PYTHON_RECEIVER_JOINS.md). Next gaps: precise exceptional/closure/default receiver environments, broader destructuring and heap/alias/type inference, runtime subtype contexts, representative corpus/scale and full compiler/CFG/CPG qualification.
