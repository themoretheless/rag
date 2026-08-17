#!/usr/bin/env bash
# Curl smoke for rag-mcp HTTP gateway: /health, /v1/wiki, /v1/backlinks.
#
# Prerequisites: running gateway (RAG_HTTP_BIND), curl, jq.
# Usage:
#   ./scripts/smoke_http_wiki.sh
#   BASE_URL=http://127.0.0.1:7432 ./scripts/smoke_http_wiki.sh
#
# Exit 0 on pass; non-zero on first failure.

set -euo pipefail

BASE_URL="${BASE_URL:-http://127.0.0.1:7432}"
BASE_URL="${BASE_URL%/}"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

pass() {
  echo "ok: $*"
}

command -v curl >/dev/null 2>&1 || fail "missing command: curl"
command -v jq >/dev/null 2>&1 || fail "missing command: jq"

echo "smoke_http_wiki against ${BASE_URL}"

# --- GET /health ---
health_body="$(curl -fsS --max-time 5 "${BASE_URL}/health")" \
  || fail "GET /health (is gateway up at ${BASE_URL}?)"
echo "${health_body}" | jq -e '.ok == true' >/dev/null \
  || fail "/health expected ok=true, body=${health_body}"
pass "/health ok=true documents=$(echo "${health_body}" | jq -r '.documents // "?"') mcp_http=$(echo "${health_body}" | jq -r '.mcp_http // "?"')"

# --- GET /v1/wiki (catalog, no body) ---
wiki_body="$(curl -fsS --max-time 10 "${BASE_URL}/v1/wiki")" \
  || fail "GET /v1/wiki"
echo "${wiki_body}" | jq -e '.ok == true and (.pages | type == "array") and (.count | type == "number")' >/dev/null \
  || fail "/v1/wiki expected {ok, count, pages[]}, body=${wiki_body}"
wiki_count="$(echo "${wiki_body}" | jq -r '.count')"
if [[ "${wiki_count}" != "0" ]]; then
  echo "${wiki_body}" | jq -e '.pages[0] | (.id | type == "string") and (.title | type == "string") and (.uri | type == "string")' >/dev/null \
    || fail "/v1/wiki pages[0] missing id/title/uri, body=${wiki_body}"
fi
pass "/v1/wiki ok=true count=${wiki_count}"

# --- GET /v1/backlinks?id=... ---
page_id="$(echo "${wiki_body}" | jq -r '.pages[0].id // empty')"
if [[ -z "${page_id}" ]]; then
  # Empty catalog: unknown id should still return ok + empty backlinks (not 404).
  page_id="smoke-missing-doc-id"
  pass "/v1/wiki empty; probing backlinks with placeholder id"
fi

bl_body="$(curl -fsS --max-time 10 --get "${BASE_URL}/v1/backlinks" --data-urlencode "id=${page_id}")" \
  || fail "GET /v1/backlinks?id=${page_id} (route missing on old binary?)"
echo "${bl_body}" | jq -e '.ok == true and (.backlinks | type == "array") and (.count | type == "number")' >/dev/null \
  || fail "/v1/backlinks expected {ok, count, backlinks[]}, body=${bl_body}"
bl_count="$(echo "${bl_body}" | jq -r '.count')"
if [[ "${bl_count}" != "0" ]]; then
  echo "${bl_body}" | jq -e '.backlinks[0] | (.id | type == "string") and (.label | type == "string")' >/dev/null \
    || fail "/v1/backlinks backlinks[0] missing id/label, body=${bl_body}"
fi
pass "/v1/backlinks ok=true id=${page_id} count=${bl_count}"

# Missing id query param must not succeed as 200 with ok=true.
missing_code="$(curl -sS -o /tmp/smoke_http_wiki_backlinks_missing.json -w '%{http_code}' --max-time 5 "${BASE_URL}/v1/backlinks")" \
  || true
if [[ "${missing_code}" == "200" ]]; then
  if jq -e '.ok == true' /tmp/smoke_http_wiki_backlinks_missing.json >/dev/null 2>&1; then
    fail "/v1/backlinks without id returned 200 ok=true (expected 4xx)"
  fi
fi
if [[ "${missing_code}" =~ ^2 ]]; then
  fail "/v1/backlinks without id returned HTTP ${missing_code} (expected 4xx)"
fi
pass "/v1/backlinks without id -> HTTP ${missing_code}"

echo "smoke_http_wiki: all checks passed"
