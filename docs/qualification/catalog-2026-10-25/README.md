# Catalog registry/pagination subset — 2026-10-25

The reference `op_catalog` is a project registry, not merely a file listing. The
previous local operation exposed an immutable manifest but no project registry or
continuation. The [new contract](../../CODE_CATALOG.md) preserves that default and
adds captured project inventory, declared corpus hash comparison, current sync
roots and explicit unverified states.

File pages select metadata only and pin immutable revisions across later index
publications. Project pages pin the comparison revision and reject changed current
observations. Exact filters are bound SQL parameters; same paths/null projects do
not collapse identities. The lean file query explicitly does not verify source
checksums or analysis payload integrity; it exposes availability and never fetches
source-text payloads into Rust. It does not weaken the separate freshness/code
read checks.

Five new owned-DuckDB tests cover old-revision page continuation after reindex,
cursor scope/revision mismatch, metadata-only invalid analysis handling, projects
before indexing, captured match/drift/removal states, null vs "null" project keys,
changed-project observation guards, preserved HTML source hashes, metadata fallback semantics and literal
project parameter binding. Existing drift/read/ack tests remain required.

The real stdio MCP scenario verifies the public schema, legacy files behavior,
registration before first index, root provenance, captured-vs-live distinction,
project pages/observation conflicts, capture drift and refresh, old file-page
continuation through a newer publication and historical project comparison. It
uses an isolated mock-provider database; production files/services are untouched.

Reference scope: `op_catalog` and `catalog.Entry` in RAGRAF revision
`075caf86dc2f6274b9c48487e521915bcdfeaf2c`, inspected locally. Independent project
storage/revisions, config/watch hashes, index-enable settings, explicit empty
registration, run histories, representative corpus comparison and scale gates
remain open. This subset is not full registry or product parity.

```sh
cargo test --lib code_index::catalog --locked --offline
cargo build --bin rag-mcp --locked --offline
python3 scripts/smoke_catalog.py target/debug/rag-mcp /tmp/catalog.json
```

An initial workspace run failed two existing UI gateway tests at a socket read:
`authenticated_requests_do_not_follow_redirects` and
`bearer_token_is_sent_only_in_the_authorization_header`. The accepted test socket
inherited nonblocking mode on macOS, racing the client's header write. The test
server now explicitly clears nonblocking mode before its unchanged 5-second read
timeout. Production gateway/client behavior is unchanged. A controlled delayed
client [probe](socket-probe.rs) reproduced the original immediate WouldBlock
(0 ms) and a successful 99 ms blocking read after the fix
([raw evidence](socket-probe.json)); these timings diagnose the race, not product
performance. Reproduce with `rustc --edition 2021 socket-probe.rs -o /tmp/socket-probe`
and `/tmp/socket-probe`. Final workspace results below supersede the initial run.

Final verification: **1,195 workspace tests passed, 0 failed, 1 ignored**,
including all five new catalog tests. Workspace Clippy with `-D warnings`,
formatting and `git diff --check` passed. The rebuilt binary passed
[catalog MCP evidence](runtime.json), [routes](routes-regression.json) and
[index-status](index-status-regression.json) regressions. The corrected UI gateway
tests also passed [20 repeated rounds / 260 test executions](gateway-repeats.json)
with two test threads. Repeated executions are separate from the workspace total.
[Initial failure evidence](initial-workspace-failure.txt) is retained;
[verification manifest](verification.json) records source/binary hashes and the
final results. These mock-provider checks do not qualify live filesystem/config
freshness, production service or representative scale.
