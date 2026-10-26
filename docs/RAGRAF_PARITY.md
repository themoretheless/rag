# RAGRAF parity implementation

This tracks the requested RAGRAF feature expansion. A checked item denotes
implemented source, not deployment to the running gateway or whole-product parity.

## Stage 1: syntax index and call candidates

- [x] Tree-sitter parsing: Rust, Python, JavaScript/JSX, TypeScript/TSX, Go, Java, C and C++ (qualified scopes below).
- [x] Persistent DuckDB syntax snapshots, additive schema v18.
- [x] Reuse unchanged parses; transactional corpus replacement; prune deleted inputs.
- [x] Source evidence: document, path, line, byte span, revision, content hash.
- [x] MCP `index_code`, `symbol`, `references`, `callers`, `callees`, `code`, `repo_map`, `impact`.
- [x] Explicit stale/unindexed/parse-error reporting; stale facts excluded.
- [ ] Expand grammar coverage to the remaining RAGRAF languages.
- [x] Conservative import/alias resolution and lexical binding for supported languages.
- [x] Annotated receiver types, simple type aliases, constructor-shaped assignments, self/this.
- [x] Explicit Python/JS/TS base classes, inherited method lookup, overrides and super.
- [x] C3 lookup for Python multiple inheritance; cycles/unknown or inconsistent bases fail closed.
- [x] Same-document Rust single-trait default-method lookup (not class inheritance).
- [ ] Full compiler type inference, package manifests, re-export chains and runtime dispatch.
- [ ] Dedicated indexing for Rust closures and anonymous declarations (named JS/TS arrow/function variables are indexed).
- [x] Index refresh as part of source-sync finalization, including unchanged and deletion-only syncs.
- [ ] Normalized symbol/reference tables and indexed SQL queries for large corpora.

### Usage

Start the gateway with `RAG_TOOLS=full`. Ingest source files using the existing
allowlisted `ingest_file` / `sync_sources` routes. Source sync automatically refreshes
the syntax index before terminal success; after direct ingest, use `index_code`
(admin role). The index reads stored documents, never the filesystem and never an LLM.
Sources must carry a supported extension in `source_file`, falling back to URI.
Archived, consolidated and tombstoned documents are excluded.

Example MCP arguments:

```json
{"name":"index_code","arguments":{}}
{"name":"symbol","arguments":{"symbol":"work","path":"src/service.rs","limit":20}}
{"name":"callers","arguments":{"symbol":"work","path":"src/service.rs"}}
{"name":"callees","arguments":{"symbol":"Service::run"}}
{"name":"references","arguments":{"symbol":"Service::run"}}
```

Use a returned symbol ID to disambiguate definitions. IDs identify byte positions
within a document; edits can change them. Evidence includes the revision and hash.
`code` requires a single unambiguous definition and returns its body only on this
explicit operation; it verifies freshness again before returning the body.
`repo_map` accepts `budget_chars` (128..20000) and `limit` (1..500).
`impact` accepts `depth` (1..10) and `limit` (1..500), walks incoming call candidates,
handles cycles, and reports output/depth truncation. It is an overapproximation,
not proof of actual breakage.

`references` defaults to statically bound occurrences, including imported aliases,
callbacks, qualified member values and type uses. Its `path` selects definitions;
matching usages can occur in other files. `reference_mode="lexical"` explicitly
selects name-only matches. Bound records retain the flat source occurrence fields
and add `candidates`, `resolution`, `resolution_basis` and derived provenance.
A static binding does not prove runtime behavior. Unresolved occurrences are not
assigned to a same-name definition; ambiguous bindings retain all candidates.
`absence_proven` remains false, including for empty results.
`callers` and `callees` return call sites with candidate target IDs and
`resolution_basis` (`lexical`, `import`, `receiver_type`, `qualified`, `module_path`,
or an unresolved/shadowed reason). `resolved_static` means one definition survives
the supported syntax-binding rules; `ambiguous` means several survive; `unresolved`
means no safe indexed target. Every link retains `provenance=derived`: static
binding is not proof of runtime dispatch.

