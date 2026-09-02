# Claude Code + rag-mcp

## Shared gateway (recommended)

The one `local.rag-mcp` process owns
`/Users/themoretheless/.local/share/rag-mcp/rag.duckdb`. Claude Code connects to
`http://127.0.0.1:7432/mcp`; it does not start another server or carry DB/embed
environment variables.

## Project scope

Repo already has `.mcp.json`. In Claude Code:

```bash
cd /Users/themoretheless/Documents/Sources/rag
claude
```

If the server shows as **Pending approval**, approve it when prompted (or use `/mcp`).

## Import from Claude Desktop

```bash
claude mcp add-from-claude-desktop
```

## Check

```bash
claude mcp list
claude mcp get rag-mcp
```

## User-scope HTTP config (optional)

```bash
claude mcp remove rag-mcp -s user
claude mcp add --transport http rag-mcp http://127.0.0.1:7432/mcp -s user
```

Embedding configuration belongs to the gateway service. If exclusive legacy
stdio is required, first stop `local.rag-mcp`; use the canonical DB only while
no other owner exists, or use an explicitly disposable `/tmp` database.
