# Code-analysis tasks, notes and measured usage

The full MCP surface (`RAG_TOOLS=full`) exposes `task_set`, `note`, `notes` and
`task_stats`. These are database-local control records, not Codex chats, scheduled
work, code-index facts, wiki pages or retrieval documents. Schema v21 adds three
tables without replacing existing documents or index history. Whole-database
persistence is verified; portable document bundles and cross-node replication of
these control records remain unqualified.

## Sessions and labels

```json
{"name":"task_set","arguments":{"name":"payment audit"}}
{"name":"task_set","arguments":{"name":"next investigation","if_generation":1}}
{"name":"task_set","arguments":{"clear":true,"if_generation":2}}
```

Each stdio server instance gets a random default session ID, returned by
`task_set`. Clones share it; independently started servers do not. Labels are
persisted per database/session, last 86,400 seconds by default, and accept a TTL of
1–2,592,000 seconds. Expired labels never attribute new calls to old tasks. An
optional generation precondition prevents a concurrent label update from being
silently overwritten; generations survive clear and expiry. Empty name clears.

To share a label namespace across processes, provide `session_id` on the four
control tools and `rag/task_session` in MCP request `_meta` on index calls:

```json
{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"task_set","arguments":{"name":"payment audit","session_id":"audit-session"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"symbol","arguments":{"symbol":"approve"},"_meta":{"rag/task_session":"audit-session"}}}
```

Shared HTTP servers require explicit `session_id` on the four task tools.
HTTP index calls without that metadata remain unlabelled; they cannot inherit
another client's active label. A session ID is routing metadata, not a credential
or access-isolation boundary. Existing read/write/admin credentials still apply;
read credentials can inspect database-wide labelled statistics and notes.

Statistics aggregate records with the same task **name** across sessions. A
`session_id` selects the active label, not a private task namespace. Supply an
explicit `task` to inspect historical work, an empty `task` for unlabelled work,
or `all_tasks:true` to include all labels. `task` and `all_tasks` are mutually
exclusive. Missing/expired current labels resolve to the empty label, not all tasks.

## Notes

```json
{"name":"note","arguments":{"text":"Investigate generated columns before writes","tags":["schema","follow-up"],"path":"src/payments.php"}}
{"name":"notes","arguments":{"query":"generated columns","limit":20}}
{"name":"notes","arguments":{"task":"payment audit","before_seq":42,"limit":20}}
```

Notes append distinct records even for duplicate text. They retain author/client
assertion, time, task, session, path, tags and current index revision. Every
response says `source:"note"` and `verified_by_index:false`; an index revision
records context, not validation of the assertion. Paths are metadata and are not
opened. Notes never enter document/code search or become graph evidence.

Text is nonempty and at most 4,000 characters; there are at most 10 tags of at
most 40 characters. Oversize inputs are rejected instead of silently truncated.
Tags are stored as JSON, preserving commas. Substring filters are literal and
case-sensitive; `%` and `_` have no wildcard meaning. Results sort by a monotonic
sequence, descending. `next_before_seq` provides stable older-page traversal while
new notes arrive. Limits are 1–500; counts and truncation remain explicit.

## Usage statistics

```json
{"name":"task_stats","arguments":{"task":"payment audit","format":"markdown","limit":10}}
```

Actual MCP index/navigation calls from the fixed tool whitelist are recorded.
The task is captured **before** execution, so a concurrent task change cannot
relabel an in-flight call. A start record is committed first; completion records
success/failure and elapsed wall-clock time. Crashes, cancellation or a failed
completion write leave an explicit unfinished record. If the start cannot be
persisted, the index operation is not dispatched. Mutation outcomes are never
replayed or replaced merely because a completion record failed.

No raw tool arguments, query text, result bodies or error strings are stored.
Known result containers yield returned-record counts; unknown shapes yield null.
These count **returned evidence records**, not correctness or independent facts.
The report separates failures, unfinished calls and successful unmeasured calls,
includes measured nonempty rate only when a denominator exists, and reports zero
calls for an unused label. Recent records are bounded (1–50), totals are not
restricted to that recent window. Statistics and note reads do not count themselves.

Currently only the MCP index/navigation dispatch path is instrumented, not direct
Rust API calls or every REST/CLI route. There is no claim of saved calls, source
freshness at runtime, correctness, gate effectiveness or a calibrated quality
score. Gate decisions are now included in task reports; threshold calibration reports are
available separately (see [GATE_CALIBRATION.md](GATE_CALIBRATION.md)). Their
effectiveness, retention/compaction and broader surface/scale qualification remain
open in the transfer matrix. `task_set` and `note` require
write credentials; `notes` and `task_stats` require read credentials.