Supported bindings:

- Rust `use`, aliases/grouped imports, `crate`/`self`/`super` paths, file and inline
  modules below a `src` root, annotated parameters/locals, simple type aliases,
  `self` and `Self` receiver types.
- Python explicit `from` imports with aliases, relative modules, module imports,
  annotated parameters/locals, class-shaped assignments and `self`.
- JS/TS relative named/default/namespace imports, direct named/default exports
  and local export aliases, annotated receivers, simple TS type aliases, named
  arrow variables, `new` expressions and `this`.

Local parameters/variables shadow imported/global names. There is no fallback to
unrelated same-name files. Candidate targets must have the same stored project
(`wing`) as the caller. Explicit relative/path resolution is also required;
project metadata alone never makes names resolve globally.

Not yet resolved: runtime dispatch, wildcard imports, re-export chains, CommonJS,
external package metadata, tsconfig aliases, Python runtime import paths, Rust
macro/cfg expansion, arbitrary return-type inference, complex/generic/union types,
dynamic dispatch and arbitrary alias assignments. Without captured manifests,
Rust modules retain the conventional `src` layout fallback. Captured `Cargo.toml`
now adds partial target roots, declared modules, ordinary `#[path]`, custom crate
names and renamed/inherited path dependencies; see [Rust Cargo graph](RUST_CARGO_GRAPH.md).
Feature/cfg configuration, full Cargo semantics and compiler-grade resolution remain open.
Rust member calls now support a qualified AST subset of declared-return chains,
await, named fields, array/slice/Vec elements, standard Box/Arc/Rc and explicit
generic return arguments; see [receiver typing](RUST_RECEIVER_TYPES.md). This does
not close arbitrary inference, traits/generic impls or the broader quality gates.
Constructor-shaped Python assignments are a syntax heuristic, not a runtime
guarantee. Class bodies retain lexical ownership rather than a runtime execution
model. Bound references reuse these conservative binding rules. Import declarations
link through the locally imported name; module tokens and the original name of
an aliased import do not fall back to unrelated local definitions. Object/member
reads retain the full expression, rather than treating a property name as a bare
function. Arbitrary value-alias chains, dynamic computed properties, destructured
bindings, custom compiler namespaces and unsupported languages remain incomplete.
Import-local token lookup is bounded to 4096 AST nodes per declaration. This is
not a complete compiler reference index; lexical mode remains available for
unresolved text candidates.

Files with parser errors retain recoverable facts and appear in `parse_errors`.
Traversal also stops at AST depth 256 and reports degraded coverage in that list.
Queries report stale/unindexed document IDs. Sync, background auto-sync and watcher
sync refresh the cache automatically, under the sync mutation lane, after FTS
finalization and before completion. Changed files are reparsed; unchanged parses
are reused; deleted sources are pruned. Old cache formats are invalidated and
rebuilt on sync or `index_code`. Direct document writes still need `index_code`.
Cancelled/interrupted syncs may leave stale facts, which queries omit.

Sync reports expose `code_index` counters and `code_index_error` on failure. A
`CODE_INDEX_FINALIZATION_FAILED` report retains committed source counters and
sets the job's existing errors outcome; it is never normal terminal success.
`durable_mutation_committed` distinguishes already-written sources from a failed
cache refresh. Retry `index_code` after correcting the cache error; do not replay
source writes blindly. FTS and code-index errors are reported independently. A successful empty result does not prove absence when
coverage is incomplete. The snapshots are derived data and can be rebuilt after
recovery/import; existing recovery bundles do not carry the syntax index.

The first implementation caches one syntax payload per source document. Queries
validate source hashes and assemble the graph in memory; this is not yet a
large-repository performance claim. No live user database is opened by tests.

## Inheritance and code navigation

`inheritance` lists `extends` / `implements` evidence with source path/line,
resolved base IDs and `hierarchy_status`. A relation can name an indexed base yet
belong to an invalid hierarchy: `incomplete_or_invalid` covers unknown bases,
cycles, inconsistent C3 ordering and traversal limits. Source revisions/hashes
remain available on the associated class symbols. Reindex old payloads with sync
or `index_code`; cache format changes force fresh parsing.

