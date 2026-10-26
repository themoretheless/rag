# Git impact qualification

Contract: [GIT_DIFF_IMPACT.md](../../GIT_DIFF_IMPACT.md). `detect_changes` uses
ephemeral captured source graphs, not a presumed-fresh persistent index.

Evidence files:

- `runtime.json`: actual stdio MCP and a real temporary Git repository. Independent
  labels cover three symbol change kinds and two incoming paths on each side.
  Checks distinguish staged blobs from unstaged bytes, verify per-occurrence
  source identity, preserve source/index bytes, enforce output/depth/capture
  bounds, pin commit comparisons, admit untracked source, reject unmerged index
  stages and invalid enum input, and prove a configured clean filter was not run.
- `native-tests.log`: six native Git lifecycle tests, additionally covering
  duplicate symbol groups, actual newline/space paths, source revalidation,
  source-byte/file/seed caps, unavailable parse-error sides, absent skip-worktree
  projection and manifest-only feature/configuration changes.
- `path-regression.json` and `config-regression.json`: earlier real-MCP path and
  captured Cargo configuration fixtures pass on the same binary.
- `compiler.log`: offline Cargo check of the positive baseline fixture.
- `workspace-tests.log`, `clippy.log`, `build.log`, `checks.json`: current
  regression and command outcomes. Binary/fixture hashes are in runtime reports;
  source hashes and exact test counts are in checks.json.

Final workspace outcome: 1310 passed, zero failed, one ignored. Strict library/
test Clippy, binary build, positive compiler check and all three MCP runners
passed. `native-tests.log` is the six-test excerpt from that current workspace
run, rather than an older focused-test build.

```sh
cargo test --workspace
cargo clippy --lib --tests -- -D warnings
cargo build --bin rag-mcp
python3 scripts/smoke_git_impact.py target/debug/rag-mcp /tmp/git-impact.json
cargo check --manifest-path tests/fixtures/code-intelligence/git-impact/Cargo.toml --all-targets --offline --target-dir /tmp/git-impact-fixture
```

The final runtime binary has format 35, which also includes concurrent baseline
source-representation work. This directory does not attribute that work to Git
impact. The checkout was not frozen; each runtime report identifies its exact
binary. Qualification runners use temporary databases and Git repositories;
production sources, database and installed executable are not changed by them.
Normal source/doc re-ingestion uses the already connected production writer.

This qualifies plan requirement 5 for supported captured source/manifest graph
semantics. It does not prove complete compiler/runtime impact, unsupported working
file changes, universal Rust precision/recall, incremental speed or competitor
superiority. See the contract for HEAD, mode-bit, rename, resource and source
coverage limits. Affected-test selection, representative corpus evaluation,
incremental performance, rename and infrastructure remain separate plan items.
