#!/usr/bin/env bash
# Launches the rag-mcp MCP server for Kimi Code (stdio transport).
#
# Resolution order:
#   binary: $RAG_BIN -> ~/.cargo/bin/rag-mcp -> <repo>/target/release/rag-mcp
#   db:     $RAG_DB_PATH -> ~/.local/share/rag-mcp/rag.duckdb
set -euo pipefail

BIN="${RAG_BIN:-}"
if [ -z "$BIN" ]; then
  for candidate in \
    "$HOME/.cargo/bin/rag-mcp" \
    "$HOME/Documents/Sources/rag/target/release/rag-mcp"; do
    if [ -x "$candidate" ]; then
      BIN="$candidate"
      break
    fi
  done
fi
if [ -z "$BIN" ]; then
  echo "rag-mcp plugin: binary not found; build it (cargo build --release in the rag repo) or set RAG_BIN" >&2
  exit 1
fi

export RAG_DB_PATH="${RAG_DB_PATH:-$HOME/.local/share/rag-mcp/rag.duckdb}"
mkdir -p "$(dirname "$RAG_DB_PATH")"

export RAG_EMBEDDING_PROVIDER="${RAG_EMBEDDING_PROVIDER:-mock}"
export RAG_DEFAULT_SEARCH_MODE="${RAG_DEFAULT_SEARCH_MODE:-lex}"

exec "$BIN"