Python class bases and JS/TS `extends` resolve through the existing imports and
project scopes. Own methods take precedence; inherited lookup follows C3 order
(single-base order for JS/TS). `super().method()` and `super.method()` skip the
current class. Explicit fields and Python `@property` / `@cached_property` block
fallback to a base method. Unknown/dynamic bases and invalid MRO do not produce a
unique inherited target. C3 recursion is bounded. Python's builtin `object` is a
terminal root when it is not an indexed type. Rust support is limited to
same-document concrete types with one explicit trait implementation and a default
method body; it excludes generic/coherence/trait-object resolution. All targets
remain static derived facts, not runtime dispatch guarantees. Instance attribute
mutation, descriptors, decorators and prototype mutation can change runtime
behavior and are not modeled.

New tools are on the full MCP surface:

```json
{"name":"inheritance","arguments":{"symbol":"Child"}}
{"name":"entrypoints","arguments":{}}
{"name":"routes","arguments":{"path":"/project/server.py"}}
{"name":"dead_code","arguments":{"limit":50}}
{"name":"slice","arguments":{"symbol":"Child::run","direction":"back","depth":3,"limit":100}}
```

`entrypoints` detects conventional top-level `main`, JS/TS exports and their
members, Rust public functions, Python `test_*` functions, Rust test attributes,
and recognized HTTP route handlers. These are syntax candidates. Framework
registration currently covers Python Flask/FastAPI-style decorators, Express-style
named handlers and simple Axum `.route(path, get(handler))` registrations.
`routes` retains unresolved/anonymous handler records with `symbol_id=null`;
registration paths and HTTP-method evidence are returned even without a target.
Only literal paths are recognized; mounts, dynamic paths, complex middleware,
router aliases and framework configuration are not fully expanded.

`dead_code` walks the static graph from these roots and top-level calls. All
candidate edges are included to avoid dropping ambiguous reachability. Bound value references (e.g. imported callbacks) also seed conservative
reachability, so callees reachable through such a callback remain live. Routines
with unresolved identifier-value name references are still excluded conservatively.
Results have `candidate_only=true`, `external_usage_unknown=true` and
`deletion_safe=false`; external consumers, reflection and unknown registration
can keep an apparently unreachable routine live. No automatic deletion is done.

`slice` is a structural call subgraph, not a data-flow/Joern program slice.
It requires a symbol, accepts `back`, `forward` or `both`, caps depth at 10 and
nodes at 500, and limits returned edges to 2000. It reports node/depth/edge
truncation, source lines and incomplete coverage. It never returns function
bodies; use `code` explicitly. All these analyses omit stale facts and report
missing/stale/parse-error coverage; `analysis_incomplete` also reflects unresolved
calls and bases.

## Remaining RAGRAF feature groups

- [x] Symbol body reads (`code`) and budgeted repository maps (`repo_map`).
- [x] Bounded impact traversal with evidence and explicit heuristic links.
- [x] Initial entrypoint/route candidates, reachability and conservative dead-code candidates (supported patterns below).
- [x] Bounded structural call slices with no function bodies.
- [ ] Data-flow/program slicing and optional richer interprocedural backend.
- [x] Initial SQL schema extraction from migration DDL, static table read/write usage and write checks.
- [ ] Migration-runner/ORM DSL coverage, applied-version order, full dialect constraints and live schema verification.
- [ ] External database schema/relations/samples/drift/cross-database analysis.
- [ ] Permission-policy traces and CSS definition/application analysis.
- [x] Full-text multi-string literal/regex all/any search with per-query counts.
- [x] Exact/near duplicate families and static primary-file evidence.
- [ ] Complete import/markup/dynamic file-liveness resolution and cross-engine performance comparison.
- [x] Atomic immutable index revisions, source blob deduplication and semantic file/symbol/call diff.
- [x] Allowlisted live reads, filesystem/stored checksum freshness and persistent drift/ack ledger.
- [ ] Large-repository history retention/pagination refinements, richer edit hunks and portability of derived history.
- [ ] Per-fact provenance/confidence and explicit capability/degradation envelope.
- [ ] Tool-call gate (`allow`/`ask`/`deny`), task context and per-task statistics.
- [ ] Confidence calibration and tool-effect estimates.
- [ ] MCP resources/prompts and integration/documentation parity.

