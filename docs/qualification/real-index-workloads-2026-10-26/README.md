# Real-source index workload qualification

Status: workload/instrumentation qualification passed. Release speed, memory and
competitor performance remain unqualified; this does not close requirement 8.

## Workloads actually exercised

Pinned itoa commit 1577ed901354d0d7448ac162328f9dbf5183124c uses the preceding
independent corpus's captured Cargo graph-configuration overlay. Input hashes
identify that prepared manifest and the two original Rust source files; it is not
claimed that the overlaid manifest is byte-identical to upstream Git. Every run
copies the repository to an exclusively owned temporary source tree. The original
three source/config inputs are rehashed before and after each run.

| Phase | Ingested inputs | Operation |
|---|---:|---|
| cold_ingest_and_index | 3 | Manifest + two source files, then initial graph commit |
| warm_noop | 0 | Ordinary index_code in BOTH modes; no new revision |
| body | 1 | Parenthesize an existing arithmetic expression in mulhi |
| signature | 2 | Add unused unit parameter to mulhi and update its caller |
| manifest_metadata | 1 | Add benchmark metadata nonce; not a feature/dependency change |

Both incremental and full_binding_control modes pass all five phases. Cargo
check --offline --lib runs after each phase outside the measured interval:
ten successful compiler checks total. After each phase an out-of-window forced
full binding recomputation matches the complete captured reference set and
retains the existing revision: ten equivalence audits total. This is internal
cache equivalence, not independently labelled semantic correctness of every
mutated reference or proof of program behavior beyond compilation.

The replay auditor reconstructs every mutation from the original owned inputs,
compares per-phase hashes/receipts across both modes, verifies 3/0/1/2/1 parsed
input counts, unchanged counts, revision reuse/creation, ordinary no-op arguments,
sub-operation timing bounds, parser/freshness state and binary/recipe/runner/client
identity. All output files use create-new semantics. Three guard tests cover
external/symlink paths (including directory symlinks), nonunique/no-op/conflicting
edits and environment gating that excludes the measured worker but rejects idle
compilers and other busy processes. macOS /var aliases are canonicalized.

## Timing and memory scope

The end-to-end interval starts before the first ingest RPC and ends after the
index RPC returns its committed result. Per-RPC ingest/index durations and results
are retained. Startup, temporary source copy, recipe edits,
compiler checks and equivalence audits are excluded. Warm no-op is ordinary in
both modes; it is never compared to a forced audit as though they were equal work.

The isolated server uses deterministic mock embeddings (16 dimensions), FTS and
document writes plus the code graph. These are local store/index costs, not real
embedding-model cost. Worker RSS is sampled every 50ms through ps; it includes the
process baseline and retained caches, is a sampled lower bound, and is not exact
operation peak memory, allocation delta or dedicated physical memory.

All retained timings are diagnostic: the frozen format-70 server is a debug build
and competing compilation was observed. No speed ratio or memory win is qualified.
Qualification audits also warm caches, so these runs must not be repurposed as
controlled timing evidence. The single small library does not establish scale.

## Rejection and failure evidence

The non-qualification invocation rejected the current environment/build before
creating a benchmark store: competing processes and no matching release artifact
manifest. It wrote rejected-performance-final.json and exited 3. The runner checks
manifest identity; supplied profile metadata alone is not compiler provenance or
performance qualification. Even successful unpaired measurement never sets
performance_qualified=true. Quiet process-name/CPU snapshots are a necessary guard,
not whole-machine, thermal or power-state isolation proof.

A deliberate invalid Cargo option exercises failure preservation. It exits 1
and retains failed-compiler-audit-final.json, the completed cold phase, compiler
stdout/stderr, server stderr, original input/binary integrity and error metadata.
This is an expected fault-injection check, not a real fixture compilation failure.

## Reproduction

```sh
python3 scripts/bench_real_index.py SERVER PINNED_ITOA tests/fixtures/code-intelligence/index-bench/itoa-workloads.json NEW_OUTPUT --qualification-only
python3 scripts/bench_real_index.py SERVER PINNED_ITOA tests/fixtures/code-intelligence/index-bench/itoa-workloads.json NEW_OUTPUT --mode full_binding_control --qualification-only
python3 scripts/audit_real_index_bench.py ARTIFACT_FOLDER PINNED_ITOA tests/fixtures/code-intelligence/index-bench/itoa-workloads.json NEW_SUMMARY
python3 scripts/test_real_index_bench.py
```

The auditor expects the recorded accepted/rejected/fault-injection filenames in
ARTIFACT_FOLDER. Recreate the fault recipe with the recorded compiler_arguments.
Script, client and recipe hashes are retained in source-artifact-sha256.json.
Frozen server SHA-256:
edb5c3ea69692f46690b1b974f98bf4c9f8c284d4a04b0976ecf830efe41af55.
Its debug build and separate semantic qualification are recorded in the preceding
union-field milestone. No runtime semantic code changed here, and the full Rust
workspace suite was not repeated for this Python-only harness work.

Production executable unchanged; benchmark stores exclusively owned/disposable.
Shared source checkout is not frozen and unrelated concurrent work is not
attributed here. All earlier exploratory files remain in the temporary run root.

Remaining requirement-8 gates: captured release compiler/source provenance,
quiet alternating repeated controls and stability rejection, larger real workloads,
representative memory measurement, and equivalent-work competitor adapters/runs
with explicit input/configuration/quality coverage. The full original plan remains
active, including the 105 unresolved positive corpus labels and remaining compiler/
build semantics.
