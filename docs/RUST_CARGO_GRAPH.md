# Captured Cargo boundary graph

Index format 30 adds `Cargo.toml` to native code-index inputs. Captured TOML is
parsed with `toml_edit`, not line matching. No Cargo invocation, build script,
network resolution or live filesystem reads are part of indexing or queries.
Normal source ingestion/sync must capture the manifests and Rust modules.

Implemented candidate binding:

- nearest same-project package boundary, including an invalid/stale manifest
  that blocks the former `src/lib.rs` + `src/main.rs` fallback;
- default and explicit lib/bin/test roots, automatic bin/test discovery,
  `autolib`/`autobins`/`autotests`, explicit example/bench roots;
- custom library crate names and roots;
- explicitly declared modules, inline modules, ordinary `#[path = "..."]`,
  and conventional nested module files;
- same-project captured path dependencies with renamed package aliases;
- workspace-inherited path dependencies relative to the nearest captured
  workspace root; a nested workspace does not inherit outer dependency entries;
- independent `crate::` lookup in library, binary and integration-test contexts.

Known module-file conflicts, missing modules, unsupported path literals,
conditional modules, optional dependencies and required-feature targets are not
guessed. A changed manifest changes the revision fingerprint and recomputes graph
binding, even when every Rust extraction is reused. A stale manifest is retained
as an unverified boundary; it cannot supply dependency facts. Frozen historical
graphs do not consult live source or current manifests.

`index_status.cargo_targets` exposes captured target identities and member paths,
bounded by the diagnostic request limit and 128 member paths per target. Counts
and truncation are explicit. Target IDs include project, manifest path/hash and
target declaration. These are source-context IDs, **not compiler configuration
IDs** or separate per-target symbol identities. The evidence explicitly sets
`complete=false`, `features_evaluated=false`, `cfg_evaluated=false`,
`macro_expansion=false` and `compiler_verified=false`.

## Remaining scope

This is the first part of requirement 1 in `CODE_INTELLIGENCE_PLAN.md`.
Explicit one-target build/feature views now exist as an opt-in subset; see
`RUST_BUILD_CONFIGURATIONS.md`. Full Cargo workspace membership/defaults,
target triples, resolver/edition-specific extern prelude rules, public/restricted
visibility and re-exports, registry/git/patch dependencies, crate-target symbol
identities, conditionally compiled declarations, macro/include expansion,
complete `#[path]` semantics and compiler-semantic Rust resolution remain open.
Source files shared by targets yield candidate unions; they are not monomorphized
graphs. Projects without a captured manifest keep the earlier conventional-layout
fallback, which is not a verified Cargo model. Missing captured manifests cannot
prove that no manifest exists on disk.

The current resolver constructs target/module maps during imported lookup.
Build-once context indexing, dependency-scoped incremental resolution and
representative latency/memory measurements remain in requirement 8. No speed or
competitor-superiority claim follows from the fixture tests.

Qualification: `qualification/cargo-workspace-2026-10-25/README.md`.
