# Frozen release build provenance and index workflows

Status: source/profile provenance and bounded index workflows qualified.
Representative speed, memory and competitor superiority remain unqualified.

## Build evidence

1880 current working-tree files (including uncommitted work) were captured by
content/executable-bit identity. Capture checks original hashes at both boundaries;
compilation uses an owned frozen copy, not the changing shared checkout. Every
captured file and added/missing-file inventory is checked again after the build.
This is not a claim of a clean Git release commit or an atomic filesystem snapshot.

Cargo ran offline/locked with explicit opt-level3, debug-assertions=false, thin
LTO, one codegen unit, stripped debuginfo, cleared Rust flags/wrappers and two
build jobs. Cargo/rustc/rustdoc executable paths, hashes and version output are
recorded; executable/version identities were rechecked after the build. Raw Cargo
JSON requires one server executable event and successful build-finished event.
Its profile reports opt_level="3", debuginfo=0, debug_assertions=false and test=false.
The locked dependency manifest, source inventory, command and raw logs are hashed.
External SDK/native toolchain and registry cache are not fully hermetic inputs;
this is source/profile provenance, not bit-reproducibility or supply-chain signing.

The independent verifier compares the release manifest with raw Cargo events,
actual source files, entrypoint/package paths, command controls, tool metadata,
compiler output and frozen binary. Five altered manifests (binary, profile spoof,
source inventory, release command and driver identity) are rejected while the
original still verifies. Two unit tests cover non-release/failed/duplicate artifact
and snapshot mutation rejection. Three existing benchmark guard tests pass.

Frozen server SHA-256:
dab96adb738e01528410810122a0a7002edfa5ae1090761963be0201be3ea3fe.
Index format observed through actual MCP: 70. Production executable was not
replaced and the production database was not opened for build/runtime tests.

## Actual release index workflows

| Repository | Initial inputs | Modes | Phases per mode | Successful Cargo checks | Full reference-equivalence audits |
|---|---:|---:|---:|---:|---:|
| itoa | 3 | 2 | 5 | 10 | 10 |
| memchr | 46 | 2 | 5 | 10 | 10 |

Both modes run cold ingest/index, ordinary no-op, body, signature+caller and
manifest-metadata changes. Memchr's signature edit adds a unit parameter and
updates three callers; actual compile checks validate the complete copied crate.
Per-phase changed-input counts are 3/0/1/2/1 and 46/0/1/2/1 respectively. Mutation
receipts and input hashes are replayed across modes. Ordinary no-op never forces
a full rebuild. All twenty out-of-window full binding audits match every captured
reference and keep the existing revision. This is cache equivalence and compiler
checking, not independent semantic truth of every mutated reference or behavioral
tests of every library algorithm. An earlier debug memchr recipe check is retained
as preliminary evidence and is not counted among the twenty release checks.

The cohorts use deterministic mock embeddings plus local document/FTS/index work,
with end-to-end ingest and graph commit in the measured interval. Compiler and
equivalence audits warm caches outside timing; every retained workflow time remains
diagnostic. Sampled process RSS is not exact operation peak or allocation delta.
Actual injected compiler failures retain partial phase records and stderr; missing
manifest negative controls exit3. These controls are distinct from the successful
compiler checks above.

A genuine performance-mode invocation WITH the verified release manifest also
exited3 before indexing: competing desktop/application activity (no compiler in that final sample). Its report records a matching
manifest and only the environmental rejection reason. This is a retained rejected
measurement, not evidence of slow/fast indexing. No speed ratio, memory win or
competitor advantage is published.

## Preservation and reproduction

The verified source/binary/raw-Cargo bundle is backed up in the ignored private
project artifact cache. backup.json records archive path/hash, 1880 checked source
files, binary identity and original absolute restore root. Archive entries were
stream-hashed against the source inventory and executable, and the compiler output
is an internal hardlink to avoid storing the binary twice. Restore only into a new
empty original owned staging root; never overwrite another checkout or build.
The backup preserves the compiler output needed by the verifier after temporary
build caches expire. Its presence is not a deployment or installation.

```sh
python3 scripts/build_index_release.py REPOSITORY NEW_EXTERNAL_BUILD_ROOT --jobs 2
python3 scripts/verify_index_release.py BUILD_ROOT/release-manifest.json BUILD_ROOT/bin/rag-mcp NEW_VERIFICATION
python3 scripts/bench_real_index.py BUILD_ROOT/bin/rag-mcp PINNED_SOURCE RECIPE NEW_OUTPUT --build-manifest BUILD_ROOT/release-manifest.json --qualification-only
python3 scripts/audit_real_index_bench.py COHORT_FOLDER PINNED_SOURCE RECIPE NEW_SUMMARY --release-manifest BUILD_ROOT/release-manifest.json
```

Each cohort audit expects its accepted incremental/full-binding and rejected/
fault files. Original prepared corpus overlays are pinned by recipe input hashes,
not claimed to be untouched upstream manifests. Build source-file hashes and all
raw Cargo evidence are retained here; local sources/binary remain in the staging
root and durable archive. New build/helper/auditor/test source hashes are recorded.
No Rust implementation was edited in this milestone and the full Rust workspace
suite was not repeated; runtime qualification here is specifically the index
workflow subset on this frozen release image, not every API or all original goals.

Remaining gates: stable quiet alternating repeated controls, representative scale
and memory, independently qualified quality on the same release binary and equal-
work competitor adapters/runs. Full Cargo/compiler/generic semantics and 105
unresolved positive corpus labels remain open. The complete original plan remains
active.
