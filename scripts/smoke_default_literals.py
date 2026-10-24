#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_default_literals.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-default-literals-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "migrations/001.sql": "CREATE TABLE users(age INT NOT NULL DEFAULT 7 CHECK(age > 5), enabled BOOLEAN DEFAULT TRUE CHECK(enabled), label TEXT DEFAULT 'PRIVATE_DEFAULT_LITERAL' CHECK(length(label)>0)); CREATE TABLE null_default(a INT NOT NULL DEFAULT NULL); CREATE TABLE dynamic_default(a INT DEFAULT next_value() CHECK(a>0)); CREATE TABLE wrong_type(a INT DEFAULT '7' CHECK(a>0));\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="default-literals-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                names = {tool["name"] for tool in tools["tools"]}
                assert {"references", "symbol", "sync_sources"} <= names
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not sync.get("errors"), sync
                def check(table="users", **kw):
                    return call("write_check", dict(project="fixture", table=table, **kw))
                good = check(values={})
                assert good["status"] == "valid" and set(good["inferred_default_columns"]) == {"age", "enabled", "label"}, good
                assert "PRIVATE_DEFAULT_LITERAL" not in json.dumps(good), good
                assigned = check(values={}, columns=["age"])
                assert assigned["status"] == "indeterminate" and "age" not in assigned["inferred_default_columns"], assigned
                update = check(operation="update", values={"age": 8})
                assert update["status"] == "indeterminate" and update["inferred_default_columns"] == [], update
                null = check("null_default", values={})
                assert null["status"] == "invalid" and any(i["source"] == "literal_default" for i in null["issues"] if i["code"] == "NULL_NOT_ALLOWED"), null
                for table in ("dynamic_default", "wrong_type"):
                    unknown = check(table, values={})
                    assert unknown["status"] == "indeterminate" and unknown["unverified"], unknown
                override = check("dynamic_default", values={"a": 1})
                assert override["status"] == "valid", override
                (source / "migrations/002.sql").write_text("ALTER TABLE users ALTER COLUMN age SET DEFAULT 3;\n")
                call("sync_sources", dict(path=str(source), wing="fixture"))
                invalid = check(values={})
                assert invalid["status"] == "invalid" and any(i["code"] == "CHECK_CONSTRAINT_VIOLATION" for i in invalid["issues"]), invalid
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", production_database_touched=False, migration_sql_executed=False,
                    checks=["typed_literal_defaults", "default_check_context", "no_default_echo", "unknown_assignment_not_overridden", "update_does_not_use_insert_defaults", "null_default_rejected", "function_default_unknown", "coercion_unknown", "explicit_value_overrides_default", "alter_default_refresh"])
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
