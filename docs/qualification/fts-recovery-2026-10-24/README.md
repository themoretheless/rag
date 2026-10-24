# FTS WAL recovery and shutdown qualification — 2026-10-24

The reproduced FTS overwrite failure is fixed for newly executed rebuilds.
This report closes that specific regression, not whole-RAGRAF parity or every
possible corruption/power-loss/in-flight shutdown scenario.

## Cause and change

A checkpointed `fts_main_chunks` schema contains dependent macros/tables. The
extension's `create_fts_index(... overwrite=1)` path could leave a WAL schema-drop
record that replay attempted before dependent objects were removed. A killed
process then failed to open its database with a schema dependency error.

The replacement plan operates only in the reserved derived FTS schema:

1. Collect and validate owned macros/views/tables, quoting SQL identifiers.
2. Remove the derived index-generation marker before destructive derived work.
   Chunk/vector generations are not advanced by a force rebuild.
3. Drop leaf objects in dependency order (retrying dependency-blocked leaves),
   then drop the empty schema without CASCADE.
4. Create FTS with `overwrite=0`; publish clean generation only after success.

Unknown object kinds/dependencies fail explicitly into the existing live-row
term-frequency fallback. Neither raw/control tables nor a WAL are deleted as a
recovery shortcut. On a crash during rebuild, missing generation metadata stays
dirty and ordinary refresh reconstructs a usable BM25 index. Clean warm reads
keep the existing no-rebuild behavior.

SIGTERM/SIGINT now participate in all serving modes, including an uninitialized
stdio handshake. A shared cancellation token stops HTTP acceptance/MCP streams;
HTTP drain is bounded at ten seconds, then the existing final database checkpoint
runs. Runtime shutdown is bounded at five seconds because Tokio stdin may retain
an idle blocking read after cancellation. This is not a guarantee that every
unacknowledged request/background job completes before a shutdown deadline.

## Evidence

- Red regression before the fix: a live child with **checkpointed prior FTS** was
  killed after overwrite; reopening failed with the exact dependency error.
  Testing only a new database or a normal process exit did not reproduce it, so
  the regression deliberately matches the actual startup checkpoint sequence.
- Green regression: seven live-child kill points — dirty marker, first leaf,
  all leaves, schema removal, new index built, metadata published and completed
  rebuild. [rebuild-boundaries.json](rebuild-boundaries.json).
- Every boundary preserves committed raw source, base chunks/embedding vectors,
  code snapshot, task label/note, gate decision and calibration report. Incomplete
  generations are dirty; the next lexical refresh recovers BM25. A direct SQL
  `fts_main_chunks.match_bm25` query verifies the physical backend rather than
  accepting a silently degraded per-query fallback.
- Complete library suite: **742 passed**, no failures; file-ingestion integration:
  **3 passed**, no failures, with four test threads. Final test-only strengthening
  (direct BM25 SQL and embedding-vector checks) also passed the seven-boundary
  regression after that full run.
- Both actual executables built successfully. The real-process matrix passed
  **12 scenarios**: stdio, dual, HTTP-only and dual without a stdio initialize
  request, each under SIGTERM/SIGINT/SIGKILL.
  [process-matrix.json](process-matrix.json).
- Every process case begins with a nonempty committed WAL, two source revisions
  and completed FTS refreshes. The test never removes/replaces WAL or snapshots.
  After restart it compares raw documents/chunks, immutable manifest, original
  HTML blob, note/calibration identities and task/gate state. Lexical result IDs,
  ordering and scores are unchanged; the clean warm FTS generation is reused.
  BM25 creation logs and absence of per-query fallback are checked explicitly.
- Graceful signals exit zero and log a successful final checkpoint. SIGKILL
  actually terminates the process; a new process replays the WAL successfully.

Tests use only disposable local databases, synthetic sources, mock embeddings
and the installed DuckDB BM25 extension. Production data/services were not
restarted or migrated. [source-sha256.json](source-sha256.json) identifies owning
source/test inputs in the dirty worktree, not a committed/deployed release.

Reproduction:

```sh
cargo test --lib db::fts::tests::fts_overwrite_wal_replays --no-default-features -- --nocapture
cargo test --lib --test file_ingest_test --no-default-features -- --test-threads=4
cargo build --bin rag-mcp --bin rag_gate --no-default-features
python3 scripts/smoke_fts_recovery.py target/debug/rag-mcp /tmp/fts-recovery-report.json
```

## Limits and remaining work

This prevents the reproduced overwrite sequence in future writes. It does not
rewrite or silently discard an already-malformed legacy WAL that DuckDB cannot
open; recovery of such a file needs a separately verified non-destructive path.
No user WAL was repaired/discarded here. Hardware power loss, torn sectors,
unrelated engine corruption, every extension/version/platform combination and
all long-running in-flight mutation shutdown cases remain unqualified. Unit tests
honestly identify extension-unavailable fallback; this machine exercised BM25.
The process/signal matrix is Unix-specific. Optional tokenizer features were not
part of the build. The overall transfer goal remains open in
[RAGRAF_FEATURE_MATRIX.md](../../RAGRAF_FEATURE_MATRIX.md).
