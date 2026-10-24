# Gate and calibration qualification — 2026-10-24

Verified subset: `gate`, opt-in `rag_gate`, task-scoped decision summaries,
`calibrate`, `calibration_reports`, bounded shared live-source reads and roles.
This is not whole-RAGRAF parity, production deployment or a superiority benchmark.

Evidence:

- Complete library suite: **740 passed**, no failures, four test threads.
  File-ingestion integration suite: **3 passed**, no failures. This includes
  SessionStart sequence barriers, ask escalation, real/empty/failed/pending/stale
  usage distinctions, changed bytes, missing/oversize inputs, 256-file budgets,
  shell-string false positives, calibration tie grouping and holdout uncertainty.
- The subsequent flat reference-parameter compatibility addition passed all
  **4 calibration tests**. It normalizes flat parameter assertions without
  pretending they are labelled observations.
- Both actual executables built successfully with default features disabled.
- Final actual stdio, authenticated loopback HTTP MCP and hook-executable smoke:
  **passed**. [mcp-hook-smoke.json](mcp-hook-smoke.json) records the checks. Source
  identities are in [source-sha256.json](source-sha256.json).

The fixture proves default warn behavior, declared index calls receiving no use
credit, first ask then deny, a real successful query enabling indexed use,
SessionStart invalidating old credit/escalation and same-length source changes
causing stale decisions. Task counters aggregate actual persisted decisions and
omit raw command text. Read credentials cannot mutate gate/calibration state but
can read report history. Actual calibration returns holdout metrics and small-
sample status; unlabelled asserted sample counts remain zero measured samples;
cross-split IDs are rejected and report cursors are stable.

The real hook executable emits explicit ask JSON and deny exit code 2. While the
server owns DuckDB it uses the authenticated loopback MCP bridge; it opens the
file directly after normal stdio EOF shutdown. A proposed redirection command
never creates its marker file. No client hook was installed or activated, no
user database was modified and no runtime confidence threshold was changed.
The runtime corpus, credentials and database are disposable synthetic fixtures.

Reproduction:

```sh
cargo test --lib --test file_ingest_test --no-default-features -- --test-threads=4
cargo test --lib calibration::tests --no-default-features
cargo build --bin rag-mcp --bin rag_gate --no-default-features
python3 scripts/smoke_gate_calibration.py target/debug/rag-mcp /tmp/gate-calibration-report.json
```

## Observed recovery failure (historical; follow-up now available)

The original failure below was subsequently fixed for new rebuilds. See the
[FTS recovery follow-up](../fts-recovery-2026-10-24/README.md), including actual
SIGKILL and signal-shutdown evidence. This section preserves the original finding
and its then-open scope; it is not a current claim that the fix is absent.

Force-killing the dual stdio/HTTP fixture after FTS rebuild, including SIGINT when
that serving path has not installed a shutdown handler, can leave a WAL that the
next process cannot replay. The observed failure is:

```text
Dependency Error: Failure while replaying WAL file ...
Cannot drop entry "fts_main_chunks" because there are entries that depend on it.
table "terms" depends on schema "fts_main_chunks".
table "stats" depends on schema "fts_main_chunks".
Use DROP...CASCADE to drop all dependents.
```

This was reproduced in disposable data only. The gate adapter correctly reports
unavailable (warn allows with guidance; explicitly selected deny exits 2) instead
of claiming freshness or losing the failure silently. Normal stdin EOF shutdown
runs the existing checkpoint and the offline hook reopens successfully. The
current smoke deliberately verifies that normal path; it does **not** claim to
have fixed crash recovery. To reproduce the failing variant, replace the fixture's
`process.stdin.close()` / normal wait before offline invocation with a forced
termination; the diagnostic probe in the script surfaces the replay failure.

Required follow-up: investigate dependency-ordered teardown of derived FTS objects
and robust signal/shutdown handling, then prove WAL replay preserves committed
source, index/control records and existing retrieval behavior. Deleting a WAL or
silently discarding committed raw/control writes is not an acceptable repair.
This issue remains a concrete open acceptance requirement in the transfer matrix.

Other limits: snapshot-input freshness does not discover new filesystem files or
prove an atomic cross-file state; target-project query relevance is not inferred
from database-level use. Untrusted scores/labels do not establish production
confidence, performance or savings. Optional tokenizer modes, every platform/hook
client, retention and direct REST/CLI telemetry coverage remain unqualified.
See [GATE_CALIBRATION.md](../../GATE_CALIBRATION.md) and
[RAGRAF_FEATURE_MATRIX.md](../../RAGRAF_FEATURE_MATRIX.md).
