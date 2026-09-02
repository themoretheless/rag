# Project: downloader (flowget) in rag-mcp

## Placement

| Field | Value |
|-------|--------|
| project / wing | `downloader` |
| room | derived subdirectory or `root` |
| source root | `/Users/themoretheless/Documents/Sources/downloader` |
| database | canonical shared gateway store |

Downloader is a project partition inside the main corpus, not a separate MCP
server or DuckDB file.

## Sync through the gateway

The running `local.rag-mcp` service must already allow the source root through
`RAG_INGEST_ROOTS`. Submit an incremental job to its sole-writer API:

```bash
curl -sS -X POST http://127.0.0.1:7432/v1/jobs/sync \
  -H 'content-type: application/json' \
  --data '{
    "path": "/Users/themoretheless/Documents/Sources/downloader",
    "wing": "downloader",
    "room": null,
    "remove_deleted": false
  }'
```

Poll the returned job id until it reaches a terminal state: `succeeded`,
`completed_with_errors`, `failed`, or `cancelled`. For the latter two, inspect
the returned `error` and partial `report` before retrying:

```bash
curl -sS http://127.0.0.1:7432/v1/jobs/JOB_ID
```

Embedding provider/model/dimensions come from the shared gateway. Do not start a
project-specific process with different embedding settings.

## MCP client

For Claude Code and other HTTP-capable clients, see
[`examples/downloader.mcp.json`](../examples/downloader.mcp.json). It connects to
the shared gateway and does not carry a database path. Claude Desktop uses the
`mcp-remote` bridge shown in [`CONNECT.md`](CONNECT.md).

Search scoped to this project:

```text
search query="queue lock" wing=downloader mode=hybrid
list_documents wing=downloader
list_sources wing=downloader
```

## Actualize after code changes

Submit the same source-sync job. Manifest preflight skips healthy unchanged
files; changed content is committed under the existing project partition.
