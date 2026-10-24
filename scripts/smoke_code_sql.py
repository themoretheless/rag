#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_code_references.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-sql-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "migrations/001.sql": "CREATE TABLE users(name TEXT NOT NULL, age INT);\n",
            "migrations/002.sql": "ALTER TABLE users ADD COLUMN email TEXT;\n",
            "service.py": "def fetch():\n    return query('SELECT name FROM users')\ndef save():\n    return query(\"INSERT INTO users(name) VALUES ('Ada')\")\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="sql-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                names = {tool["name"] for tool in tools["tools"]}
                assert {"references", "symbol", "sync_sources"} <= names
                assert {"schema", "table_usage", "write_check"} <= names
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not sync.get("errors"), sync
                schema = call("schema", dict(project="fixture", table="users"))
                assert schema["total"] == 1, schema
                assert set(schema["items"][0]["columns"]) == {"name", "age", "email"}, schema
                for mode in ("read", "write"):
                    usage = call("table_usage", dict(project="fixture", table="users", mode=mode))
                    assert usage["total"] >= 1, usage
                    assert all(u["mode"] == mode for u in usage["items"]), usage
                    assert any(u.get("symbol") for u in usage["items"]), usage
                def check(**kw):
                    return call("write_check", dict(project="fixture", table="users", **kw))
                assert check(columns=["name"])["status"] == "valid"
                assert check(columns=["nmae"])["status"] == "invalid"
                assert check(values={"name": None})["status"] == "invalid"
                assert check(columns=[])["status"] == "invalid"
                assert check(operation="delete")["status"] == "valid"
                assert check(operation="update", columns=[])["status"] == "invalid"
                warm = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert warm["code_index"]["unchanged"] == len(fixtures), warm
                migration = source / "migrations/002.sql"
                migration.write_text("ALTER TABLE users ADD COLUMN email TEXT NOT NULL;\n")
                call("ingest_file", dict(path=str(migration), wing="fixture"))
                assert check(columns=["name"])["status"] == "indeterminate"
                refreshed = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not refreshed.get("errors"), refreshed
                assert check(columns=["name"])["status"] == "invalid"
                assert check(columns=["name", "email"])["status"] == "valid"
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", embedding="mock",
                    production_database_touched=False, sql_executed=False,
                    checks=["migration_create_and_alter", "read_and_write_locations_with_owners",
                            "valid_insert", "unknown_column", "null_rejected", "required_column",
                            "delete", "empty_update", "warm_cache", "stale_schema_indeterminate",
                            "source_sync_refresh"], index_rev=call("schema", {})["index_rev"])
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
