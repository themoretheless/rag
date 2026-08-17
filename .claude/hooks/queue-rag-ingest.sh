#!/bin/bash
# Claude Code PostToolUse: queue paths for rag-mcp ingest_file (does NOT open DuckDB).
set -euo pipefail
ROOT="${CLAUDE_PROJECT_DIR:-.}"
QUEUE="$ROOT/.rag/pending-ingest.txt"
mkdir -p "$ROOT/.rag"

INPUT=$(cat)
FILE=$(echo "$INPUT" | jq -r '.tool_input.file_path // .tool_input.path // empty')
[ -n "$FILE" ] || exit 0
[ -f "$FILE" ] || exit 0

case "$FILE" in
  *.md|*.rs|*.toml|*.txt|*.json) ;;
  *) exit 0 ;;
esac
case "$FILE" in
  */target/*|*/.git/*|*/node_modules/*) exit 0 ;;
esac

case "$FILE" in
  /*) ABS="$FILE" ;;
  *) ABS="$ROOT/$FILE" ;;
esac

echo "$ABS" >> "$QUEUE"
sort -u "$QUEUE" -o "$QUEUE" 2>/dev/null || true
echo "rag-mcp: queued for ingest: $ABS" >&2
exit 0
