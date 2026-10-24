#!/usr/bin/env python3
"""Exercise durable task attribution and notes through the real stdio MCP binary.

Usage: python3 scripts/smoke_analysis_tasks.py [path/to/rag-mcp] [report.json]
Never uses the user's database, watcher or external embedding/LLM service.
"""
import json
import os
from pathlib import Path
import queue
import subprocess
import socket
import urllib.request
import urllib.error
import sys
import tempfile
import threading
import time


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp").resolve()
    report = Path(sys.argv[2]) if len(sys.argv) > 2 else None
    with tempfile.TemporaryDirectory(prefix="rag-task-notes-") as temporary:
        root = Path(temporary)
        source = root / "sources"
        source.mkdir()
        (source / "main.css").write_text("/* .fake {} */\n.live:hover {color:red}\n.unused {}\n")
        page = source / "page.html"
        page.write_text('<html><body><p class="live missing">Visible.</p></body></html>')
        policy = source / "policy.php"
        policy.write_text("<?php function policy($role) { if ($role === 'admin') return true; return false; } function handler() { return policy('employee'); }")
        env = {key: value for key, value in os.environ.items() if not key.startswith("RAG_")}
        env.update(RAG_DB_PATH=str(root / "test.duckdb"), RAG_INGEST_ROOTS=str(source),
                   RAG_EMBEDDING_PROVIDER="mock", RAG_EMBEDDING_DIMS="16",
                   RAG_LLM_ENABLED="false", RAG_TOOLS="full")
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        write_token, read_token = "w" * 48, "r" * 48
        env.update(RAG_HTTP_BIND=f"127.0.0.1:{port}", RAG_HTTP_WRITE_TOKEN=write_token, RAG_HTTP_READ_TOKEN=read_token)
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
            def request(method, params):
                nonlocal request_id
                request_id += 1
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", id=request_id, method=method, params=params)) + "\n")
                process.stdin.flush()
                deadline = time.monotonic() + 60
                while True:
                    response = incoming.get(timeout=max(0.01, deadline - time.monotonic()))
                    assert response is not None, "MCP process exited"
                    if response.get("id") == request_id:
                        assert "error" not in response, response
                        return response["result"]
            def call(name, arguments):
                result = request("tools/call", dict(name=name, arguments=arguments))
                assert not result.get("isError"), result
                return json.loads(next(c["text"] for c in result["content"] if c["type"] == "text"))
            def http_rpc(name, arguments, token=write_token, scope=None):
                params = dict(name=name, arguments=arguments)
                if scope is not None:
                    params["_meta"] = {"rag/task_session": scope}
                payload = dict(jsonrpc="2.0", id=101, method="tools/call", params=params)
                req = urllib.request.Request(f"http://127.0.0.1:{port}/mcp", data=json.dumps(payload).encode(), headers={
                    "Content-Type":"application/json", "Accept":"application/json, text/event-stream", "Authorization":f"Bearer {token}"})
                with urllib.request.urlopen(req, timeout=15) as response:
                    if "application/json" in response.headers.get("Content-Type", ""):
                        return json.load(response)
                    for line in response:
                        if line.startswith(b"data:"):
                            data = json.loads(line[5:].strip())
                            if data.get("id") == 101:
                                return data
                raise AssertionError("HTTP MCP returned no result")
            def http_call(name, arguments, token=write_token, scope=None):
                result = http_rpc(name, arguments, token, scope)
                assert "error" not in result, result
                value = result["result"]
                assert not value.get("isError"), value
                return json.loads(next(c["text"] for c in value["content"] if c["type"] == "text"))
            try:
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="analysis-task-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                names = {tool["name"] for tool in tools["tools"]}
                assert {"task_set", "task_stats", "note", "notes", "symbol"} <= names
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not sync.get("errors"), sync
                label = call("task_set", dict(name="audit"))
                assert label["generation"] == 1
                session = label["session_id"]
                first = call("note", dict(text="100%_ exact", tags=["a,b", "c"]))
                second = call("note", dict(text="100%_ exact", tags=["a,b", "c"]))
                assert first["source"] == "note" and first["verified_by_index"] is False
                assert first["note_id"] != second["note_id"] and first["task"] == "audit"
                assert first["tags"] == ["a,b", "c"]
                page = call("notes", dict(query="%_", limit=1))
                assert page["total_matching"] == 2 and page["truncated"]
                older = call("notes", dict(query="%_", before_seq=page["next_before_seq"]))
                assert older["items"][0]["note_id"] == first["note_id"]
                call("symbol", dict(symbol="policy"))
                call("symbol", dict(symbol="not_a_real_symbol"))
                try:
                    call("class_usage", dict(mode="invalid"))
                except AssertionError:
                    pass
                else:
                    raise AssertionError("invalid mode should fail")
                stats = call("task_stats", dict(format="markdown"))
                assert stats["calls"] == 3 and stats["succeeded"] == 2 and stats["failed_calls"] == 1, stats
                assert stats["notes_total"] == 2 and stats["unfinished"] == 0, stats
                assert stats["returned_evidence_records"] == 1, stats
                assert "not_a_real_symbol" not in json.dumps(stats["recent"])
                assert all("arguments" not in r and "error" not in r for r in stats["recent"])
                call("task_set", dict(name="other", session_id="other-session"))
                result = request("tools/call", dict(name="symbol", arguments=dict(symbol="policy"), _meta={"rag/task_session":"other-session"}))
                assert not result.get("isError"), result
                other = call("task_stats", dict(session_id="other-session"))
                assert other["task"] == "other" and other["calls"] == 1, other
                assert call("task_stats", {})["calls"] == 3
                zero = call("task_stats", dict(task="never used"))
                assert zero["calls"] == 0 and zero["measured_nonempty_rate"] is None
                try:
                    call("task_set", dict(name="overwrite", if_generation=0))
                except AssertionError:
                    pass
                else:
                    raise AssertionError("stale generation should fail")
                assert call("task_stats", {})["task"] == "audit"
                call("task_set", dict(clear=True, if_generation=1))
                assert call("task_stats", {})["calls"] == 0
                assert call("task_stats", dict(task="audit"))["calls"] == 3
                missing_scope = http_rpc("task_set", dict(name="must not be shared"))
                assert "error" in missing_scope, missing_scope
                http_call("task_set", dict(name="http audit", session_id="http-one"))
                # No scope must not inherit the named HTTP client's label.
                http_call("symbol", dict(symbol="policy"))
                assert http_call("task_stats", dict(session_id="http-one"))["calls"] == 0
                http_call("symbol", dict(symbol="policy"), scope="http-one")
                http_stats = http_call("task_stats", dict(session_id="http-one"), token=read_token)
                assert http_stats["calls"] == 1 and http_stats["task"] == "http audit", http_stats
                forbidden = http_rpc("note", dict(text="must be denied", session_id="http-one"), token=read_token)
                assert "error" in forbidden, forbidden
                forbidden = http_rpc("task_set", dict(name="must be denied", session_id="http-one"), token=read_token)
                assert "error" in forbidden, forbidden
                http_call("note", dict(text="allowed writer note", session_id="http-one"))
                http_notes = http_call("notes", dict(session_id="http-one"), token=read_token)
                assert http_notes["total_matching"] == 1 and http_notes["items"][0]["verified_by_index"] is False, http_notes
                evidence = dict(status="passed", transport="stdio and loopback HTTP MCP JSON-RPC", protocol=init["protocolVersion"],
                    embedding="mock", external_database=False, tools_verified=["task_set", "task_stats", "note", "notes", "symbol"],
                    checks=["tool_discovery", "session_task_label", "unverified_notes", "JSON_tags", "stable_note_cursor", "literal_filter",
                            "real_index_success_and_failure_attribution", "zero_unfinished_calls", "independent_explicit_session_metadata",
                            "empty_task_has_zero_calls", "generation_conflict", "clear_does_not_erase_history", "zero_usage_is_explicit", "HTTP_requires_explicit_task_session", "HTTP_unscoped_call_does_not_inherit_task",
                            "HTTP_metadata_attribution", "HTTP_read_role_cannot_write_note_or_task", "HTTP_read_role_can_read_notes_and_stats"],
                    observed_audit_calls=stats["calls"], observed_other_calls=other["calls"], observed_notes=stats["notes_total"])
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
