#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_live_hunks.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-live-hunks-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "alpha/main.py": "def target():\n    old_secret = 1\n    bridge = 0\n    other_secret = 2\n    return bridge\n",
            "alpha/large.py": "# old\n" * 1100,
            "beta/other.py": "def other(): pass\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="live-hunks-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                for project in ("alpha","beta"):
                    sync=call("sync_sources",dict(path=str(source/project),wing=project))
                    assert not sync.get("errors"),sync
                path=source/"alpha/main.py"
                original=fixtures["alpha/main.py"]
                path.write_text(original.replace("old_secret", "new_secret").replace("other_secret", "updated_secret"))
                args=dict(path=str(path),project="alpha",start_line=1,end_line=1,max_chars=20)
                live=call("live_read",args)
                assert live["status"] == "modified",live
                hunks=live["diff"]["edit_hunks"]
                assert hunks["exact"] and hunks["total"] == 2 and not hunks["truncated"],live
                assert "secret" not in json.dumps(live),live
                assert live["content"] == "def target():" and live["historical_index_immutable"],live
                again=call("live_read",args)
                assert again["drift_event_id"] == live["drift_event_id"],again
                path.write_text(original.rstrip("\n"))
                newline=call("live_read",args)
                assert newline["diff"]["edit_hunks"]["total"] == 1,newline
                path.write_bytes(original.replace("\n","\r\n").encode())
                crlf=call("live_read",args)
                assert crlf["status"] == "modified" and crlf["diff"]["edit_hunks"]["total"] >= 1,crlf
                path.write_text(original)
                restored=call("live_read",args)
                assert restored["status"] == "unchanged" and restored["diff"]["edit_hunks"]["total"] == 0,restored
                large=source/"alpha/large.py"
                large.write_text("# new\n" * 1100)
                coarse=call("live_read",dict(path=str(large),project="alpha",start_line=1,end_line=1,max_chars=4))
                assert coarse["diff"]["edit_hunks"]["exact"] is False,coarse
                assert coarse["diff"]["edit_hunks"]["fallback_reason"] == "lcs_cell_budget_exceeded",coarse
                assert coarse["content"] == "# ne" and coarse["truncated"],coarse
                assert live["index_rev"] == coarse["index_rev"],coarse
                large.write_text(fixtures["alpha/large.py"])
                path.write_text(original.replace("old_secret","new_secret"))
                call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                synced=call("live_read",args)
                assert synced["status"] == "unchanged" and synced["diff"]["edit_hunks"]["total"] == 0,synced
                frozen=call("live_read",dict(**args,index_revision=live["index_rev"]))
                assert frozen["status"] == "modified" and frozen["diff"]["edit_hunks"]["total"] == 1,frozen
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",production_database_touched=False,source_applications_executed=False,
                    checks=["separate_hunks","no_out_of_range_source_text","idempotent_drift_event","final_newline","crlf","restored_source","bounded_fallback","body_budget","read_preserves_index_revision","sync_refresh","frozen_revision_comparison"],
                    hunk_contract_version=hunks["contract_version"])
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
