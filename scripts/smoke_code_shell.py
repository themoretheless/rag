#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_code_shell.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-shell-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "lib.sh": "work(){ :; }\n",
            "main.bash": f"source '{source / 'lib.sh'}'\nwork\nexport -f work\nquery 'SELECT * FROM users'\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="shell-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                names = {tool["name"] for tool in tools["tools"]}
                assert {"references", "symbol", "sync_sources"} <= names
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not sync.get("errors"), sync
                symbols = call("symbol", dict(path=str(source / "lib.sh"), symbol="work"))["items"]
                assert len(symbols) == 1, symbols
                refs = call("references", dict(symbol=symbols[0]["id"]))
                assert any(r["is_call"] and r["resolution"] == "resolved_static" for r in refs["items"]), refs
                assert any(not r["is_call"] for r in refs["items"]), refs
                usage = call("table_usage", dict(table="users", mode="read"))
                assert usage["total"] == 1, usage
                warm = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert warm["code_index"]["unchanged"] == 2, warm
                lib = source / "lib.sh"
                lib.write_text("changed(){ :; }\n")
                call("ingest_file", dict(path=str(lib), wing="fixture"))
                stale = call("references", dict(symbol=symbols[0]["id"]))
                assert stale["stale_documents"] and not stale["items"], stale
                call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not call("symbol", dict(path=str(lib), symbol="work"))["items"]
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", scripts_executed=False,
                    production_database_touched=False, checks=["absolute_source_binding", "export_function_reference",
                        "constant_sql_usage", "warm_cache", "stale_dependency_omitted", "source_sync_refresh"])
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
