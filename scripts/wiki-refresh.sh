#!/bin/sh
#
# Background wiki refresh driver for rag-mcp.
#
# Speaks MCP over the running gateway's streamable-HTTP endpoint instead of
# opening rag.duckdb. This is deliberate: the gateway process is the single
# DuckDB writer (see CLAUDE.md "One writer"), so a cron job must never use the
# ingest_file / ingest_project binaries, which open the database directly.
#
# Stages (each can be skipped via env):
#   1. flush  .rag/pending-ingest.txt  -> ingest_file per path
#   2. refresh_stale_wiki dry_run=false -> recompile pages older than their raw
#   3. lint_wiki                        -> report structural problems
#   4. maintain_refresh                 -> fts + dirty graph + wiki index
#
# Requires the gateway to run with RAG_TOOLS=full and a working LLM, otherwise
# stage 2 and 4 are unavailable or silently degrade to list-only. The script
# detects both conditions and says so rather than reporting a false success.
#
# Usage:  scripts/wiki-refresh.sh [--dry-run]
# Env:    RAG_MCP_URL        default http://127.0.0.1:7432/mcp
#         RAG_CRON_WING      wing passed to ingest_file (optional)
#         RAG_CRON_ROOM      room passed to ingest_file (optional)
#         RAG_CRON_MAX_DOCS    cap for refresh_stale_wiki (default: server side)
#         RAG_CRON_SKIP      space-separated stage names: flush refresh lint maintain
#         RAG_CRON_PROJECT_ROOT  project the queue paths must live under
#                              (default: the parent of this script's directory)
#         RAG_CRON_STATE_DIR   where queue/log/lock live
#                              (default: $RAG_CRON_PROJECT_ROOT/.rag)
#
# As a launchd agent this must be installed OUTSIDE ~/Documents and run with
# RAG_CRON_STATE_DIR outside it too: TCC denies a launchd-spawned /bin/sh every
# read under ~/Documents, so the script itself would not even be readable, let
# alone its queue. The gateway is a separately-granted binary and still reads
# the queued paths fine, so only this script's own files need to move.

set -eu

