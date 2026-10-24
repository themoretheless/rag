#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_code_styles_policy.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-css-policy-") as temporary:
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
            try:
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="styles-policy-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                names = {tool["name"] for tool in tools["tools"]}
                assert {"class_usage", "policy_trace", "sync_sources"} <= names
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not sync.get("errors"), sync
                classes = call("class_usage", dict(name="live"))
                assert classes["items"][0]["status"] == "defined_and_used", classes
                assert call("class_usage", dict(name="fake"))["found"] is False
                missing = call("class_usage", dict(mode="missing"))
                assert missing["items"][0]["name"] == "missing", missing
                trace = call("policy_trace", dict(symbol="policy"))
                assert trace["default_behavior"] == "deny" and len(trace["callers"]) == 1, trace
                warm = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert warm["code_index"]["unchanged"] == 3, warm
                page.write_text('<html><body><p class="missing">Visible.</p></body></html>')
                policy.write_text("<?php function policy($role) { if ($role === 'blocked') return false; return true; }")
                updated = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not updated.get("errors"), updated
                classes_after = call("class_usage", dict(name="live"))
                assert classes_after["items"][0]["status"] == "unused_in_indexed_scope", classes_after
                trace_after = call("policy_trace", dict(symbol="policy"))
                assert trace_after["default_behavior"] == "allow", trace_after
                assert trace_after["index_rev"] > trace["index_rev"]
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", protocol=init["protocolVersion"],
                    embedding="mock", external_database=False, tools_verified=["class_usage", "policy_trace", "sync_sources"],
                    checks=["tool_discovery", "CSS_comment_false_positive", "original_HTML_class_evidence", "missing_class",
                            "static_PHP_callers", "default_deny", "warm_index_reuse", "markup_only_refresh", "default_allow_after_refresh"],
                    initial_index_rev=trace["index_rev"], updated_index_rev=trace_after["index_rev"])
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