Existing generic search, wiki, diary, graph, access roles, telemetry and retrieval
evaluation remain foundations, not substitutes for these code-specific contracts.
Reference: https://github.com/paulmann/RAGRAF (reviewed 2026-10-23).

## SQL analysis

`schema`, `table_usage` and `write_check` are implemented on the full MCP surface.
SQL files share the persistent syntax cache and source-sync refresh; static SQL
literals carry source/symbol evidence. Migration DDL replay and write checks fail
closed on incomplete or ambiguous schemas. See [SQL_CODE_ANALYSIS.md](SQL_CODE_ANALYSIS.md)
for supported changes, rollback conventions, examples and exact validation scope.

## Search and index history

`search_multi`, `duplicates`, `index_revisions`, `diff`, `catalog`, `index_status`,
`freshness`, `live_read`, `drift_report` and `drift_ack` are implemented.
See [CODE_HISTORY.md](CODE_HISTORY.md) for consistency, evidence, limits and access
contracts. The full RAGRAF goal remains open; this checklist does not substitute
for end-to-end parity or superiority qualification.

## CSS classes and permission decision syntax

`class_usage` and `policy_trace` are available on the full MCP surface. CSS/HTML
syntax grammars and basic PHP declaration/call parsing extend the index. Remaining
language/compiler/control-flow qualification is still open. See
[CSS_POLICY_ANALYSIS.md](CSS_POLICY_ANALYSIS.md) for supported syntax, project
isolation, fallback semantics, uncertainty and budgets.


## Bound references qualification

Syntax-cache format 13 captures qualified non-call occurrences and type uses.
Sync or `index_code` rebuilds earlier cache formats. Immutable revision graphs
freeze bound reference evidence alongside their call/entrypoint/inheritance data;
the current diff API still compares its documented call edges and definitions,
not arbitrary reference relations.

The [local qualification](qualification/bound-references-2026-10-24/README.md)
checks alias callbacks, same-name/project separation, inherited method values,
parameter/default/rest shadowing, lexical fallback, deterministic limits and stale
input omission. The actual stdio MCP fixture verifies automatic reference refresh
after source sync and preservation of transitive callback liveness. Remaining
languages, non-code relation categories and broad compiler/runtime/reference
comparison gates remain open in the feature matrix.


## Go packages, methods and module imports

Go declarations now participate in `symbol`, `code`, `repo_map`, `references`,
`callers`/`callees`, structural slices and reachability. Functions, receiver
methods (`Service::Run`), interfaces/method declarations, named types and aliases
retain original spans, signatures and hashes. Same-directory/same-package binding
works across files within one stored project. Local variables, grouped/variadic
parameters, constants, type parameters and local type declarations shadow package
and imported names. Local type references bind to the local declaration.

An ingested `go.mod` is part of the derived index and immutable revision inputs.
It supplies the own module path; source sync includes this exact filename by
default. Imports can resolve to captured packages inside that module. Explicit
aliases are respected; default imports use the captured package declaration, not
the last path component. Nested module boundaries and project IDs prevent a
fallback into unrelated packages, including documents with the same source path.
No filesystem, Go command, dependency download or network request is used while
indexing/querying. Missing, stale, malformed or oversized module declarations
cannot supply an import root. Unindexed/stale nested manifests retain a blocking
boundary so the parent module cannot claim their packages. Quoted/raw module
paths and line comments are
supported; forbidden block comments and duplicate module declarations are rejected.
This is a bounded own-module declaration reader, not full `go.mod` validation.

