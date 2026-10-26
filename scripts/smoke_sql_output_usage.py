#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_sql_output_usage.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-sql-output-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "alpha/ops.sql": "MERGE INTO inventory t USING staging s ON t.id=s.id WHEN MATCHED THEN UPDATE SET qty=s.qty OUTPUT inserted.id INTO audit_rows(id);\nSELECT id INTO report_rows FROM source_rows;\nSELECT id INTO @capture FROM source_rows;\nUPDATE inventory SET qty=1 OUTPUT inserted.id;\n",
            "alpha/export.py": 'def make_report():\n    q = "SELECT id INTO python_report FROM source_rows"\n',
            "alpha/migrations/001.sql": "CREATE TABLE known(id INT); SELECT id INTO unknown_shape FROM known;",
            "beta/other.sql": "SELECT id INTO foreign_report FROM foreign_source;",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="sql-output-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                for project in ("alpha","beta"):
                    sync=call("sync_sources",dict(path=str(source/project),wing=project))
                    assert not sync.get("errors"),sync
                writes=call("table_usage",dict(project="alpha",mode="write"))
                pairs={(r["table"],r["operation"]) for r in writes["items"]}
                assert {("inventory","merge"),("inventory","update"),("audit_rows","merge_output_into"),("report_rows","select_into"),("python_report","select_into"),("unknown_shape","select_into")} <= pairs,writes
                assert not any("capture" in t or t == "foreign_report" for t,_ in pairs),writes
                reads=call("table_usage",dict(project="alpha",mode="read"))
                assert {"staging","source_rows","known"} <= {r["table"] for r in reads["items"]},reads
                assert not any(r["table"] in {"audit_rows","report_rows","python_report","unknown_shape"} for r in reads["items"]),reads
                owned=next(r for r in writes["items"] if r["table"] == "python_report")
                assert owned["symbol"] and owned["evidence"]["line"] == 2,owned
                assert any(d["code"] == "SQL_INTO_TARGET_UNRESOLVED" for d in writes["diagnostics"]),writes
                assert any(d["code"] == "SELECT_INTO_SCHEMA_UNVERIFIED" and d["schema_relevant"] for d in writes["diagnostics"]),writes
                unknown=call("schema",dict(project="alpha",table="unknown_shape"))
                assert unknown["total"] == 0,unknown
                check=call("write_check",dict(project="alpha",table="known",operation="insert",columns=["id"]))
                assert check["status"] == "indeterminate",check
                noop=call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                assert noop["code_index"]["unchanged"] == 4,noop
                path=source/"alpha/ops.sql"
                path.write_text(fixtures["alpha/ops.sql"].replace("audit_rows", "audit_new"))
                call("ingest_file",dict(path=str(path),wing="alpha"))
                stale=call("table_usage",dict(project="alpha",mode="write"))
                assert len(stale["stale_documents"]) == 1 and not any(r["table"] == "audit_rows" for r in stale["items"]),stale
                call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                refreshed=call("table_usage",dict(project="alpha",mode="write"))
                assert any(r["table"] == "audit_new" for r in refreshed["items"]),refreshed
                assert not any(r["table"] == "audit_rows" for r in refreshed["items"]),refreshed
                status=call("index_status",{})
                assert status["versions"]["index_format"] >= 28,status
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",production_database_touched=False,source_sql_executed=False,
                    checks=["merge_secondary_write","select_into_write","no_fake_sink_read","literal_owner_line","variable_target_excluded","migration_schema_uncertain","projection_not_invented","write_check_indeterminate","project_isolation","warm_reuse","stale_exclusion","source_sync_refresh"],
                    write_operations=sorted({r["operation"] for r in writes["items"]}))
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
