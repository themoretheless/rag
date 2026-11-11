# Index-usage gate and confidence threshold reports

`RAG_TOOLS=full` adds `gate`, `calibrate` and read-only `calibration_reports`.
Schema v23 stores gate decisions and calibration reports separately from code,
retrieval documents and agent notes. These tools do not execute application code,
change runtime confidence thresholds or install client hooks.

## Gate decisions

```json
{"name":"gate","arguments":{"payload":{"tool_name":"Bash","tool_input":{"command":"rg payments"},"cwd":"/repo","session_id":"audit-session"}}}
{"name":"gate","arguments":{"mode":"deny","payload":{"tool_name":"Read","tool_input":{"path":"src/payments.rs"},"cwd":"/repo","session_id":"audit-session"}}}
```

Modes: `warn` (default), `ask`, `deny`, `off`. `RAG_GATE_MODE` supplies an optional
process default; a call can explicitly select a mode. Invalid modes are rejected.
This is an **index-usage advisory**, not command-safety analysis, runtime
authorization or a security sandbox. It does not override the application's
existing read/write/admin credentials. The tool itself requires write credentials
because its decision is persisted, including in `off` mode.

Rules within a registered corpus:

- Actual index-query tools are allowed. Declaring an index call in a hook does
  **not** count as completed usage. Prefixes cannot fabricate successful evidence.
- Direct code reads are allowed, with a navigation hint when appropriate.
- File searches and uncertain shell calls check freshness and actual recent use.
  Stale inputs, unknown freshness or no qualifying use produce a guidance-only
  allow in `warn`, ask on the first offence then deny in `ask`, and immediate deny
  in `deny`. Escalation is keyed by session, task, revision and a 15-minute window.
- Calls unrelated to index usage, calls outside registered/allowed corpus roots,
  and calls while the gate is off are not blocked by this advisory.

Bash tree-sitter command nodes distinguish `rg x` from `echo 'rg x'` and comments.
Pipelines, command substitution, simple wrappers and literal `sh/bash/zsh -c`
scripts are recognized. Dynamic commands, unsupported shell/PowerShell/cmd
semantics and depth limits remain explicit unknown classifications. This parser
is neither a shell evaluator nor an exploit detector. Payloads are bounded to
64 KiB, shell command text to 16 KiB and traversal depth is bounded.

A qualifying index use is a **real**, completed, successful MCP query with a
known nonempty returned-record container, within the last 900 seconds, in the
same database, task, session and index revision captured at call start. Empty,
failed, unfinished, older-revision and unknown-result-shape calls do not qualify.
This establishes use of that database revision, not that the earlier query
covered the exact target project. No user-supplied `facts` fields are trusted.
A `SessionStart` event records a sequence barrier: prior and in-flight older calls
cannot carry usage credit into the new session epoch, and ask escalation restarts.
It preserves historical statistics and the independently managed task label.
Use the same session ID as `task_set` / MCP `rag/task_session` metadata; see
[ANALYSIS_TASKS.md](ANALYSIS_TASKS.md).

The longest registered source-manifest root containing the canonical cwd selects
the corpus. Latest immutable manifest inputs are compared with current document
revisions and current allowlisted source files decoded with the ingestion decoder. Format changes, document
updates/deletions and new ingested documents invalidate stored-input freshness.
Decoded-source hashes catch same-length text changes; modification-time shortcuts and
freshness caches are not used. The gate rechecks index revision and stored input
changes after live reads, before committing its decision. It never rebuilds the
index, discovers newly added filesystem files, or traverses every directory.
Freshness therefore means **indexed manifest inputs**, not whole-repository
exhaustiveness or an atomic cross-file filesystem snapshot.

Live checks cover at most 256 files, 4 MiB per file and 16 MiB total, with a bounded
sentinel byte for detecting a concurrent growth past the cap. Missing known files
are stale; unavailable/undecodable/oversize inputs, revision races, incomplete checks
and budget exhaustion produce unknown freshness. Symlink targets must still be
within `RAG_INGEST_ROOTS`; only regular files are read. No partial hash is accepted. Byte budgets count original file bytes, including BOMs; checksums compare decoded UTF-8 source. Encoding-only changes can preserve freshness, and legacy decoder choices remain explicitly heuristic in live source evidence.
Raw command/input text is never persisted. Decisions record only classification,
source-check summaries, current task/session/revision and rule/outcome. `task_stats`
aggregates **all** matching decisions and bounds only its recent list. Counts are
observed outcomes, not a measured improvement in agent correctness or efficiency.

