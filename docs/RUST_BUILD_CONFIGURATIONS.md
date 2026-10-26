# Captured Rust build configurations

Index format 33 adds an explicit configured graph view. Normal Rust extraction
remains raw and reusable. Both current snapshots and frozen revision publication
apply the same captured view; historical queries do not read live manifests.

Opt in per package using captured `Cargo.toml`:

```toml
[package.metadata.rag.graph]
version = 1
target_kind = "lib" # lib, bin, test, bench, example
# target_name = "custom_name" # required when kind alone is ambiguous
features = ["fast"]
default_features = false # otherwise true
cfg_flags = []
cfg_values = { target_os = ["linux"] }
# target_triple = "x86_64-unknown-linux-gnu"
```

This selects **one compilation target per package**, not every compilation unit
that Cargo would build. A selected bin/test does not automatically instantiate
its package's library as another unit. Target kind/name must resolve uniquely;
required features are checked. Local features expand transitively, including
`default` when enabled, and supported optional-dependency activation is captured.
Dependency feature forwarding is reported as unverified; Cargo resolver-version
rules and dependency-wide feature unification are not implemented.

Direct `#[cfg]` predicates support `all`, `any`, `not`, `feature`, bare flags and
asserted cfg-value sets. Bare flags are a complete asserted set: omitted flags
are false. An omitted cfg-value key is unknown; an explicitly supplied value set
is complete for that key. A target triple is an identity label, not a source of
inferred compiler cfg values. The indexer never borrows the indexing host's cfg.
False and unknown declarations are excluded from the configured view.
`cfg_attr` predicates now preserve inactive attributes, support nested `cfg` and
`cfg_attr`, and admit known identity/lint attributes. Active unknown attribute
macros remain unknown. Inner attributes gate their owning scope. Empty compiler
cfg values (for example `target_env=""`) are valid; each asserted value set is
bounded to 128 entries, admitting host target-feature sets larger than 32.
Conditional public imports use the same declaration-level filtering as private
imports: inactive bindings disappear, unknown bindings retain a shadow without
a guessed target. Captured Cargo-local public re-export chains now follow the
bounded rules and remaining limits in [RUST_REEXPORTS.md](RUST_REEXPORTS.md).
Formats 53/54 additionally project conditional call arguments,
statement expressions and blocks: inactive or unknown references and semantic
declarations are excluded without
blocking unrelated bindings in the file. Field types carry captured positions;
inactive fields cannot supply receiver types. Unknown conditional assignments
retain their invalidations, because they may execute. Argument values/arity are
not used for Rust generic or overload inference. Enclosing permission-decision
syntax is suppressed when a conditional argument, statement or block is false or
unknown; its
control-flow projection remains unmodeled. Other active nested conditional forms block typed
binding instead of guessing. Unsupported descendants within a known-disabled
projectable owner do not block live bindings elsewhere in the file. Unknown input keys, invalid input, stale boundaries,
ambiguous manifest identities and unavailable targets fail with diagnostics.
Unverified configurations do not publish permission, SQL, route or inheritance
facts. Raw source definitions may remain as unverified evidence.

The test/not(test) preparation overlay preserves unresolved feature/platform
imports until the actual captured build configuration is evaluated. It no longer
irreversibly clears their targets before feature selection. Raw extraction facts
are unchanged by these query overlays.

Selected source symbols and references carry `build_context`; symbol and caller
IDs include the configuration hash. The hash includes captured manifest identity
and content, project, input selection and resolved local features. Body reads
retain original document identity, source hash and spans. Existing policy, SQL
and semantic owner IDs are remapped together. Files outside the selected target
are excluded. The underlying extraction cache remains raw, so changing only the
manifest reuses Rust parses while recomputing binding and publishing a revision.

`index_status.rust_build_contexts` and code-query envelopes expose bounded context
evidence with counts and truncation. Immutable `diff.rust_build_contexts` reports
before/after contexts; symbol keys contain build identity. Legacy revisions with
no captured context evidence report it unavailable. The older `cargo_targets`
capability flags describe the coarse Cargo model and do not imply compiler-wide
feature evaluation; inspect the explicit context's scope and limitations.

Remaining requirement-1 work includes multiple units sharing a source, automatic
library dependencies of binaries/tests, dependency feature unification, full
workspace membership, target-specific dependency selection, build scripts and
macro/include expansion. This is not universal rustc semantics. Quality on real
repositories and performance comparisons remain requirements 4 and 8.

Qualification: [captured configuration evidence](qualification/rust-cfg-2026-10-25/README.md).


## Conditional members and namespace declarations (format 66)

Named struct fields retain every raw declaration alternative, exact owner symbol
identity and original source position. Configuration selects the owner and groups
remaining alternatives by qualified field name. A type is published only for one
known-active declaration; duplicate or unknown competing declarations prevent a
unique inferred receiver. Same-name inactive struct owners cannot donate fields.
Conditional named/shorthand field initializers project their whole owning AST
node, including attributes attached inside that node. Calls in inactive
initializers disappear; permission syntax containing an omitted/unknown
initializer remains suppressed because its control flow is not evaluated.

Conditional extern-crate declarations and type aliases use original declaration
anchors for cfg evaluation while retaining Rust namespace hoisting. Standard
std/alloc extern aliases carry a reserved internal target tag, distinct from
ordinary use imports and legal source names. Nonstandard renamed extern crates
remain unverified. Captured direct Cargo dependencies retain their existing
resolution. A plain use path is never reinterpreted as an extern declaration.
Root extern shadows, captured dependencies named std/alloc and local namespace
shadows prevent fabricated standard-wrapper receiver inference.

Known-disabled item macro invocations/definitions need no expansion and do not
block live bindings. Active conditional item macros remain unmodeled. This does
not implement macro expansion, union-layout typing, tuple/enum field projection,
complete namespace/alias inference or compiler-wide build semantics.

Evidence: [conditional-member qualification](qualification/rust-cfg-members-2026-10-26/README.md).