Annotated and simple composite-literal receivers bind direct methods across
package files; simple alias chains are bounded to 16. Value receiver method
expressions do not acquire pointer-only methods. Interface calls target the
captured **interface declaration** (`go_interface_contract`), not a guessed
implementation. Exported declarations, conventional package-main `main`, `init`
and test-name candidates are entrypoint evidence. Test signatures are not fully
validated by a Go compiler.

Production files do not bind to `_test.go` definitions. Filename platform suffixes,
build tags, compiler-ignored filename prefixes and cgo are reported as unselected
build inputs, and do not provide confident binding targets. Syntax definitions
remain inspectable. Anonymous function ownership, generic instantiation, embedded
method promotion, dot imports, imported initialization effects, arbitrary return /
callback inference, pointer alias/method expressions, local-type method lookup,
external dependency versions, `replace`, vendoring and `go.work` remain incomplete.
Unknown cases retain unresolved evidence rather than guessed same-name edges.

Syntax-cache format 14 rebuilds earlier caches on sync/`index_code`. Changing only
`go.mod` recomputes graph bindings over reused Go syntax and publishes a new
immutable revision. [Go qualification](qualification/go-index-2026-10-24/README.md)
contains the real MCP module-change/staleness checks and test evidence. This is
not complete Go compiler analysis or whole-RAGRAF acceptance.


## Java declarations and static candidates

Java classes, interfaces, enums, records, annotation types, methods and explicit
constructors now participate in the source tools. Package headers and captured
single/wildcard/static imports supply source-only type candidates within the same
stored project. Comments inside qualified names/imports are handled through AST
nodes. Unavailable explicit imports do not fall back to unrelated same-name types;
identical source paths in different projects retain their own symbol IDs.

Annotated parameters, local variables, simple `var = new Type()` declarations and
own fields (including `this.field`) supply receiver types. Method candidates are
narrowed by argument count, including varargs; equal-arity overloads remain
ambiguous. Known signatures suppress overridden declarations while retaining
inherited overloads. Super calls and explicit constructor delegation have separate
lookup paths. Class methods take precedence over matching interface declarations.
Interface calls identify the static contract, not a guessed implementation.
Missing/ambiguous bases, cycles and hierarchy budgets prevent a unique verdict.
The `inheritance` view reports captured extends/implements evidence and structural
hierarchy completeness; this does not prove that javac accepts the program.

`object::method`, unbound `Type::method` and `Type::new` retain non-call reference
records. No target SAM typing is inferred, so constructor/method-reference
overloads can remain ambiguous even where a compiler could choose one.
Main/test annotations are **candidates**, not proof of a runnable program or JUnit
execution. Anonymous/lambda and enum-constant subclass ownership stays unmodeled;
its calls do not receive guessed outer-method targets. Record component bindings
remain inside their owner, and enhanced-for variables do not shadow the iterable
expression. Generated record accessors and implicit constructor bodies are not
fabricated. A zero-argument default construction may link its declared class with
`java_implicit_constructor_type` instead of an invented constructor symbol.

The envelope explicitly reports `JAVA_CLASSPATH_AND_RUNTIME_DISPATCH_UNVERIFIED`.
This is not full Java compiler name/access/overload resolution. JDK/dependency
classes must be captured to have definitions; Maven/Gradle/JPMS/classpath selection,
generics/erasure, argument conversions, most-specific interface defaults,
protected/module access, pattern variables, inherited/chained fields, outer-instance
method lookup, factories, reflection, generated code and optional Joern data-flow
remain outside the qualified subset. The lexical reference mode remains available.

Ordinary Java quoted SQL literals use the existing bounded static SQL analyzer.
Java SQL text blocks are explicitly unverified until indentation/escape decoding
is implemented; they do not publish guessed table names.

Syntax-cache format 15 rebuilds earlier parses on sync or `index_code` and freezes
Java evidence in revision graphs. [Java qualification](qualification/java-index-2026-10-24/README.md)
includes actual MCP constructor/overload/reference/hierarchy checks and source-sync
rebinding after a captured base changes, plus the Go regression fixture.


## C/C++ declarations and captured include graphs

