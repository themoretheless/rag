#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_semantic_diff.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-semantic-diff-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "migrations/001.sql": "CREATE TABLE users(name TEXT NOT NULL);\n",
            "models.py": "class Base: pass\nclass Other: pass\nclass Child(Base): pass\ndef work(): pass\nregistered = work\ndef test_old(): pass\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="semantic-diff-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                names = {tool["name"] for tool in tools["tools"]}
                assert {"references", "symbol", "sync_sources"} <= names
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not sync.get("errors"), sync
                before = call("schema", dict(project="fixture"))["index_rev"]
                migration = source / "migrations/001.sql"
                migration.write_text("CREATE TABLE users(name TEXT NOT NULL, age INT NOT NULL, email TEXT);\n")
                code = source / "models.py"
                code.write_text("class Base: pass\nclass Other: pass\nclass Child(Other): pass\ndef work(): pass\nregistered = None\ndef plain(): pass\n# SECRET_BODY_VALUE\n")
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not sync.get("errors"), sync
                after = call("schema", dict(project="fixture"))["index_rev"]
                args = dict(before=before, after=after, project="fixture")
                result = call("diff", args)
                assert result["before_format_version"] == result["after_format_version"] and result["analysis_format_changed"] is False, result
                assert result["schema_diff"]["available"] and result["schema_diff"]["total"] == 1, result
                assert result["schema_diff"]["items"][0]["column_changes_total"] == 2, result
                assert any(e["kind"] == "inheritance" for e in result["relations_added"]), result
                assert any(e["kind"] == "non_call_reference" for e in result["relations_removed"]), result
                assert any(e["kind"] == "entrypoint" for e in result["relations_removed"]), result
                assert "SECRET_BODY_VALUE" not in json.dumps(result), result
                bounded = call("diff", dict(**args, limit=1))
                assert bounded["truncated"] and bounded["schema_diff"]["items"][0]["columns_truncated"], bounded
                code.write_text("# changed after publication without syncing\n")
                migration.write_text("DROP TABLE users;\n")
                assert call("diff", args) == result
                same = call("diff", dict(before=before, after=before, project="fixture"))
                assert same["schema_diff"]["total"] == 0 and same["relations_added_total"] == 0, same
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", production_database_touched=False,
                    checks=["frozen_sql_columns", "inheritance_change", "callback_reference_removed", "test_entrypoint_removed",
                        "bounded_nested_changes", "no_source_bodies", "independent_of_current_files", "same_revision_noop"], before=before, after=after)
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
