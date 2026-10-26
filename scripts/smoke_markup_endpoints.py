#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_markup_endpoints.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-markup-endpoints-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {"view.html": "<form action='/save?a=1&amp;b=2'><input formaction='/other'></form><img src='/image.png'><script src='/app.js'></script><a href='https://external/x'>ignored</a>"}
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="markup-endpoints-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                sync = call("sync_sources", dict(path=str(source), wing="routes"))
                assert not sync.get("errors"), sync
                result = call("routes", {})
                before = result["index_rev"]
                rows = result["items"]
                assert len(rows) == 4, result
                assert all(r["kind"] == "http_endpoint_reference" and r["symbol_id"] is None and r["method"] is None and r["resolution"] == "unbound_reference" for r in rows), result
                assert all(r["project"] == "routes" and r["document_id"] and r["revision"] >= 1 and r["content_hash"] for r in rows), result
                assert any(r["route"] == "/save?a=1&b=2" and r["reference_kind"] == "html:form:action" for r in rows), result
                assert result == call("routes", {}), "unstable endpoints"
                assert call("entrypoints", {})["items"] == [], "endpoint became handler root"
                assert call("index_status", {})["state"] == "observed_checks_passed"
                path = source / "view.html"
                path.write_text("<form action='/replacement'></form>")
                call("ingest_file", dict(path=str(path), wing="routes"))
                stale = call("routes", {})
                assert stale["items"] == [] and stale["stale_documents"], stale
                call("sync_sources", dict(path=str(source), wing="routes"))
                fresh = call("routes", {})
                assert [r["route"] for r in fresh["items"]] == ["/replacement"], fresh
                assert not fresh["stale_documents"], fresh
                delta = call("diff", dict(before=before, after=fresh["index_rev"], project="routes"))
                added = [json.loads(r) if isinstance(r, str) else r for r in delta["relations_added"]]
                removed = [json.loads(r) if isinstance(r, str) else r for r in delta["relations_removed"]]
                assert any(r["kind"] == "endpoint_reference" and r["target"] == "/replacement" for r in added), delta
                assert len([r for r in removed if r["kind"] == "endpoint_reference"]) == 4, delta
                foreign = call("diff", dict(before=before, after=fresh["index_rev"], project="other"))
                assert foreign["relations_added"] == [] and foreign["relations_removed"] == [], foreign
                prior_revision = fresh["index_rev"]
                prior_hash = fresh["items"][0]["content_hash"]
                path.write_text("\n<form action='/replacement'></form>")
                call("sync_sources", dict(path=str(source), wing="routes"))
                moved = call("routes", {})
                assert moved["items"][0]["line"] == 2 and moved["items"][0]["content_hash"] != prior_hash, moved
                movement = call("diff", dict(before=prior_revision, after=moved["index_rev"], project="routes"))
                assert movement["relations_added"] == [] and movement["relations_removed"] == [], movement
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", production_database_touched=False,
                    checks=["literal_markup_targets", "entity_decoding", "separate_reference_kind", "no_liveness_root", "source_provenance", "deterministic_replay", "stale_exclusion", "sync_refresh", "immutable_diff", "project_diff_isolation", "status_not_empty", "line_move_is_not_relation_change"],
                    initial_references=len(rows), refreshed_references=len(fresh["items"]))
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