ROOT=${RAG_CRON_PROJECT_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}
STATE=${RAG_CRON_STATE_DIR:-$ROOT/.rag}
MCP_URL=${RAG_MCP_URL:-http://127.0.0.1:7432/mcp}
QUEUE="$STATE/pending-ingest.txt"
LOG="$STATE/wiki-refresh.log"
LOCK="$STATE/wiki-refresh.lock"
SKIP=${RAG_CRON_SKIP:-}
DRY=false
[ "${1:-}" = "--dry-run" ] && DRY=true

mkdir -p "$STATE"

log() { printf '%s %s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*" >>"$LOG"; }

# mkdir is atomic on every POSIX filesystem; macOS has no flock(1).
if ! mkdir "$LOCK" 2>/dev/null; then
  log "SKIP another run holds $LOCK"
  exit 0
fi
cleanup() { rmdir "$LOCK" 2>/dev/null || true; rm -f "$TMP_REQ" "$TMP_RES" 2>/dev/null || true; }
trap cleanup EXIT INT TERM

TMP_REQ=$(mktemp -t ragreq)
TMP_RES=$(mktemp -t ragres)
RPC_ID=0

skipped() {
  for s in $SKIP; do [ "$s" = "$1" ] && return 0; done
  return 1
}

# rpc <method> <params-json> -> prints result object on stdout, empty on error.
# Unwraps the SSE framing ("data: {...}") that the streamable-HTTP transport uses.
rpc() {
  RPC_ID=$((RPC_ID + 1))
  printf '{"jsonrpc":"2.0","id":%s,"method":"%s","params":%s}' "$RPC_ID" "$1" "$2" >"$TMP_REQ"
  curl -sS -m 900 -H 'Content-Type: application/json' \
    -H 'Accept: application/json, text/event-stream' \
    --data-binary @"$TMP_REQ" "$MCP_URL" >"$TMP_RES" 2>/dev/null || return 1
  sed -n 's/^data: //p' "$TMP_RES" | head -n 1 | jq -c '.result // empty'
}

# call <tool> <arguments-json> -> prints the tool's decoded JSON payload.
call() {
  args=$(printf '{"name":"%s","arguments":%s}' "$1" "$2")
  rpc tools/call "$args" | jq -c 'if .isError then {error: .content[0].text}
                                  else (.content[0].text | fromjson) end'
}

# ---- handshake -------------------------------------------------------------

INIT=$(rpc initialize '{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"rag-wiki-cron","version":"0.1.0"}}') || {
  log "FATAL gateway unreachable at $MCP_URL"
  exit 1
}
[ -n "$INIT" ] || { log "FATAL empty initialize response from $MCP_URL"; exit 1; }

printf '{"jsonrpc":"2.0","method":"notifications/initialized"}' >"$TMP_REQ"
curl -sS -m 30 -o /dev/null -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  --data-binary @"$TMP_REQ" "$MCP_URL"

TOOLS=$(rpc tools/list '{}' | jq -r '.tools[].name')
[ -n "$TOOLS" ] || { log "FATAL tools/list returned nothing from $MCP_URL"; exit 1; }
has_tool() { printf '%s\n' "$TOOLS" | grep -qx "$1"; }

log "START dry_run=$DRY tools=$(printf '%s\n' "$TOOLS" | wc -l | tr -d ' ')"

# ---- stage 1: flush the ingest queue ---------------------------------------
#
# The Claude Code hooks only append paths here; nothing else ever drains the
# queue. Paths outside the project root (scratchpad temp files from other
# sessions) are dropped rather than ingested.

if ! skipped flush && [ -s "$QUEUE" ]; then
  REMAIN=$(mktemp -t ragq)
  OK=0
  FAIL=0
  DROP=0
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    case "$path" in
      "$ROOT"/*) ;;
      *) DROP=$((DROP + 1)); continue ;;
    esac
    # Only trust an existence test when we can see the project at all: a
    # launchd-spawned shell gets EPERM under ~/Documents, which would otherwise
    # read as "every queued file is gone" and silently drop the whole queue.
    if [ -d "$ROOT" ] && [ ! -f "$path" ]; then DROP=$((DROP + 1)); continue; fi
    if [ "$DRY" = true ]; then
      log "  would ingest $path"
      printf '%s\n' "$path" >>"$REMAIN"
      continue
    fi
    a=$(jq -nc --arg p "$path" --arg w "${RAG_CRON_WING:-}" --arg r "${RAG_CRON_ROOM:-}" \
      '{path:$p} + (if $w == "" then {} else {wing:$w} end) + (if $r == "" then {} else {room:$r} end)')
    res=$(call ingest_file "$a" || true)
    if printf '%s' "$res" | jq -e '.document_id' >/dev/null 2>&1; then
      OK=$((OK + 1))
    else
      FAIL=$((FAIL + 1))
      log "  ingest FAILED $path: $res"
      printf '%s\n' "$path" >>"$REMAIN"   # keep for the next run
    fi
  done <"$QUEUE"
  # Truncate in place instead of renaming $REMAIN onto it: the installed job's
  # queue is a symlink pointing out of ~/Documents, and a rename would leave a
  # plain file behind that the next launchd run cannot read.
  cat "$REMAIN" >"$QUEUE"
  log "flush ok=$OK failed=$FAIL dropped=$DROP"
fi

# ---- stage 2: recompile stale wiki pages -----------------------------------
#
# A page is stale when wiki.updated_at < raw.updated_at (src/wiki/mod.rs:1289).
# Two traps guarded here:
#   * dry_run defaults to true server-side, so it must be passed explicitly;
#   * with no ChatClient the tool returns success and recompiles nothing
#     (src/wiki/mod.rs:1394), which would otherwise look like a clean run.

if ! skipped refresh; then
  if ! has_tool refresh_stale_wiki; then
    log "SKIP refresh_stale_wiki not exposed - restart the gateway with RAG_TOOLS=full"
  else
    a=$(jq -nc --argjson d "$([ "$DRY" = true ] && echo true || echo false)" \
      --arg m "${RAG_CRON_MAX_DOCS:-}" \
      '{dry_run:$d} + (if $m == "" then {} else {max_docs:($m|tonumber)} end)')
    res=$(call refresh_stale_wiki "$a" || true)
    stale=$(printf '%s' "$res" | jq -r '(.stale // []) | length' 2>/dev/null || echo 0)
    done_n=$(printf '%s' "$res" | jq -r '(.recompiled // []) | length' 2>/dev/null || echo 0)
    errs=$(printf '%s' "$res" | jq -r '(.errors // []) | length' 2>/dev/null || echo 0)
    log "refresh stale=$stale recompiled=$done_n errors=$errs"
    if [ "$DRY" = false ] && [ "$stale" -gt 0 ] && [ "$done_n" -eq 0 ]; then
      log "  WARN stale pages found but nothing recompiled - LLM likely disabled on the gateway (RAG_LLM_ENABLED / RAG_LLM_MODEL)"
    fi
    [ "$errs" -gt 0 ] && log "  errors: $(printf '%s' "$res" | jq -c '.errors')"
  fi
fi

# ---- stage 3: lint ---------------------------------------------------------

if ! skipped lint && has_tool lint_wiki; then
  res=$(call lint_wiki '{}' || true)
  log "lint $(printf '%s' "$res" | jq -c '{issues: ((.issues // []) | length)}' 2>/dev/null || echo "$res")"
fi

# ---- stage 4: cheap maintenance --------------------------------------------
#
# Defaults only: fts reindex + dirty-graph rebuild + wiki index. The heavier
# LLM passes (maintain_organize, maintain_compress L2) stay manual on purpose:
# docs/LOCAL_LLM_MAINTENANCE.md:216 requires dry_run/confirm rails for those.

if ! skipped maintain; then
  if ! has_tool maintain_refresh; then
    log "SKIP maintain_refresh not exposed - needs RAG_TOOLS=full"
  else
    a=$(printf '{"dry_run":%s}' "$([ "$DRY" = true ] && echo true || echo false)")
    res=$(call maintain_refresh "$a" || true)
    log "maintain $(printf '%s' "$res" | jq -c '{applied: ((.applied // []) | length), errors: ((.errors // []) | length)}' 2>/dev/null || echo "$res")"
  fi
fi

log "DONE"
