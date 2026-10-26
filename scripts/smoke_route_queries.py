#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_route_queries.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-route-queries-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "alpha/api.py":"@app.get('/same')\ndef get(): pass\n@app.post('/literal_%.*')\ndef post(): pass\n",
            "alpha/client.js":"fetch('/same', {method:'POST'}); fetch('/same');",
            "alpha/view.html":"<form action='/same'></form>",
            "beta/api.py":"@app.delete('/foreign')\ndef foreign(): pass\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="route-queries-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                schema = next(t["inputSchema"] for t in tools["tools"] if t["name"] == "routes")
                assert {"project","route","route_contains","kind","method","request_method","reference_kind","offset","expected_view_token"} <= set(schema["properties"]), schema
                for project in ("alpha","beta"):
                    sync = call("sync_sources",dict(path=str(source/project),wing=project))
                    assert not sync.get("errors"),sync
                legacy = call("routes",{})
                assert legacy["total"] == 6,legacy
                no_arguments=request("tools/call",dict(name="routes"))
                assert not no_arguments.get("isError"),no_arguments
                assert json.loads(next(c["text"] for c in no_arguments["content"] if c["type"] == "text"))["total"] == 6
                endpoint = call("routes",dict(project="alpha",kind="endpoint",route="/same",request_method="post",limit=1))
                assert endpoint["total"] == 1 and endpoint["items"][0]["request_method"] == "POST",endpoint
                assert call("routes",dict(project="alpha",kind="endpoint",method="GET"))["total"] == 0
                assert call("routes",dict(project="alpha",reference_kind="html:form:action"))["total"] == 1
                literal = call("routes",dict(project="alpha",route_contains="_%.*",limit=1))
                assert literal["total"] == 1 and literal["items"][0]["route"] == "/literal_%.*",literal
                first = call("routes",dict(project="alpha",limit=2))
                token=first["view_token"]
                assert first["total"] == 5 and first["pagination"]["has_more"],first
                assert first["counts_by_kind"] == dict(http_route_candidate=2,http_endpoint_reference=3),first
                page2=call("routes",dict(project="alpha",limit=2,offset=2,expected_view_token=token))
                page3=call("routes",dict(project="alpha",limit=2,offset=4,expected_view_token=token))
                combined=first["items"]+page2["items"]+page3["items"]
                assert len({(r["document_id"],r["start_byte"],r["kind"],r["route"]) for r in combined}) == 5
                assert not page3["pagination"]["has_more"]
                def failure(arguments, message):
                    result=request("tools/call",dict(name="routes",arguments=arguments),allow_error=True)
                    assert result.get("isError") or ("code" in result and "message" in result),result
                    assert message in json.dumps(result),result
                failure(dict(project="alpha",offset=2),"expected_view_token")
                failure(dict(project="alpha",kind="endpoint",offset=2,expected_view_token=token),"ROUTES_VIEW_CHANGED")
                beta=source/"beta/api.py"
                beta.write_text("@app.get('/changed-foreign')\ndef changed(): pass\n")
                call("ingest_file",dict(path=str(beta),wing="beta"))
                assert call("routes",dict(project="alpha",limit=2))["view_token"] == token
                client=source/"alpha/client.js"
                client.write_text("fetch('/changed');")
                call("ingest_file",dict(path=str(client),wing="alpha"))
                failure(dict(project="alpha",limit=2,offset=2,expected_view_token=token),"ROUTES_VIEW_CHANGED")
                fresh_view=call("routes",dict(project="alpha",limit=2))
                assert fresh_view["index_rev"] == first["index_rev"] and fresh_view["view_token"] != token,fresh_view
                assert len(fresh_view["stale_documents"]) == 1 and fresh_view["total"] == 3,fresh_view
                assert call("routes",dict(project="alpha' OR 1=1 --"))["total"] == 0
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",production_database_touched=False,
                    checks=["public_schema","legacy_empty_arguments","legacy_missing_arguments","filters_before_limit","literal_substring","separate_methods","kind_and_category","complete_nonoverlapping_pages","token_required","query_mismatch_rejected","project_isolation","stale_view_without_revision_rejected","parameter_binding"],
                    alpha_total=5,page_lengths=[len(first["items"]),len(page2["items"]),len(page3["items"])])
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
