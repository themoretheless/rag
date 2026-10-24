#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_migration_checks.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-migration-checks-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "migrations/001.sql": "CREATE TABLE users(age INT CONSTRAINT positive CHECK(AGE > 0), name TEXT, CONSTRAINT small CHECK(age < 100)); CREATE TABLE guarded(value TEXT CHECK(dangerous_function(value))); CREATE TABLE pair(a INT, b INT, CONSTRAINT ordered CHECK(a < b));\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="migration-checks-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                names = {tool["name"] for tool in tools["tools"]}
                assert {"references", "symbol", "sync_sources"} <= names
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not sync.get("errors"), sync
                def check(table="users", **kw):
                    return call("write_check", dict(project="fixture", table=table, **kw))
                good = check(values={"age": 7})
                assert good["status"] == "valid" and len(good["constraint_checks"]) == 2, good
                bad = check(values={"age": -1})
                assert bad["status"] == "invalid" and any(i["code"] == "CHECK_CONSTRAINT_VIOLATION" for i in bad["issues"]), bad
                null = check(values={"age": None})
                assert null["status"] == "valid", null
                unknown = check(values={}, columns=["age"])
                assert unknown["status"] == "indeterminate" and unknown["unverified"], unknown
                columns = check(columns=["age"])
                assert columns["status"] == "valid" and not columns["literal_row_requested"], columns
                private = check("guarded", values={"value": "PRIVATE_LITERAL_NEVER_ECHO"})
                assert private["status"] == "indeterminate" and "PRIVATE_LITERAL_NEVER_ECHO" not in json.dumps(private), private
                update = check("pair", operation="update", values={"a": 1})
                assert update["status"] == "indeterminate" and update["schema_complete"], update
                deletion = check(operation="delete")
                assert deletion["status"] == "valid" and not deletion["constraint_checks"], deletion
                (source / "migrations/002.sql").write_text("ALTER TABLE users ADD CONSTRAINT lower_bound CHECK(age > 5) NOT VALID;\n")
                call("sync_sources", dict(path=str(source), wing="fixture"))
                added = check(values={"age": 2})
                assert added["status"] == "invalid", added
                assert any(i["scope"]["not_valid"] for i in added["issues"] if i["code"] == "CHECK_CONSTRAINT_VIOLATION"), added
                (source / "migrations/003.sql").write_text("ALTER TABLE users DROP CONSTRAINT lower_bound;\n")
                call("sync_sources", dict(path=str(source), wing="fixture"))
                assert check(values={"age": 2})["status"] == "valid"
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", production_database_touched=False, migration_sql_executed=False,
                    checks=["named_column_and_table_checks", "identifier_case_folding", "rejected_row", "sql_null_logic", "unknown_assignment",
                        "column_plan_scope", "unknown_function_no_execution", "no_literal_echo", "partial_update_context", "delete_skips_checks", "not_valid_applies_to_new_row", "constraint_drop_refresh"])
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
