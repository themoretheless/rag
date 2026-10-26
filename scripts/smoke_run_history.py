#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_run_history.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-run-history-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {"alpha/a.py":"def a(): return 'PRIVATE_BODY'\n", "beta/b.py":"def b(): pass\n"}
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="run-history-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                empty=call("catalog",dict(view="runs"))
                assert empty["total"] == 0 and empty["items"] == [],empty
                for project in ("alpha","beta"):
                    sync=call("sync_sources",dict(path=str(source/project),wing=project))
                    assert not sync.get("errors"),sync
                call("index_code",{})
                call("index_code",{})
                first=call("catalog",dict(view="runs",limit=1))
                assert first["has_more"] and first["scope"] == "corpus_global",first
                top=first["items"][0]
                assert top["metrics_available"] and not top["revision_created"],first
                assert not top["metrics"]["commit_included"] and "PRIVATE_BODY" not in json.dumps(first),first
                call("index_code",{})
                path=source/"alpha/a.py"
                path.write_text("def changed(): pass\n")
                call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                rest=call("catalog",dict(view="runs",cursor=first["next_cursor"],limit=500))
                assert rest["total"] == first["total"] and not rest["has_more"],rest
                assert len(rest["items"]) == first["total"]-1,rest
                assert all(r["seq"] < top["seq"] for r in rest["items"]),rest
                allruns=call("catalog",dict(view="runs",limit=500))
                assert allruns["total"] == first["total"]+2,allruns
                assert allruns["items"][0]["revision_created"],allruns
                scoped=call("catalog",dict(view="runs",index_revision=top["index_rev"],limit=500))
                assert scoped["items"] and all(r["index_rev"] == top["index_rev"] for r in scoped["items"]),scoped
                assert call("catalog",dict(view="runs",limit=500)) == allruns
                rejected=request("tools/call",dict(name="catalog",arguments=dict(view="runs",project="alpha")),allow_error=True)
                assert rejected.get("isError") or "code" in rejected,rejected
                rejected=request("tools/call",dict(name="catalog",arguments=dict(view="files",cursor=first["next_cursor"])),allow_error=True)
                assert rejected.get("isError") or "code" in rejected,rejected
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",production_database_touched=False,source_applications_executed=False,
                    checks=["empty_history","publication_and_noop_runs","validated_metrics","pinned_pages_across_appends","limit_change","new_revision_visible_to_fresh_query","revision_filter","read_only_history","project_filter_rejected","cursor_view_guard","no_source_bodies"],scope="corpus_global",failed_runs_available=False,versions=call("index_status",{})["versions"])
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
