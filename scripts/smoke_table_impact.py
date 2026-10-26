#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_table_impact.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-table-impact-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "alpha/api.py": "def load():\n    q = 'SELECT * FROM users'\ndef save():\n    q = 'INSERT INTO users(id) VALUES(1)'\ndef screen(): load()\ndef submit(): save()\ndef dashboard(): screen()\n",
            "alpha/raw.sql": "SELECT * FROM users;",
            "alpha/ambiguous.sql": "SELECT * FROM public.logs; SELECT * FROM admin.logs;",
            "beta/other.sql": "SELECT * FROM users;",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="table-impact-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                for project in ("alpha","beta"):
                    sync=call("sync_sources",dict(path=str(source/project),wing=project))
                    assert not sync.get("errors"),sync
                args=dict(impact_table="users",impact_project="alpha")
                symbols=call("symbol",dict(limit=500))["items"]
                names={s["id"]:s["qualified_name"] for s in symbols}
                result=call("impact",args)
                assert result["direct_total"] == 3 and result["ownerless_direct_uses"] == 1,result
                assert result["direct_owner_symbols"] == 2,result
                assert {names[r["symbol_id"]] for r in result["items"]} == {"screen","submit","dashboard"},result
                assert all(u["evidence"]["project"] == "alpha" for u in result["direct_readers_writers"]),result
                read=call("impact",dict(**args,impact_table_mode="read"))
                assert read["direct_total"] == 2 and {names[r["symbol_id"]] for r in read["items"]} == {"screen","dashboard"},read
                write=call("impact",dict(**args,impact_table_mode="write"))
                assert write["direct_total"] == 1 and [names[r["symbol_id"]] for r in write["items"]] == ["submit"],write
                capped=call("impact",dict(**args,limit=1))
                assert capped["direct_truncated"] and capped["direct_owner_symbols"] == 2 and capped["truncated"],capped
                ambiguous=call("impact",dict(impact_table="users"))
                assert ambiguous["needs_disambiguation"] and ambiguous["table_candidates_total"] == 2,ambiguous
                schemas=call("impact",dict(impact_table="logs",impact_project="alpha"))
                assert schemas["needs_disambiguation"] and schemas["table_candidates_total"] == 2,schemas
                qualified=call("impact",dict(impact_table="public.logs",impact_project="alpha"))
                assert qualified["direct_total"] == 1 and qualified["items"] == [],qualified
                invalid=call("impact",dict(impact_table="users; DROP TABLE users"))
                assert "error" in invalid,invalid
                noop=call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                assert noop["code_index"]["unchanged"] == 4,noop
                path=source/"alpha/api.py"
                path.write_text(fixtures["alpha/api.py"].replace("FROM users","FROM accounts"))
                call("ingest_file",dict(path=str(path),wing="alpha"))
                stale=call("impact",args)
                assert stale["direct_total"] == 1 and stale["items"] == [] and len(stale["stale_documents"]) == 1,stale
                call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                refreshed=call("impact",args)
                assert refreshed["direct_total"] == 2 and len(refreshed["items"]) == 1,refreshed
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",production_database_touched=False,source_sql_executed=False,
                    checks=["direct_readers_writers","ownerless_sql_preserved","indirect_code_chain","read_write_filters","independent_direct_budget","untruncated_owner_roots","project_disambiguation","schema_disambiguation","qualified_target","identifier_validation","warm_reuse","stale_exclusion","sync_refresh"],
                    runtime_impact_proven=False)
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
