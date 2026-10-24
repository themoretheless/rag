# CSS and policy trace qualification — 2026-10-24

Scope: `class_usage`, `policy_trace`, original-markup ingestion, their cached
source-sync lifecycle and actual local stdio MCP registration/dispatch. This is
not a deployment report or whole-RAGRAF parity/quality benchmark.

Verified in the current worktree:

- Complete library suite: 722 passed, no failures, with four test threads;
  file-ingestion integration suite: 3 passed. This run preceded the final two
  focused regression additions (computed case-label filtering and JSX entity
  interpretation).
- After those final changes: all 56 code-index-related tests passed, including
  both new regressions, CSS/HTML/JSX/DOM evidence, project isolation, preprocessing
  diagnostics, PHP decisions/callers, missing/corrupt original sources, persistence,
  stale exclusion and synchronization.
- The actual `rag-mcp` executable built successfully without default features.
- The stdio JSON-RPC smoke test passed against that executable with an isolated
  temporary database and mock embeddings. Machine-readable output:
  [mcp-smoke.json](mcp-smoke.json). The fixture supplies an explicit `wing` because
  a source root named `sources` otherwise applies the existing collection/project
  taxonomy. This verifies isolation rather than silently joining different projects.

Reproduction:

```sh
cargo test --lib --test file_ingest_test --no-default-features -- --test-threads=4
cargo test --lib code_index --no-default-features -- --test-threads=4
cargo build --bin rag-mcp --no-default-features
python3 scripts/smoke_code_styles_policy.py target/debug/rag-mcp /tmp/rag-css-policy-report.json
```

One initial unrestricted parallel run had 720 passes and an existing retrieval
embedding-timeout assertion failure. That test passed in isolation, and the full
suite passed with four threads. The test uses a 20 ms real-time deadline; its
load-sensitive assertion was not weakened or changed.

Limits: optional tokenizer features and the running production gateway were not
qualified/restarted. Permission results are syntactic source evidence, not runtime
role decisions. CSS results are static mentions matched by project/name, not a
browser cascade or safe deletion proof. PHP namespace bindings, framework routing,
embedded markup and full receiver/inheritance analysis remain partial/unqualified.
See [CSS_POLICY_ANALYSIS.md](../../CSS_POLICY_ANALYSIS.md) for the exact contract
and [RAGRAF_FEATURE_MATRIX.md](../../RAGRAF_FEATURE_MATRIX.md) for open acceptance
requirements.
