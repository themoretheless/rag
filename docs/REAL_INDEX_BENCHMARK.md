# Real-source index benchmark protocol

`bench_real_index.py` measures explicitly declared real-source workloads through
an isolated MCP server/store. Recipes pin every Cargo.toml/src input hash and
specify exact, single-match source edits. The original source tree is not mutated;
all edits and compiler output live in an owned temporary copy.

The interval includes every changed-file ingest RPC and the graph commit. Cold
startup/copy, file edits, compiler checking and equivalence audits are outside it.
Warm no-op always uses ordinary index_code for both incremental and full binding
control modes. Other full-control phases force complete binding; this is an
internal algorithm control, not a competitor. Worker RSS sampling is approximate
process RSS including baseline/cache retention, not exact operation peak.

Qualification-only mode runs Cargo checks and full-binding equivalence audits
outside timing and marks all times diagnostic. Both modes use identical per-phase
source hashes and mutations; the replay auditor validates them and ordinary no-op
revision reuse. Mock embeddings and exact input/work scopes are explicit.

Performance-mode attempts require a matching declared release artifact manifest
and reject competing compiler/busy-process snapshots before/after phases. Initial
rejection writes evidence and exits 3; mid-run failures retain partial records,
compiler/server stderr and error metadata. A manifest identity match does not
prove compiler optimization/source provenance. The runner never sets
performance_qualified=true: repeated stable release controls and equivalent-work
competitor qualification remain separate gates. Passing process snapshots alone
does not prove thermal/power or whole-machine isolation.

Qualified first recipe: pinned itoa with captured corpus Cargo overlay, five
phases (cold, ordinary no-op, body, signature+caller, manifest metadata). Both modes
pass ten compiler checks and ten full-equivalence audits. Three guard tests and
actual rejected/fault-injection runs qualify the instrumentation. Evidence:
[real workload qualification](qualification/real-index-workloads-2026-10-26/README.md).
The debug/busy-machine timings are not accepted performance results. Larger real
sources, feature/dependency workloads, release provenance, repeated stable controls,
representative RSS and GitNexus/Codebase Memory MCP/RAGRAF adapters remain open.


A stronger release producer/verifier now exists. The optional
`audit_real_index_bench.py ... --release-manifest PATH` validates retained raw Cargo
compiler events, frozen source inventory, command/profile and executable identity
through `verify_index_release.py`; it does not trust profile metadata alone.
The verified release passes both itoa and memchr workflows (twenty compiler checks
and twenty full binding audits). An actual valid-manifest performance attempt is
rejected for competing activity. These workflow times remain diagnostic, and no
performance gate is upgraded. Details:
[INDEX_RELEASE_ARTIFACT.md](INDEX_RELEASE_ARTIFACT.md).
