#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_fetch_endpoints.py [path/to/rag-mcp] [report.json]
Never uses the user's database, watcher or external embedding/LLM service.
"""
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp").resolve()
    report = Path(sys.argv[2]) if len(sys.argv) > 2 else None
    with tempfile.TemporaryDirectory(prefix="rag-fetch-endpoints-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "client.ts": "fetch('/read'); window.fetch('/write', {method:'POST'}); fetch('/unknown', {...opts}); fetch(`/dynamic/${id}`);",
            "view.html": "<html>\n<script>\nfetch('/inline', {method:'DELETE'});\n</script><script src='/bundle.js'>fetch('/ignored')</script><script type='application/json'>fetch('/json')</script></html>",
        }
        for name, content in fixtures.items():
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        env = {key: value for key, value in os.environ.items() if not key.startswith("RAG_")}
        env.update(RAG_DB_PATH=str(root / "test.duckdb"), RAG_INGEST_ROOTS=str(source),
                   RAG_EMBEDDING_PROVIDER="mock", RAG_EMBEDDING_DIMS="16",
                   RAG_LLM_ENABLED="false", RAG_TOOLS="full")
        with (root / "stderr.log").open("w") as stderr:
            process = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                       stderr=stderr, text=True, env=env)
            incoming = queue.Queue()
            def receive():
                for line in process.stdout:
                    incoming.put(json.loads(line))
                incoming.put(None)
            threading.Thread(target=receive, daemon=True).start()
            request_id = 0
            def request(method, params, allow_error=False):
                nonlocal request_id
                request_id += 1
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", id=request_id, method=method, params=params)) + "\n")
                process.stdin.flush()
                deadline = time.monotonic() + 60
                while True:
                    response = incoming.get(timeout=max(0.01, deadline - time.monotonic()))
                    assert response is not None, "MCP process exited"
                    if response.get("id") == request_id:
                        if allow_error and "error" in response:
                            return response["error"]
                        assert "error" not in response, response
                        return response["result"]
            def call(name, arguments):
                result = request("tools/call", dict(name=name, arguments=arguments))
                assert not result.get("isError"), result
                return json.loads(next(c["text"] for c in result["content"] if c["type"] == "text"))
            try:
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="fetch-endpoints-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                sync = call("sync_sources", dict(path=str(source), wing="routes"))
                assert not sync.get("errors"), sync
                tools = request("tools/list", {})
                route_tool = next(t for t in tools["tools"] if t["name"] == "routes")
                assert "endpoint references" in route_tool["description"], route_tool
                result = call("routes", {})
                before = result["index_rev"]
                rows = result["items"]
                assert len(rows) == 5, result
                js = [r for r in rows if r["reference_kind"] == "js:fetch"]
                assert len(js) == 4, result
                assert all(r["kind"] == "http_endpoint_reference" and r["method"] is None and r["symbol_id"] is None for r in js), result
                assert next(r for r in js if r["route"] == "/write")["request_method"] == "POST", result
                assert next(r for r in js if r["route"] == "/unknown").get("request_method") is None, result
                inline = next(r for r in js if r["route"] == "/inline")
                assert inline["line"] == 3 and inline["request_method"] == "DELETE", result
                assert result["detail"]["fetch_identity_verified"] is False, result
                assert result["detail"]["fetch_default_method_inferred"] is False, result
                assert result == call("routes", {}), "unstable endpoints"
                path = source / "client.ts"
                path.write_text(fixtures["client.ts"].replace("'POST'", "'PUT'"))
                call("ingest_file", dict(path=str(path), wing="routes"))
                stale = call("routes", {})
                assert len(stale["stale_documents"]) == 1 and all(r["path"] != str(path) for r in stale["items"]), stale
                call("sync_sources", dict(path=str(source), wing="routes"))
                fresh = call("routes", {})
                assert next(r for r in fresh["items"] if r["route"] == "/write")["request_method"] == "PUT", fresh
                delta = call("diff", dict(before=before, after=fresh["index_rev"], project="routes"))
                added = [json.loads(r) if isinstance(r,str) else r for r in delta["relations_added"]]
                removed = [json.loads(r) if isinstance(r,str) else r for r in delta["relations_removed"]]
                assert len(added) == 1 and added[0]["target"] == "/write" and added[0]["request_method"] == "PUT", delta
                assert len(removed) == 1 and removed[0]["request_method"] == "POST", delta
                foreign = call("diff", dict(before=before, after=fresh["index_rev"], project="other"))
                assert foreign["relations_added"] == [] and foreign["relations_removed"] == [], foreign
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", production_database_touched=False,
                    checks=["public_tool_description", "literal_fetch_targets", "separate_reference_kind", "explicit_request_method", "spread_method_unknown", "embedded_offsets", "inert_scripts_excluded", "identity_unverified", "no_default_method_inferred", "deterministic_replay", "stale_exclusion", "sync_refresh", "immutable_method_diff", "project_diff_isolation"],
                    fetch_references=len(js), markup_references=len(rows)-len(js))
                if report:
                    report.parent.mkdir(parents=True, exist_ok=True)
                    report.write_text(json.dumps(evidence, indent=2) + "\n")
                print(json.dumps(evidence))
            finally:
                process.stdin.close()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.terminate()
                    process.wait(timeout=10)


if __name__ == "__main__":
    main()
