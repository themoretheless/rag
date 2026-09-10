# Database sync

Each `rag-mcp` process owns its own DuckDB file. A process without
`RAG_PRIMARY_URL` is the primary; a replica points at the primary HTTP gateway.

```bash
export RAG_NODE_ID=m4
export RAG_NODE_NAME=m4.local
export RAG_PRIMARY_URL=http://MAIN_HOST:7432
export RAG_SYNC_INTERVAL_SECS=5
```

For an authenticated primary, provision `RAG_PRIMARY_TOKEN` in the replica's
private service environment with the primary's **Admin** credential. The worker
sends it as `Authorization: Bearer ...` for register, push, pull and ack. A Read
or Write token is insufficient for replication. Do not put the raw value in
`RAG_PRIMARY_URL`, repository config, logs or commits.

The primary's incoming role tokens (`RAG_HTTP_READ_TOKEN`,
`RAG_HTTP_WRITE_TOKEN`, `RAG_HTTP_ADMIN_TOKEN`) and the replica's own incoming
credentials are independent of this outgoing credential. A primary listening
outside loopback must have `RAG_HTTP_ALLOW_REMOTE=true` and at least one valid
role token; in practice replication requires its Admin token. Configure the
primary and all replicas **before restarting an existing remote deployment**.
Without tokens, the new binary refuses non-loopback startup before opening the
Store. Use HTTPS or a protected tunnel for remote traffic; see
[authentication and migration](AUTHENTICATION.md).

Wiki text creation, updates and revision restores commit the page, chunks,
graph, catalog, audit entry and replication event in one transaction. On a
replica the event enters `sync_outbox`; on the primary it enters the canonical
`sync_changes` journal. Wiki restore now rolls back its document, derived state
and journal if audit logging fails; it no longer reports a successful restore
with a missing audit entry. The saved revision's metadata remains intact, CAS
still checks the current head, and chunk embedding runs once.
An application-supplied agent name such as `sync:...`
cannot suppress journalling. Incoming replication uses a typed internal apply
path and does not generate another local event.

Generic atomic writes and direct document metadata updates also journal changes
to a `layer=wiki` page with a canonical `wiki://slug` URI: title, content, wing,
room, kind or metadata
in the same transaction as the document and catalog. A metadata edit made after
a wiki write therefore has its own event and cannot be lost when the earlier
event is replayed. Journal failure rolls back the metadata update as well.

The worker registers the node, pushes pending changes, pulls the canonical
stream after its durable cursor, then acknowledges delivery. Primary sequence
allocation and the accepted page write share one transaction. Event identity
is `(origin_node, origin_seq)`: an exact retry returns the previous canonical
sequence without writing another revision; reuse with different content returns
`409 Conflict`. The entire push batch is validated before the first write, and
each event commits atomically. If a later event or the connection fails, a retry
can resume the already committed prefix safely. Timestamps on the wire are UTC
RFC3339 with microsecond precision.

Every node receives the same stream **including its own events**. Conflicting
offline edits resolve in the primary's commit order, not by client wall clocks.
A replica commits each applied event and its local cursor together; retries do
not repeat page revisions. If a newer local edit still has an unsent outbox
event, an older pulled event is journalled and advances the cursor while leaving
the local draft visible. Once the pending edit is pushed, its own canonical
event reconciles the page in the shared order. With successful subsequent
cycles and no new edits, replicas converge. A failed delivery acknowledgement
is retried even when the next pull has no new events.

Each wiki event carries title, content, placement, kind, catalog category/summary
and a complete metadata snapshot, including captured `source_versions` hashes.
Legacy events without `metadata_json` remain accepted and retain unrelated local
metadata. The scope remains `wiki` + `upsert`, including metadata-only updates to
the fields listed above. Status, pinned, boost, source-file ownership and layer
are local-only fields; changing only these fields adds no event. Lifecycle
operations, deletes/tombstones, raw corpus, KG and collection mutations are not
replicated.
Existing corpus bootstrap, journal compaction, node reassignment and automatic
conflict merging need separate protocols; this transport alone does not backfill
pages that predate its journal.

Push batches are limited by serialized bytes as well as count, so they fit the
gateway's 1 MiB HTTP body limit. The worker processes up to 100 queued events per
cycle; the transport accepts at most 500. A new event larger than 1,000,000
serialized bytes is rejected before its page transaction commits: shorten its
content or metadata. Existing oversized outbox rows are preserved and reported
as blocked legacy entries, never silently deleted or marked sent. Resolve such
an entry through an explicit operator repair before retrying the queue.

Operational endpoints:

- `GET /v1/sync/status` — Read
- `POST /v1/sync/register` — Admin
- `POST /v1/sync/push` — Admin
- `GET /v1/sync/pull?node_id=...&after=...&limit=...` — Admin
- `POST /v1/sync/ack` — Admin

The sync UI reads the status endpoint and distinguishes registered database
nodes from clients merely observed in request logs. `node_id` is replication
identity, not authentication; the Admin Bearer credential authorizes these
operations. Token rotation on the primary requires updating `RAG_PRIMARY_TOKEN`
on its replicas and restarting the affected processes.
