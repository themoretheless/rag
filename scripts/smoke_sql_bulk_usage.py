#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_sql_bulk_usage.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-sql-bulk-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "alpha/ops.sql":"MERGE INTO inventory t USING staging s ON t.id=s.id WHEN MATCHED THEN UPDATE SET qty=s.qty;\nTRUNCATE TABLE ONLY archive;\nCOPY inbound FROM '/nonexistent-input-for-static-analysis.csv';\nCOPY (SELECT * FROM audit) TO STDOUT;\n",
            "alpha/export.py":"def export():\n    q = \"COPY outbound TO STDOUT\"\n",
            "alpha/dynamic.ts":"function purge(table) { const q = `TRUNCATE TABLE ${table}`; }",
            "beta/other.sql":"COPY foreign_rows TO STDOUT;",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="sql-bulk-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                for project in ("alpha","beta"):
                    sync=call("sync_sources",dict(path=str(source/project),wing=project))
                    assert not sync.get("errors"),sync
                writes=call("table_usage",dict(project="alpha",mode="write"))
                assert {(r["table"],r["operation"]) for r in writes["items"]} == {("inventory","merge"),("archive","truncate"),("inbound","copy_from")},writes
                reads=call("table_usage",dict(project="alpha",mode="read"))
                expected={("inventory","merge"),("staging","merge"),("audit","copy_to"),("outbound","copy_to")}
                assert {(r["table"],r["operation"]) for r in reads["items"]} == expected,reads
                assert not any(r["table"] == "foreign_rows" for r in reads["items"]),reads
                exported=next(r for r in reads["items"] if r["table"] == "outbound")
                assert exported["symbol"] and exported["evidence"]["line"] == 2,exported
                assert any(d["code"] == "DYNAMIC_OR_UNSUPPORTED_SQL_LITERAL" for d in reads["diagnostics"]),reads
                assert not any(d["code"] == "UNSUPPORTED_WRITE_OPERATION" for d in reads["diagnostics"]),reads
                noop=call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                assert noop["code_index"]["unchanged"] == 4,noop
                before=reads["index_rev"]
                path=source/"alpha/ops.sql"
                path.write_text(fixtures["alpha/ops.sql"].replace("COPY inbound FROM", "COPY new_inbound FROM"))
                call("ingest_file",dict(path=str(path),wing="alpha"))
                stale=call("table_usage",dict(project="alpha",mode="write"))
                assert stale["items"] == [] and len(stale["stale_documents"]) == 1,stale
                call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                refreshed=call("table_usage",dict(project="alpha",mode="write"))
                assert refreshed["index_rev"] != before and any(r["table"] == "new_inbound" for r in refreshed["items"]),refreshed
                assert not any(r["table"] == "inbound" for r in refreshed["items"]),refreshed
                status=call("index_status",{})
                assert status["versions"]["index_format"] >= 26,status
                rejected=request("tools/call",dict(name="write_check",arguments=dict(table="inventory",operation="merge")),allow_error=True)
                assert rejected.get("isError") or ("code" in rejected and "message" in rejected),rejected
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",production_database_touched=False,source_sql_executed=False,
                    checks=["merge_read_write","truncate_only_explicit_write","copy_direction","query_copy_reads","literal_owner_line","dynamic_target_excluded","project_isolation","warm_reuse","stale_exclusion","source_sync_refresh","unsupported_write_check_not_validated"],
                    write_operations=sorted({r["operation"] for r in writes["items"]}),read_operations=sorted({r["operation"] for r in reads["items"]}))
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
