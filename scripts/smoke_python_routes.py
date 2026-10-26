#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_python_routes.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-python-routes-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {"routes.py": "@app.route('/multi', methods=['GET', 'POST'])\ndef handler(): pass\n@app.api_route('/api', methods=['PATCH'])\ndef handler(): pass\n@app.route('/dynamic', methods=verbs)\ndef dynamic(): pass\n"}
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="python-routes-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                sync = call("sync_sources", dict(path=str(source), wing="routes"))
                assert not sync.get("errors"), sync
                result = call("routes", {})
                assert result["detail"]["framework_identity_verified"] is False, result
                assert result["detail"]["runtime_registration_verified"] is False, result
                rows = result["items"]
                assert len(rows) == 4, result
                assert all(r["resolution"] == "resolved_static" for r in rows), result
                assert {r["method"] for r in rows if r["route"] == "/multi"} == {"GET", "POST"}, result
                assert [r["method"] for r in rows if r["route"] == "/dynamic"] == ["ANY"], result
                assert next(r["symbol_id"] for r in rows if r["route"] == "/multi") != next(r["symbol_id"] for r in rows if r["route"] == "/api"), result
                assert result == call("routes", {}), "unstable routes"
                path = source / "routes.py"
                path.write_text("@app.api_route('/changed', methods=['DELETE', 'PUT'])\ndef replacement(): pass\n")
                call("ingest_file", dict(path=str(path), wing="routes"))
                stale = call("routes", {})
                assert stale["items"] == [] and stale["stale_documents"], stale
                call("sync_sources", dict(path=str(source), wing="routes"))
                fresh = call("routes", {})
                assert {r["method"] for r in fresh["items"]} == {"DELETE", "PUT"}, fresh
                assert all(r["route"] == "/changed" for r in fresh["items"]), fresh
                assert not fresh["stale_documents"], fresh
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", production_database_touched=False,
                    checks=["literal_methods", "dynamic_any", "same_name_span_binding", "no_keyword_fake_handlers", "deterministic_replay", "stale_exclusion", "sync_refresh"],
                    initial_routes=len(rows), refreshed_routes=len(fresh["items"]))
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
