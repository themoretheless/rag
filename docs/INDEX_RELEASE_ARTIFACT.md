# Index benchmark release artifacts

`build_index_release.py` captures current tracked/nonignored working-tree files
into an owned external source snapshot, verifying capture-boundary content hashes.
It builds offline/locked with an absolute compiler toolchain, explicit optimized
release controls and bounded Cargo jobs. Raw Cargo JSON, compiler profiles, tool
identities, source/dependency hashes and logs accompany the frozen executable.
Successful compilation alone leaves runtime/performance qualification false.

`verify_index_release.py` independently checks manifest/source/raw-Cargo/command/
executable consistency. It rejects pending or unoptimized artifacts and metadata
that disagrees with raw compiler output. This guards stale/corrupt evidence; it is
not a hermetic build, cryptographic attestation or guarantee about every external
SDK/native toolchain input.

`audit_real_index_bench.py ... --release-manifest PATH` requires this stronger
verification in addition to same-input, mutation-replay and cache-equivalence
checks. The underlying runner's manifest identity match by itself is not enough
to establish compiler provenance. The auditor never upgrades audit-warmed workflow
times into accepted speed results.

The first artifact captures 1880 source files and is release-profile verified.
Actual isolated MCP workflows on itoa (3 inputs) and memchr (46 inputs) pass twenty
Cargo checks and twenty full reference-binding equivalence audits across five
phases and two modes. An actual timing attempt with its valid manifest is rejected
for competing desktop/application activity. Source, executable and Cargo evidence have a
verified backup in the ignored project artifact cache; backup.json describes the
original-path restoration constraints.

Evidence: [release provenance and workflows](qualification/index-release-provenance-2026-10-26/README.md).
Production executable is not replaced. Stable repeated performance controls,
representative memory, same-release independent quality and equivalent competitor
workloads remain open alongside the complete original code-intelligence plan.


The same release image also retains the independent Rust corpus result: 465/570
local positives, zero false local targets across 613 external negatives, and exact
native/MCP candidate parity at all 1183 labelled sites. The native exporter uses
the original server's optimized Cargo library objects/full metadata with verified
compiler/source/input identities and no dependency-feature rebuild. See
[same-release quality](qualification/same-release-rust-quality-2026-10-26/README.md).
Stable repeated timing/RSS and competitor qualification remain separate gates.
