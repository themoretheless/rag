# Related-test qualification

Contract: [AFFECTED_TESTS.md](../../AFFECTED_TESTS.md). This qualifies requirement
6 for supported captured static relationships, not safe CI test omission.

The independently labelled Cargo fixture supplies seven related tests: ordinary
unit calls, an alias import, nested `super::super`, an external cfg(test) module,
a callback value, an ignored case and a hyphenated integration target. A doc
string containing `[test]` is explicitly not a test.

- `runtime.json`: actual stdio MCP, exact independently expected test sets,
  original occurrence evidence, calls versus value references, two-edge paths,
  direct changed-test identity, depth/result/work limits, typed input refusal,
  stale stored-source exclusion/refresh and Git before/after recommendations.
- `commands.log`: every one of the seven recommended Cargo argv vectors was
  executed against the matching baseline fixture. Each executed one passing
  test, including the ignored case (`-- --ignored`) and `--test with-hyphen`.
  Exit codes/counts and stdout hashes are recorded in runtime.json. The product
  query itself does not execute commands.
- `native-tests.log`: native tests additionally cover test-marker lookalikes,
  custom harness filtering, project isolation, ambiguity, normal-to-test context
  ID remapping, obsolete configuration-ID refusal and cfg-gated import ownership.
- `git-regression.json`, `path-regression.json`, `config-regression.json`: previous
  real-MCP contracts pass on the same format-39 binary.
- `compiler.log`: offline all-target Cargo check of the positive fixture.
- `workspace-tests.log`, `clippy.log`, `build.log`, `checks.json`: current command
  outcomes, counts and source hashes. Runtime reports pin binary/fixture hashes.

Final outcome: 1329 workspace tests passed, zero failed, one ignored. Strict
library/test Clippy, the compiler check, binary build and all four actual-MCP
runners passed. All four runtime reports use the same format-39 binary.

```sh
cargo test --workspace
cargo clippy --lib --tests -- -D warnings
cargo build --bin rag-mcp
python3 scripts/smoke_affected_tests.py target/debug/rag-mcp /tmp/affected-tests.json
cargo check --manifest-path tests/fixtures/code-intelligence/affected-tests/Cargo.toml --all-targets --offline --target-dir /tmp/affected-test-fixture
```

Earlier development checks exposed the missing inline-super path and inactive
cfg-import ownership cases; final regression covers the corrected behavior.
The checkout contains concurrent baseline corpus/resource work and was not
frozen. This evidence does not attribute that work to related-test discovery.
Runners use temporary databases, Cargo targets and Git repositories; production
database and installed executable are not opened/replaced by them. Normal source
and documentation ingestion uses the existing connected production writer.

Open: representative Rust graph precision/recall, universal test discovery,
runtime coverage/admitted safe narrowing, Cargo multi-unit/feature unification,
doctests/custom frameworks, Go/Java runner configuration, source-state-specific
environment/toolchain qualification and performance comparisons. Empty results
or complete bounded traversal do not establish that unlisted tests are safe to
skip. The complete original ten-item goal remains active.
