# Independent Rust quality on the benchmark release image

Status: bounded same-release corpus quality qualified. Full plan remains open.

## Results on the exact performance candidate

| Repository | Positive labels | Correct local targets | External-negative labels | False local targets |
|---|---:|---:|---:|---:|
| itoa | 9 | 9 | 48 | 0 |
| ryu | 95 | 93 | 142 | 0 |
| memchr | 466 | 363 | 423 | 0 |
| Combined | 570 | 465 | 613 | 0 |

Server SHA-256:
dab96adb738e01528410810122a0a7002edfa5ae1090761963be0201be3ea3fe.
This is the exact frozen optimized image already used by both release index
workload cohorts. Helper SHA-256:
441b15167a0901a8f342877af431f23c0943ca578d22e03ba93f23a8cbbff7e5.
Index format 70. Native and actual-MCP candidate sets match at every one of the
1183 scored sites. Positive recall stays 465/570 (81.58%). All 613 external
sites have no local candidates; 608 have observations and five have none.
Missing observations and negative abstentions are not positive target resolutions.

All 1183 oracle labels, source/config inputs and independently derived SCIP/CST
facts remain unchanged. The original debug baseline's expected labels, observed
local definition positions and every per-case TP/FP/FN/exact score match the new
release results. No recovered or lost site is hidden by unchanged aggregate counts.
Baseline artifacts are hashed and byte-checked. This is a same-input quality
retention comparison; it does not isolate optimization flags as a causal variable
between the earlier shared debug checkout and later frozen build snapshot.

The corpus remains three small libraries, with original RA errors, excluded
ambiguous/generated anchors, disabled-branch denominator and configuration limits.
External negatives cover core/alloc. These are source-definition correspondence
labels with explicit limitations, not universal/compiler/runtime call truth.
105 original positive labels remain unresolved; no speed or memory win follows.

## Exporter linked to the original release libraries

The guarded Rust exporter source comes from the same frozen source snapshot.
It is linked directly against the original Cargo-produced optimized rag_mcp and
serde_json runtime objects and full metadata, using the original rustc executable
and matching explicit release options. The package/features match the server;
there is no Cargo example/dev-dependency feature rebuild. The helper hashes and
rechecks 838 compiled-library/metadata search inputs before/after linking. These
include potential search candidates; the count is not a claim that all 838 are
actually consumed by the final linker.

The full .rmeta and matching .rlib are both passed explicitly. The observed split
metadata layout and need for a companion metadata input are documented in the
[Rust team's metadata experiment note](https://blog.rust-lang.org/inside-rust/2026/08/18/reducing-target-dir-size-on-nightly/).
The first rlib-only command failed for missing full metadata; the rmeta-only
command failed for missing linkable runtime objects. Both failed states/stdout/
stderr are retained. The final paired command succeeds. These attempts never
opened the production store or generated accepted corpus scores.

verify_release_quality_capture.py independently checks final state/manifest,
retained build-driver identity, original server provenance, helper source/binary,
compiler/options, raw Cargo runtime library selection, full metadata plus object
extern arguments and all recorded library hashes. It qualifies the pair before
its corpus runs. Final executable instructions can differ after thin LTO even
though their server-library inputs are the same; no instruction identity is claimed.
Two selector regressions reject host/ambiguous/outside runtime artifacts. Five
SCIP adapter/metric tests pass. No Rust implementation was edited and full Rust
workspace tests were not repeated for this Python/link/export qualification.

## Store ownership and reproducibility

The quality runner owns disposable stores and stops the MCP writer before native
capture. The Rust exporter checks a strict temporary directory prefix/marker and
uses create-new output. The production database is never opened by the exporter.
The server and helper are rehashed; the original release source/profile verifier
still passes after helper compilation and corpus collection.

```sh
python3 scripts/build_release_quality_capture.py SERVER_BUILD_ROOT NEW_HELPER_ROOT
python3 scripts/verify_release_quality_capture.py HELPER_ROOT/helper-manifest.json SERVER_BUILD_ROOT/release-manifest.json NEW_PAIR_PROOF
python3 scripts/qualify_rust_corpus.py SERVER HELPER PINNED_SOURCE SCIP NEW_OUTPUT --external-descriptors
python3 scripts/summarize_rust_quality.py CONTROLLED_ROOT NEW_SUMMARY
python3 scripts/test_release_quality_capture.py
```

The helper, retained driver/manifest/logs and all 838 compiled search inputs have
stream-verified private ignored-cache backups. Another 27 native archives are
retained as post-build relink support; they are not asserted to have been separately
consumed. The server's existing source/raw-Cargo/binary backup remains unchanged.
backup.json records original restoration roots, archive identities and constraints;
restore only into owned staging locations without overwriting existing files.
Original toolchain and external SDK inputs remain necessary for relinking.

Runtime binaries remain frozen, production executable unchanged. Shared working
tree changes and source whitespace elsewhere are not attributed to this milestone.
Remaining gates include stable repeated release controls, representative scale/
memory, equivalent-work competitor adapters and broader semantic/corpus coverage.
