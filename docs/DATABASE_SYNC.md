# Database sync

Each `rag-mcp` process owns its own DuckDB file. A process without
`RAG_PRIMARY_URL` is the primary; a replica points at the primary HTTP gateway.

```bash
export RAG_NODE_ID=m4
export RAG_NODE_NAME=m4.local
export RAG_PRIMARY_URL=http://MAIN_HOST:7432
export RAG_SYNC_INTERVAL_SECS=5
```

Wiki writes are journalled durably. On a replica they enter `sync_outbox`; the
background worker registers the node, pushes pending changes, pulls changes
from other nodes after its durable cursor, applies them locally (including
chunks and graph rebuild), and acknowledges the cursor. The primary assigns a
monotonic `primary_seq`. `(origin_node, origin_seq)` makes retries idempotent.

Current scope is deliberately narrow: `wiki` + `upsert`. Tombstones and raw
corpus/KG/collection replication must be added as explicit typed operations;
unknown operations are rejected rather than silently ignored.

Operational endpoints:

- `GET /v1/sync/status`
- `POST /v1/sync/register`
- `POST /v1/sync/push`
- `GET /v1/sync/pull?node_id=...&after=...&limit=...`
- `POST /v1/sync/ack`

The sync UI reads the status endpoint and distinguishes registered database
nodes from clients merely observed in request logs.
