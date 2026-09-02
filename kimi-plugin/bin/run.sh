#!/usr/bin/env bash
# Legacy exclusive/offline stdio launcher.
#
# Normal Kimi use is configured by kimi.plugin.json and connects to the shared
# gateway at http://127.0.0.1:7432/mcp. This script deliberately refuses to run
# unless exclusive offline mode is explicit and the gateway is stopped.
#
# Resolution order:
#   binary: $RAG_BIN -> ~/.cargo/bin/rag-mcp -> <repo>/target/release/rag-mcp
#   db:     $RAG_DB_PATH -> disposable file below
set -euo pipefail

if [ "${RAG_EXCLUSIVE_OFFLINE_STDIO:-}" != "1" ]; then
  echo "rag-mcp plugin: use the shared HTTP gateway; set RAG_EXCLUSIVE_OFFLINE_STDIO=1 only for an isolated offline smoke" >&2
  exit 1
fi

if ! command -v curl >/dev/null 2>&1; then
  echo "rag-mcp plugin: curl is required to prove that the shared gateway is stopped" >&2
  exit 1
fi

set +e
curl -s --connect-timeout 1 --max-time 3 -o /dev/null http://127.0.0.1:7432/live
gateway_probe_status=$?
set -e
if [ "$gateway_probe_status" -eq 0 ]; then
  echo "rag-mcp plugin: shared gateway is running; stop it before exclusive offline stdio" >&2
  exit 1
fi
if [ "$gateway_probe_status" -ne 7 ]; then
  echo "rag-mcp plugin: cannot prove that the shared gateway is stopped (curl exit $gateway_probe_status); refusing offline stdio" >&2
  exit 1
fi

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

canonical_db="$HOME/.local/share/rag-mcp/rag.duckdb"
if [ -n "${RAG_DB_PATH:-}" ] && {
  [ "$RAG_DB_PATH" = "$canonical_db" ] ||
    { [ -e "$RAG_DB_PATH" ] && [ -e "$canonical_db" ] && [ "$RAG_DB_PATH" -ef "$canonical_db" ]; }
}; then
  echo "rag-mcp plugin: offline helper refuses the canonical live database; use a disposable path" >&2
  exit 1
fi

offline_dir=""
if [ -z "${RAG_DB_PATH:-}" ]; then
  offline_dir="$(mktemp -d "${TMPDIR:-/tmp}/rag-mcp-kimi-offline.XXXXXX")"
  export RAG_DB_PATH="$offline_dir/rag.duckdb"
fi
mkdir -p "$(dirname "$RAG_DB_PATH")"

export RAG_EMBEDDING_PROVIDER="${RAG_EMBEDDING_PROVIDER:-mock}"
export RAG_DEFAULT_SEARCH_MODE="${RAG_DEFAULT_SEARCH_MODE:-lex}"

child_pid=""

cleanup_offline_dir() {
  if [ -n "$child_pid" ] && kill -0 "$child_pid" 2>/dev/null; then
    echo "rag-mcp plugin: child is still running; preserving temporary database at $RAG_DB_PATH" >&2
    return
  fi
  if [ -n "$offline_dir" ]; then
    rm -rf -- "$offline_dir"
  fi
}

forward_signal_and_exit() {
  signal_name="$1"
  signal_status="$2"
  trap '' HUP INT TERM
  if [ -n "$child_pid" ] && kill -0 "$child_pid" 2>/dev/null; then
    kill -s "$signal_name" "$child_pid" 2>/dev/null || true
    wait "$child_pid" 2>/dev/null || true
  fi
  child_pid=""
  exit "$signal_status"
}

trap cleanup_offline_dir EXIT
trap 'forward_signal_and_exit HUP 129' HUP
trap 'forward_signal_and_exit INT 130' INT
trap 'forward_signal_and_exit TERM 143' TERM

"$BIN" <&0 >&1 2>&2 &
child_pid=$!
set +e
wait "$child_pid"
server_status=$?
set -e
child_pid=""
exit "$server_status"
