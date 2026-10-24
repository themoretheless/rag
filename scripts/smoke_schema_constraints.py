#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_schema_constraints.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-constraints-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "migrations/001.sql": "CREATE TABLE parent(id INT PRIMARY KEY); CREATE TABLE users(name TEXT NOT NULL, age INT CONSTRAINT positive CHECK(age > 0), parent_id INT REFERENCES parent(id), CONSTRAINT uq UNIQUE(name));\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="constraints-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                names = {tool["name"] for tool in tools["tools"]}
                assert {"references", "symbol", "sync_sources"} <= names
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not sync.get("errors"), sync
                args = dict(project="fixture", table="users")
                before = call("schema", args)
                table = before["items"][0]
                assert table["columns"]["age"]["constraints"][0]["name"] == "positive", table
                assert table["columns"]["parent_id"]["constraints"][0]["kind"] == "foreign_key", table
                assert table["constraints"][0]["kind"] == "unique", table
                (source / "migrations/002.sql").write_text("ALTER TABLE users ADD CONSTRAINT cap CHECK(age < 100) NOT VALID; ALTER TABLE users DROP CONSTRAINT positive;\n")
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not sync.get("errors"), sync
                after = call("schema", args)
                table = after["items"][0]
                assert table["complete"] and not table["columns"]["age"].get("constraints"), table
                assert any(c["name"] == "cap" and c["not_valid"] for c in table["constraints"]), table
                diff = call("diff", dict(before=before["index_rev"], after=after["index_rev"], project="fixture", limit=1))
                assert diff["schema_diff"]["constraint_changes_available"] and diff["schema_diff"]["total"] == 1, diff
                assert diff["schema_diff"]["items"][0]["column_changes_total"] == 1, diff
                assert diff["schema_diff"]["items"][0]["constraints_added_total"] == 1, diff
                assert diff["schema_diff"]["items"][0]["constraints_truncated"] and diff["truncated"], diff
                (source / "migrations/003.sql").write_text("ALTER TABLE users RENAME COLUMN age TO years;\n")
                call("sync_sources", dict(path=str(source), wing="fixture"))
                uncertain = call("schema", args)
                assert not uncertain["items"][0]["complete"], uncertain
                assert any(d["code"] == "SCHEMA_DEPENDENCY_REWRITE_UNVERIFIED" for d in uncertain["diagnostics"]), uncertain
                check = call("write_check", dict(**args, columns=["name"]))
                assert check["status"] == "indeterminate", check
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", production_database_touched=False, sql_executed=False,
                    checks=["column_check", "foreign_key", "table_unique", "add_constraint_not_valid", "drop_named_column_constraint",
                        "frozen_constraint_diff", "nested_limits", "dependency_rewrite_diagnostic", "uncertain_write_plan"])
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
