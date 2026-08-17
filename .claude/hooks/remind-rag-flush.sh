#!/bin/bash
# Claude Code Stop: remind agent to flush pending ingest via MCP.
set -euo pipefail
ROOT="${CLAUDE_PROJECT_DIR:-.}"
QUEUE="$ROOT/.rag/pending-ingest.txt"
[ -s "$QUEUE" ] || exit 0
N=$(wc -l < "$QUEUE" | tr -d ' ')
# Inject context for the next model turn when Stop supports additionalContext.
printf '%s\n' "{\"hookSpecificOutput\":{\"hookEventName\":\"Stop\",\"additionalContext\":\"rag-mcp: $N path(s) in .rag/pending-ingest.txt. Call ingest_file for each absolute path via MCP, then clear the file. If wiki hub runbook/architecture changed, get_wiki_page + update_wiki_page with if_match_revision.\"}}"
exit 0
