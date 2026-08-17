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
# Env:    RAG_MCP_URL      default http://127.0.0.1:7432/mcp
#         RAG_CRON_WING    wing passed to ingest_file (optional)
#         RAG_CRON_ROOM    room passed to ingest_file (optional)
#         RAG_CRON_MAX_DOCS  cap for refresh_stale_wiki (default: server side)
#         RAG_CRON_SKIP    space-separated stage names: flush refresh lint maintain

set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
MCP_URL=${RAG_MCP_URL:-http://127.0.0.1:7432/mcp}
QUEUE="$ROOT/.rag/pending-ingest.txt"
LOG="$ROOT/.rag/wiki-refresh.log"
LOCK="$ROOT/.rag/wiki-refresh.lock"
SKIP=${RAG_CRON_SKIP:-}
DRY=false
[ "${1:-}" = "--dry-run" ] && DRY=true

mkdir -p "$ROOT/.rag"

log() { printf '%s %s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*" >>"$LOG"; }

# mkdir is atomic on every POSIX filesystem; macOS has no flock(1).
if ! mkdir "$LOCK" 2>/dev/null; then
  log "SKIP another run holds $LOCK"
  exit 0
fi
cleanup() { rmdir "$LOCK" 2>/dev/null || true; rm -f "$TMP_REQ" "$TMP_RES" "$TMP_SID" 2>/dev/null || true; }
trap cleanup EXIT INT TERM

TMP_REQ=$(mktemp -t ragreq)
TMP_RES=$(mktemp -t ragres)
# The session id lives in a file, not a variable: rpc() is always invoked inside
# command substitution, which runs in a subshell, so a plain assignment would be
# discarded and every call after initialize would be unauthenticated.
TMP_SID=$(mktemp -t ragsid)
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
  sid=$(cat "$TMP_SID" 2>/dev/null || true)
  if [ -n "$sid" ]; then
    curl -sS -m 900 -H 'Content-Type: application/json' \
      -H 'Accept: application/json, text/event-stream' \
      -H "Mcp-Session-Id: $sid" \
      --data-binary @"$TMP_REQ" "$MCP_URL" >"$TMP_RES" 2>/dev/null || return 1
  else
    curl -sS -m 900 -D "$TMP_RES.hdr" -H 'Content-Type: application/json' \
      -H 'Accept: application/json, text/event-stream' \
      --data-binary @"$TMP_REQ" "$MCP_URL" >"$TMP_RES" 2>/dev/null || return 1
    sed -n 's/^[Mm]cp-[Ss]ession-[Ii]d: *//p' "$TMP_RES.hdr" | tr -d '\r' >"$TMP_SID"
    rm -f "$TMP_RES.hdr"
  fi
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

SESSION=$(cat "$TMP_SID")
[ -n "$SESSION" ] || { log "FATAL no Mcp-Session-Id returned by $MCP_URL"; exit 1; }

printf '{"jsonrpc":"2.0","method":"notifications/initialized"}' >"$TMP_REQ"
curl -sS -m 30 -o /dev/null -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -H "Mcp-Session-Id: $SESSION" --data-binary @"$TMP_REQ" "$MCP_URL"

TOOLS=$(rpc tools/list '{}' | jq -r '.tools[].name')
[ -n "$TOOLS" ] || { log "FATAL tools/list returned nothing (session rejected?)"; exit 1; }
has_tool() { printf '%s\n' "$TOOLS" | grep -qx "$1"; }

log "START session=$SESSION dry_run=$DRY tools=$(printf '%s\n' "$TOOLS" | wc -l | tr -d ' ')"

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
    if [ ! -f "$path" ]; then DROP=$((DROP + 1)); continue; fi
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
  mv "$REMAIN" "$QUEUE"
  [ -s "$QUEUE" ] || rm -f "$QUEUE"
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
