#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_php_routes.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-php-routes-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {"routes.php": "<?php function show() { return 1; } Route::get('/read', 'show'); Route::match(['GET','POST'], '/multi', [Controller::class, 'save']); $app->post('/write', function () { return 1; }); // Route::get('/comment', 'show');\n"}
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="php-routes-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                sync = call("sync_sources", dict(path=str(source), wing="routes"))
                assert not sync.get("errors"), sync
                tools = request("tools/list", {})
                route_tool = next(t for t in tools["tools"] if t["name"] == "routes")
                assert "PHP" in route_tool["description"], route_tool
                result = call("routes", {})
                before = result["index_rev"]
                rows = result["items"]
                assert len(rows) == 4, result
                assert all(r["kind"] == "http_route_candidate" and r["symbol_id"] is None and r["resolution"] == "unresolved" for r in rows), result
                assert {r["method"] for r in rows if r["route"] == "/multi"} == {"GET", "POST"}, result
                assert next(r for r in rows if r["route"] == "/multi")["handler_expression"] == "Controller::save", result
                assert result["detail"]["php_handler_binding_verified"] is False, result
                assert result["detail"]["php_group_prefix_resolved"] is False, result
                assert call("entrypoints", {})["items"] == [], "unbound PHP handler became liveness root"
                assert call("dead_code", {})["total"] == 1
                assert result == call("routes", {}), "unstable routes"
                path = source / "routes.php"
                path.write_text(fixtures["routes.php"].replace("Route::get('/read'", "Route::delete('/read'"))
                call("ingest_file", dict(path=str(path), wing="routes"))
                stale = call("routes", {})
                assert stale["items"] == [] and len(stale["stale_documents"]) == 1, stale
                call("sync_sources", dict(path=str(source), wing="routes"))
                fresh = call("routes", {})
                assert next(r for r in fresh["items"] if r["route"] == "/read")["method"] == "DELETE", fresh
                delta = call("diff", dict(before=before, after=fresh["index_rev"], project="routes"))
                added = [json.loads(r) if isinstance(r,str) else r for r in delta["relations_added"]]
                removed = [json.loads(r) if isinstance(r,str) else r for r in delta["relations_removed"]]
                assert len(added) == 1 and added[0]["kind"] == "unbound_route_registration" and added[0]["method"] == "DELETE", delta
                assert len(removed) == 1 and removed[0]["method"] == "GET", delta
                foreign = call("diff", dict(before=before, after=fresh["index_rev"], project="other"))
                assert foreign["relations_added"] == [] and foreign["relations_removed"] == [], foreign
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", production_database_touched=False, php_code_executed=False,
                    checks=["public_tool_description", "php_route_inventory", "match_method_set", "class_callable_description", "comments_excluded", "handler_binding_unverified", "no_liveness_root", "deterministic_replay", "stale_exclusion", "sync_refresh", "immutable_unbound_route_diff", "project_diff_isolation"],
                    route_candidates=len(rows))
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