### Optional hook executable

Build and invoke manually; no settings are automatically installed or activated:

```sh
cargo build --bin rag_gate --no-default-features
# Hook event JSON is supplied on stdin:
rag_gate --mode warn
# Inspect the entire decision without applying hook exit semantics:
rag_gate --mode deny --json
```

The adapter and MCP tool call the same rule implementation. Hook output follows
`hookSpecificOutput` for `PreToolUse`: ask is explicit permission-decision JSON;
deny is empty stdout plus reason on stderr and exit code 2. Warn remains allow
with context, not an implicit approval prompt. `--json` prints an inspection
report and exits normally; it is not the client-hook transport.

When `RAG_HTTP_BIND` is configured, the adapter uses that **loopback** MCP server,
with `RAG_HTTP_WRITE_TOKEN` or `RAG_HTTP_ADMIN_TOKEN` when configured. Unspecified
bind addresses map to loopback. Proxies and redirects are disabled, response size
is capped at 256 KiB and the request has a ten-second timeout. This avoids opening
DuckDB from a second process while the server owns its writer lock. With no HTTP
listener configured, the adapter opens the local database directly; it cannot
share a live stdio-only server's DuckDB writer. Enable a loopback HTTP listener
for that use case. A missing local database is reported without creating one.

A missing hook session ID gets a unique anonymous ID, never another client's
label. On operational failure the adapter reports unknown/unavailable, no saved
decision, and follows the explicitly selected mode: warn allows with guidance,
ask requests confirmation and deny exits 2. Invalid CLI arguments/modes exit 2.
Commands are never executed, rewritten or passed as a replacement input.
Other clients' hook protocols, remote HTTP endpoints, cross-platform installers
and production activation remain separate integration qualification work.

## Confidence calibration

```json
{"name":"calibrate","arguments":{"kind":"confidence","min_precision":0.9,"samples":{"dataset_name":"labelled-call-candidates-v1","training":[{"id":"train-1","score":0.9,"relevant":true},{"id":"train-2","score":0.1,"relevant":false}],"validation":[{"id":"holdout-1","score":0.95,"relevant":true},{"id":"holdout-2","score":0.05,"relevant":false}]}}}
{"name":"calibration_reports","arguments":{"limit":20}}
```

Caller-supplied confidence scores must be finite and in [0,1]. IDs are nonempty
and unique across training/validation splits; overlapping declared IDs are
rejected. At most 2,048 labelled samples are accepted, with ID/dataset-name bounds.
No automatic holdout is invented or training results presented as validation.

Selection maximizes training F1 subject to the requested **training point
precision** floor, breaking ties toward the larger threshold. Prediction is
`score >= threshold`, so equal scores always move together. Both training classes
are required. No feasible threshold yields an explicit null threshold, no selected
positives and undefined precision; rejecting everything is not perfect precision.
Reports include class counts, TP/FP/TN/FN, precision, recall, F1 and the 95% Wilson
lower bound for precision. A supplied holdout is evaluated separately. Missing
holdout, missing validation classes, target misses and fewer than 20 examples in
either split have explicit statuses. Statistical bounds and the point target are
reported separately; neither proves general correctness or future performance.

A legacy-style `samples` object containing a flat parameter map or nested `params` / asserted `sample_size`
is preserved as `recorded_unvalidated` with **zero measured samples**. Custom
`kind` names can record such unvalidated parameter objects; empirical fitting is
currently for `confidence`. Flat parameter maps are normalized into `params`; conflicting flat/nested keys
and unknown keys mixed into labelled datasets are rejected. Parameter objects are
capped at 64 KiB. Oversize or
invalid data is rejected, not silently truncated. Raw labelled rows/IDs are not
stored; reports retain a canonical dataset fingerprint, sizes, aggregate metrics,
parameter assertions and index-revision context. That revision is context, not
proof that supplied labels describe that revision.

`applied` and `production_qualified` remain false: the operation does not mutate
query thresholds or claim to have established label accuracy, representativeness,
score meaning, hidden duplicate independence or future stability. Applying a
threshold to a particular runtime feature requires its own integration and
qualification. Calibration writes require write credentials; report history
requires read credentials. Reports are append-only through this API, with stable
sequence cursors, at most 200 records and a 256 KiB aggregate report budget per
history page. Small synthetic tests qualify implementation behavior, not a
production confidence model or whole-RAGRAF superiority benchmark.
