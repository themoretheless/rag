#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_repo_map.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-repo-map-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "alpha/lib/api.py": "def work(): pass\ndef extra(): pass\n",
            "alpha/app/main.py": "from lib.api import work\ndef main(): work()\n",
            "alpha/lib-copy/other.py": "def excluded(): pass\n",
            "beta/foreign.py": "def foreign(): pass\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="repo-map-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                schema = next(t["inputSchema"] for t in tools["tools"] if t["name"] == "repo_map")
                assert {"project", "path_prefix", "budget_tokens", "budget_chars"} <= set(schema["properties"]), schema
                for project in ("alpha", "beta"):
                    sync = call("sync_sources", dict(path=str(source / project), wing=project))
                    assert not sync.get("errors"), sync
                first = call("repo_map", dict(project="alpha"))
                assert first["total_files"] == 3 and first["ranking_nodes"] == 3, first
                assert first["ranking_edges"] >= 1, first
                assert first["map"].splitlines()[0].endswith('api.py"'), first
                assert "foreign" not in first["map"], first
                assert first == call("repo_map", dict(project="alpha")), "unstable map"
                scoped = call("repo_map", dict(project="alpha", path_prefix=str(source / "alpha/lib")))
                assert scoped["total_files"] == 1 and "excluded" not in scoped["map"], scoped
                no_prefix = call("repo_map", dict(project="alpha", path_prefix=str(source / "alpha/li")))
                assert no_prefix["map"] == "" and no_prefix["ranking_nodes"] == 0, no_prefix
                for budget in (1, 5, 32, 64, 256):
                    bounded = call("repo_map", dict(project="alpha", budget_tokens=budget, budget_chars=256))
                    assert bounded["estimated_tokens"] <= budget and bounded["used_chars"] == len(bounded["map"]) <= 256, bounded
                    if budget == 1: assert bounded["map"] == "", bounded
                beta = source / "beta/foreign.py"
                beta.write_text("def foreign_changed(): pass\n")
                call("ingest_file", dict(path=str(beta), wing="beta"))
                new = source / "beta/new.py"
                new.write_text("def new_unindexed(): pass\n")
                call("ingest_file", dict(path=str(new), wing="beta"))
                assert first == call("repo_map", dict(project="alpha")), "other-project mutations leaked"
                global_map = call("repo_map", {})
                assert len(global_map["stale_documents"]) == 1 and len(global_map["unindexed_documents"]) == 1, global_map
                assert not call("repo_map", dict(project="missing"))["map"]
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", production_database_touched=False,
                    checks=["public_schema", "central_file_first", "deterministic_replay", "project_isolation",
                            "component_prefix", "strict_payload_character_budget", "estimated_token_budget",
                            "empty_tiny_budget", "scoped_stale_and_unindexed_coverage"],
                    ranking_nodes=first["ranking_nodes"], ranking_edges=first["ranking_edges"])
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
