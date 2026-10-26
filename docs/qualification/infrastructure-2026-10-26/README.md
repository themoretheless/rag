# Captured infrastructure graph qualification

This advances requirement 10 of the [complete plan](../../CODE_INTELLIGENCE_PLAN.md).
The [contract](../../INFRASTRUCTURE_GRAPH.md) lists provider, freshness,
provenance and execution limits. This is fixture qualification of captured
declarations, not observed deployment or representative infrastructure accuracy.

## Independent labels and actual MCP

The fixed source corpus is `tests/fixtures/code-intelligence/infrastructure/`.
Its `expected.json` independently names an eight-edge path:

`Kubernetes Service → workload → container → image → Dockerfile → stage → executable → source file → Rust main`.

`runtime.json` records actual stdio MCP verification against those labels, the
forbidden different-namespace workload, exact ENTRYPOINT line 8 and supporting
COPY/RUN/inter-stage COPY lines 3/4/7 plus captured Cargo manifest provenance.
Every returned byte/line anchor is checked against the matching fixture source.
The `code_symbol.symbol_id` is accepted by the ordinary symbol query.

The runner also verifies the GitHub Actions build job and local script path,
deterministic nonmutating reads, depth/work caps, stale bridge exclusion after
ingestion, frozen historical paths, changed binding after reindexing, default
project isolation, explicit cross-project exact-image candidates, endpoint
ambiguity and typed-input rejection. Deleting an un-ingested live Dockerfile
does not affect the captured graph, proving the stated stored-source boundary.

The positive Rust bin is separately built with actual offline Cargo release
compilation and executed on the host. It prints the expected fixture marker.
That verifies the selected source/bin example; it is not a Docker image build,
Kubernetes schema admission, deployment, architecture compatibility or runtime
path qualification. Docker/cluster commands are never run by the graph query.

## Native regression boundaries

Seven native infrastructure tests cover YAML block/flow/multiple documents and
rejection of duplicate keys/aliases/tags/invalid syntax; full labelled path and
supporting evidence; namespace negatives; stale deployment/manifest guards and
frozen inputs; explicit project linking and traversal limits; dynamic image/
command/context failures; duplicate resource identities, unpublished sources and
standalone Dockerfile context diagnostics. Unsupported RUN/COPY operations must
not retain an earlier code mapping. Regression cases also prevent fallback from
unknown ENTRYPOINT to CMD and interpreting a JSON COPY source as a `--from` flag.
Source-ingestion regression now admits Dockerfile/Containerfile variants.
The 128/129-stage boundary rejects overflow without publishing a partial stage.
An integration regression admits hash-guarded README/YAML/Dockerfile metadata
without treating that text as Rust collisions; metadata changes still invalidate
rename application. Other source languages remain blocked by the Rust rename.

`checks.json` contains final workspace, strict library/test Clippy, build, native
and real-MCP outcomes, binary identity and artifact hashes. The rename and
resolution MCP regressions use the same frozen executable as infrastructure.
The checkout also contains concurrent language work and is not a frozen source
tree. A localized PowerShell identifier clone repairs a concurrent borrow-check
failure without claiming that extractor as infrastructure work.
The workspace checkpoint passed 1398 tests, zero failed, one external-DB fixture
ignored. After the final test-name and equivalent JSON-comparison style repairs,
351 current code-index tests and strict library/test Clippy passed. The three
actual MCP reports share format-48 frozen binary SHA-256
`62a5ffbc2d294c04e81ac77734dc9a0c9e573b48fc99b5f0b6d8722c30d6bfeb`.
Format changes include concurrent language work, not an infrastructure-owned
syntax-cache version change.

## Remaining scope

YAML aliases/templates, Helm/Kustomize rendering, Terraform/Ansible, general shell/
Make evaluation, remote contexts, workspace/custom Cargo output directories,
external image content, ignore-file selection and runtime overrides retain the
documented unsupported/unverified boundaries. Unknown inputs block the relevant
source mapping rather than guessing. Image-name matches remain candidates.
Infrastructure edges are recomputed from frozen source inputs by the current
contract; a persisted infrastructure-edge diff format is not implemented.

The fixture does not establish real-repository precision/recall or competitor
speed superiority. Qualification uses disposable sources/databases/Cargo targets,
with no installed executable replacement, commit or push. The original complete
goal remains active, including real-corpus quality, comparable performance and
remaining Cargo/typing semantics.

## Reproduction

```sh
cargo test --lib code_index::infrastructure -- --nocapture
python3 scripts/smoke_infrastructure.py /path/to/rag-mcp /tmp/infrastructure.json
```