C and C++ now contribute source functions, prototypes, struct/class/union/enum
bodies and named namespaces. Declarator traversal distinguishes a function from
a variable holding a function pointer, including pointer return types. Prototypes
use `function_prototype`, preserving source evidence without claiming a body or
reporting a declaration as a dead routine. Operators/destructors retain source
symbols; full operator dispatch remains unqualified. Templates and anonymous type
scopes never become unrelated free functions.

Quoted includes are resolved only against captured exact relative source paths in
the same project. The include graph is bounded to 128 files. Includes after a call
do not make earlier declarations visible. Conventional top-level header guards
are recognized as a source convention; this is an assumption, not evaluation of
build macros. Angle/macro includes and missing/stale headers cannot supply guessed
search roots. `.h` uses C parsing first and switches to C++ only when that removes
parser errors; `.hh`/`.hxx` are also ingested as C++ sources. This is a syntax
choice, not compilation-database language selection.

Visible prototypes can link to same-project external function bodies; `static`
functions in unrelated translation units/projects are excluded. Potentially
conflicting definitions and equal-arity C++ overloads remain ambiguous. Function
pointer calls stay unresolved while direct callback-value references retain
symbol binding. C empty parameter lists remain unspecified; a C-compatible header
is interpreted with zero-argument prototype rules when used in a C++ source view.
Default argument declarations can select a body without reinterpreting its source.

Annotated pointer/reference receivers bind captured C++ methods, including simple
base lookup. C++ member-name hiding precedes free functions and base overloads.
Own fields prevent a fallback to a same-name base method. Multiple-inheritance
subobject identity, templates, access control, virtual dispatch, aliases/using,
argument conversion and pointer-to-member inference remain incomplete. The
`inheritance` view labels bases `structural_bases_only`; it does not claim a full
C++ MRO or compile acceptance. Lambda ownership is unmodeled. Zero-argument type
creation can link its class with `cxx_type_construction_without_body`, never an
invented constructor implementation.

The envelope includes `C_FAMILY_PREPROCESSING_BUILD_AND_LINKAGE_UNVERIFIED`.
Visible macros prevent direct function-name claims; conditional declarations do
not receive a unique verdict. No preprocessor, linker or compilation database is
run by the index. Qualified source links do not prove target selection for a real
binary. Include search paths, macros/cfg, ABI/language linkage, inline/ODR rules,
modules, generated members, initializers, semantic template/overload resolution and
representative performance remain outside the qualified subset.

Ordinary C/C++ quoted SQL uses the existing static literal checks. C++ raw-string
SQL decoding preserves its delimiter, raw backslashes and physical line evidence,
including encoding prefixes. Unmodeled escaped/concatenated forms remain diagnosed.
Syntax-cache format 16 rebuilds earlier parses on sync/`index_code` and freezes
new source evidence in history. [C/C++ qualification](qualification/c-family-index-2026-10-24/README.md)
contains library checks, actual MCP stale-header/rebinding cases and independent
Clang syntax-only positive/negative probes. This does not close the full feature
matrix or establish deployment.

### Ruby syntax transfer

[Ruby code analysis](RUBY_CODE_ANALYSIS.md) adds captured `.rb` declarations, owner-qualified method/constant candidates, conservative constructor locals, require-relative file scopes, single-superclass candidates and frozen/cache/sync integration. This closes part of the remaining language gap; runtime dispatch, framework and full language qualification remain open.

### C# syntax transfer

