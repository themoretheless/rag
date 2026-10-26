# Independent Rust source-definition corpus

This is requirement-4 progress, not completion of the ten-item plan. Three pinned
public library crates provide 570 admitted callable-reference sites, including
486 direct-call sites. The corpus is small, has no admitted external-negative
sites, and does not represent compiler-complete Rust or runtime dispatch.

| Repository | Commit | Before correct / admitted | After correct / admitted |
|---|---|---:|---:|
| dtolnay/itoa | 1577ed901354d0d7448ac162328f9dbf5183124c | 0 / 9 | 9 / 9 |
| dtolnay/ryu | 75f9e707e77c89717ffb9c2a01fe2c28a016cc82 | 0 / 95 | 56 / 95 |
| BurntSushi/memchr | bd6068c30e9074a90c285e47912fa0b047d07597 | 107 / 466 | 107 / 466 |

Combined: 107 to 172 correct targets, two to zero false targets, 463 to 398 missed
targets. Admitted-reference recall increases from 18.77% to 30.18%. The direct-call
subset increases from 106/486 to 156/486 (21.81% to 32.10%), with one to zero false
targets. Imports, macro tokens and value/other references have separate metrics
in `summary.json`. Zero observed false targets is not universal 100% precision:
the sample excludes ambiguous/generated anchors and has no external negatives.

## Controls and oracle

`corpus-manifest.json` pins repositories, original bytes, explicit host rustc cfg,
default features, Cargo build-script cfg output and the manifest metadata overlay.
Rust source bytes are unchanged. Inputs are Cargo.toml and every src/**/*.rs;
benchmark haystacks and unrelated repository assets are outside this workload.
Cargo checks the default-feature library for the recorded target. The indexer
does not automatically execute Cargo or build scripts; preparation supplies cfg.

rust-analyzer 2026-10-19 provides independent SCIP occurrence/definition anchors
and CST syntax classes. `oracle-provenance.json` and `oracle-artifact-hashes.json`
record binary/archive/schema/SCIP/CST identities. The raw cache under /tmp is
disposable; reproduce it with the commands below. RA exited successfully but
logged 6/60/8 ERROR messages for itoa/ryu/memchr. Original logs are retained.
Multiple-definition, missing-local-definition, nonlexical/generated anchors,
noncallable kinds and references outside the library unit are excluded with
counts in every `oracle-labels.json`. In particular six ryu references from
test compilation units are excluded, despite their src/ paths.

The label adapter uses document-local SCIP IDs, UTF-8 byte coordinates, typed and
legacy ranges, library-unit monikers, callable kinds and lexical fn anchors.
Syntax classification comes from RA CST, independently of the rag parser.
This tests correspondence to language-server source definitions, not an
independent rustc correctness theorem or monomorphized runtime call graph.

Every run uses a fresh owned temporary DuckDB. MCP ingests and indexes sources,
then stops before the native capture opens that same temporary database. Native
capture stops before MCP restarts. Bound-reference target-file queries fall back
to individual symbol IDs on truncation. Native/MCP candidates agree at every
admitted site in both phases. Production DuckDB and installed MCP are untouched.

The comparison verifier requires identical input hashes, oracle hashes and label
hashes, exact scored-label correspondence and one frozen server/helper pair per
phase. Before is format 50; after is format 52. The latter includes cfg_attr
handling, macro-token and module-call false-target guards, and conditional public
import filtering. Raw before/after results and SHA-256 identities are retained.
This comparison does not isolate the contribution of each individual fix.

The public-import regression switches feature-selected same-name targets and
checks unknown-cfg abstention. It does not fix ryu's remaining nested conditional
expressions: 35 of its 39 misses still carry
RUST_CFG_NESTED_SEMANTICS_UNMODELED, two are shadowed/external and two lack extracted
reference facts. Broad Rust typing, macro expansion and visibility/re-export
chains remain open. Index format 52 invalidates earlier cached extraction.

## Reproduction

Use a clean output directory and the verified RA release binary. Repository
caches must contain the pinned commits; preparation exports exact git archives.

```sh
python3 scripts/prepare_rust_quality_corpus.py /tmp/new-rust-corpus /path/to/rust-analyzer --cache /path/to/git-caches
cargo build --bin rag-mcp --example code_quality_capture
python3 scripts/qualify_rust_corpus.py /path/to/frozen/rag-mcp /path/to/frozen/code_quality_capture /tmp/new-rust-corpus/itoa /tmp/new-rust-corpus/itoa.scip /tmp/result-itoa
python3 -m unittest discover -s scripts -p test_scip_binding_labels.py
```

Repeat qualification for ryu and memchr and both frozen versions. The checked
summary script expects `controlled-{itoa,ryu,memchr}-{before,after}` directories:

```sh
python3 scripts/summarize_rust_quality.py /tmp/results /tmp/summary.json
```

The pre-final workspace checkpoint passed 1417 tests, one ignored. Final focused
Rust regressions passed 40 tests, strict library/test/example Clippy passed and
the three adapter/metric regressions passed. The shared dirty checkout includes
concurrent unrelated changes and was not frozen; binaries were copied and hashed
before these runtime runs. Sync timing is uncontrolled debug diagnostic data,
not release speed evidence. Competitor measurements, representative scale/RSS,
broader repositories and the remaining original plan gates are still required.
