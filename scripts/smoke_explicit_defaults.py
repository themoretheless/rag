#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_explicit_defaults.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-explicit-default-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "migrations/001.sql": "CREATE TABLE t(a INT DEFAULT 7 CHECK(a>5)); CREATE TABLE nullable_default(a INT CHECK(a IS NULL)); CREATE TABLE required_default(a INT NOT NULL); CREATE TABLE generated_default(a INT, b INT GENERATED ALWAYS AS (a+1) STORED);\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="explicit-default-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                names = {tool["name"] for tool in tools["tools"]}
                assert {"references", "symbol", "sync_sources"} <= names
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not sync.get("errors"), sync
                def check(table="t", **kw):
                    return call("write_check", dict(project="fixture", table=table, **kw))
                for operation in ("insert", "update"):
                    result = check(operation=operation, default_columns=["a"])
                    assert result["status"] == "valid" and result["explicit_default_columns"] == ["a"], result
                    assert result["inferred_default_columns"] == ["a"], result
                counted = check(default_columns=["a"], values=1)
                assert counted["status"] == "valid" and counted["value_count"] == 1, counted
                mismatch = check(default_columns=["a"], values=2)
                assert mismatch["status"] == "invalid", mismatch
                nullable = check("nullable_default", default_columns=["a"])
                assert nullable["status"] == "valid", nullable
                required = check("required_default", default_columns=["a"])
                assert required["status"] == "indeterminate" and required["inferred_default_columns"] == [], required
                generated = check("generated_default", default_columns=["b"], values={"a": 1})
                assert generated["status"] == "indeterminate", generated
                duplicate = check(default_columns=["a", "A"])
                assert duplicate["status"] == "invalid", duplicate
                deletion = check(operation="delete", default_columns=["a"])
                assert deletion["status"] == "invalid", deletion
                conflict = request("tools/call", dict(name="write_check", arguments=dict(table="t", project="fixture", default_columns=["a"], values={"a": 1})), allow_error=True)
                assert conflict.get("isError") or "code" in conflict, conflict
                (source / "migrations/002.sql").write_text("ALTER TABLE t ALTER COLUMN a SET DEFAULT 3;\n")
                call("sync_sources", dict(path=str(source), wing="fixture"))
                assert check(operation="update", default_columns=["a"])["status"] == "invalid"
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", production_database_touched=False, migration_sql_executed=False,
                    checks=["explicit_default_insert", "explicit_default_update", "values_count", "count_mismatch", "nullable_default_null", "unknown_implicit_default", "unknown_generated_default", "duplicate_defaults", "delete_default_rejected", "literal_default_conflict", "alter_default_refresh"])
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