[The C# index](CSHARP_CODE_ANALYSIS.md) now captures namespaces, type/member declarations, calls and base syntax, with conservative namespace/alias/type/arity candidates. Source sync, stale guards, impact, cache equivalence and frozen base history are integrated. Compiler/build/overload/runtime conformance and the remaining language matrix stay open.

### Perl syntax and variables transfer

[Perl code intelligence](PERL_CODE_ANALYSIS.md) uses the maintained grammar for package/sub/class/method syntax and scalar/array/hash declarations and references. Captured package/lexical calls, single-parent candidates and package-file relations integrate with source sync, stale guards, impact, cache equivalence and immutable history. INC/exports, dynamic/runtime/experimental forms and complete parity remain open.

### PowerShell syntax transfer

[PowerShell code intelligence](POWERSHELL_CODE_ANALYSIS.md) adds functions/classes/methods/properties/enums, captured case-insensitive command scopes, literal source-load relations and static/this/single-base candidates. Source sync, stale guards, impact, cache equivalence and immutable history are integrated. Ambient session/alias/scope/export, full language/runtime behavior and complete parity remain open.

### Duplicate candidate role evidence

[Duplicate role contract 2](DUPLICATE_ROLE_EVIDENCE.md) extends primary-candidate ranking with bound values/types, module imports and HTML/CSS resource candidates. Source and target fingerprint/project guards, non-code published identity, complete evidence counts, deterministic bounded retention and primary visibility under member limits are integrated. Runtime activity, representative ranking/scale and complete parity remain open.

### Search family roles and coherent observations

[Search role evidence](SEARCH_ROLE_EVIDENCE.md) now includes captured imports/resources and non-code identities outside target prefixes. Search and duplicates share a scoped corpus/index read transaction, retain independent evidence counts/caps and validate source fingerprints. A cloned-connection source/cache/history publication case qualifies read-view coherence; global ranking, runtime usage, representative scale and complete parity remain open.

### Literal path mention roles

[Literal path role evidence](LITERAL_PATH_ROLE_EVIDENCE.md) now supplements search/duplicate rankings with exact stored-text mentions such as JS endpoint constants. Same-project published identities, global basename guards, path/URL boundaries, typed-pair suppression and explicit work/retention budgets preserve uncertainty. Runtime path binding, representative quality/scale and complete parity remain open.

### Duplicate member comparison (2026-10-26)

Returned duplicate members now include normalized unique-line Jaccard against
the selected primary and both directional unique-line deltas. The phase has
independent pair/byte/line budgets, explicit null metrics for unchecked pairs,
source identities and body-free output. It runs after returned group/member
limits and does not change token-trigram grouping. See
[DUPLICATE_PRIMARY_COMPARISON.md](DUPLICATE_PRIMARY_COMPARISON.md) and
[qualification](qualification/duplicate-comparison-2026-10-26/README.md).
Full RAGRAF parity, runtime binding/deletion safety and representative quality
and scale remain open.

### Independent duplicate family inventory (2026-10-26)

`duplicates(grouping="exact_and_name_families")` now inventories directory- and
case-sensitive numbered filename families even with dissimilar content, alongside
independent exact stored-text partitions. Proper exact subgroups remain visible;
only identical member sets suppress redundant family groups. Returned families
classify as `family`, `same_name` or explicit `not_checked`, with bounded primary
comparison coverage and unrounded threshold decisions. The default component
view remains available. See [contract](DUPLICATE_FAMILY_GROUPING.md) and
[qualification](qualification/duplicate-families-2026-10-26/README.md).
Representative corpus quality/scale, broader runtime roles and full parity remain
unverified; classification does not imply semantic equivalence or deletion safety.

### Captured source-byte duplicate identity (2026-10-26)

`duplicates(grouping="source_byte_copies")` adds project-scoped captured BLAKE3
source digest/length partitions, separately from decoded/extracted text copies.
All duplicate modes expose qualified capture evidence or explicit unknowns per
member. Text decoding and PDF/OOXML extraction now bind to exactly the captured
input; binary document extractors read a temporary snapshot. Source sync upgrades
legacy metadata and refreshes byte-only changes with unchanged decoded text.
[Contract](SOURCE_BYTE_DUPLICATES.md),
[qualification](qualification/source-byte-copies-2026-10-26/README.md).
Direct byte comparison, current filesystem equality, arbitrary binary ingestion,
representative scale and full RAGRAF parity remain unverified.

### Non-code reference targets (2026-10-26)

`references` now accepts explicit SQL table or literal URL targets in addition to
existing symbols. Project-scoped snapshots, schema/project ambiguity before
filters, captured read/write owner or ownerless-file evidence, and separately
qualified route registrations/client mentions cover previously absent target
kinds. Handler alternatives at one registration are grouped and do not become
fake source owners or runtime dispatch proof. Counts precede output limits;
stale facts remain excluded. [Contract](NON_CODE_REFERENCES.md),
[qualification](qualification/non-code-references-2026-10-26/README.md).
Translation keys, remaining non-code kinds, broader surfaces and representative
quality/scale remain open; this does not close the full parity goal.

### Translation-key references (2026-10-26)

Persisted `uses_key` candidates now link raw PHP/PHTML/HTML placeholders with
literal PHP translator-name calls through `references(key=..., project=...)`.
Exact decoded keys, quote-aware static PHP escapes, UTF-8 source provenance,
per-project identity and explicit capture limits improve the reference's raw
stripping/line-dedup behavior. Frozen relations preserve repeat counts, ignore
position/literal-spelling churn and refuse fabricated comparisons against
legacy or incomplete captures. [Contract](TRANSLATION_KEY_REFERENCES.md),
[qualification](qualification/translation-keys-2026-10-26/README.md).
Catalogue/locale/framework resolution, dynamic/named PHP forms, broader formats,
shared-key symbol listing, remaining surfaces, representative quality/scale and
full parity remain open.

### Shared captured symbol nodes (2026-10-26)

`symbol` now exposes typed translation-key, SQL table and URL identities alongside
its unchanged default code-definition view. Project-scoped stable IDs, prefix/name
lookups, literal contains selection, source-window counts and independently bounded
proofs provide discoverable navigation through `reference_target`. Shared nodes
have no invented last-writer definition location; schema/use-only tables and
route handler alternatives remain explicit. [Contract](SHARED_SYMBOL_NODES.md),
[qualification](qualification/symbol-nodes-2026-10-26/README.md).
Catalogue/framework binding, unresolved external placeholders, broader domains
and surfaces, ranking/scale and full parity remain unverified.

### Literal target impact and invalid-source traversal guard (2026-10-26)

`impact_key`/`impact_route` connect captured key/URL syntax to separate bounded
code and multi-root file graphs, keeping ownerless templates and all seed roots
before output caps. Route handlers remain alternatives, never fake source owners.
Shared symbol nodes provide impact selectors. A reproduced JS parse-error bridge
is now excluded in the common symbol walker; SQL schema-only identities also
participate in disambiguation before source/access filters.
[Contract](LITERAL_TARGET_IMPACT.md),
[qualification](qualification/literal-impact-2026-10-26/README.md).
Runtime binding/impact, unsupported import/ownership forms, broader surfaces and
representative quality/scale remain unproven; full parity remains open.

### PHPDoc parameter contracts

PHP/PHTML named callable symbols now retain structured stated parameter contracts, nested/optional shapes, AST parameter binding and explicit unsupported/ambiguous/limited evidence. See [contract and remaining boundaries](PHP_PARAMETER_CONTRACTS.md). Runtime type checking, inherited contracts, dialect precedence and semantic contract diffs remain open; this does not close whole-product parity.

### Client HTTP endpoint syntax

[Qualified contract](CLIENT_ENDPOINT_REFERENCES.md) extends fetch evidence with AST jQuery/Axios URL candidates, explicit method basis and legacy-aware immutable relations. Runtime client identity/defaults, library instances, XHR and relative URL resolution remain open.

### Stored code read windows and ID navigation

[Contract](CODE_READS.md) adds project-scoped file/document/ID reads, inclusive line windows and shared text budgets. Nested/partially overlapping source intervals are returned once, and chunk IDs require exact current source mapping. One read transaction binds selection to source identity. Broader source-mode/style/transformed-chunk and scale acceptance remains open.

### Complete traversal of bounded source reads

[Source cursors](CODE_SOURCE_PAGINATION.md) resume record/character/segment pages against a checked current view. UTF-8 byte windows and delimited IDs extend navigation; canonical partitions preserve unique bytes across pages. This improves large/single-line source reads while leaving historical snapshots, other source modes, styles and representative scale open.
