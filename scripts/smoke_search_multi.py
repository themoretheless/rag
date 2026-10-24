#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_search_multi.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-multi-search-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "Files.txt": "alpha alpha alpha alpha\n",
            "Files2.txt": "beta\n",
            "Other.txt": "alpha\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="multi-search-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                names = {tool["name"] for tool in tools["tools"]}
                assert {"references", "symbol", "sync_sources"} <= names
                assert "search_multi" in names
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not sync.get("errors"), sync
                args = dict(queries=["alpha", "beta", "absent"], project="fixture", limit=1)
                result = call("search_multi", args)
                assert result["matching_documents_total"] == 0, result
                assert result["union_matching_documents_total"] == 3, result
                assert result["per_query"][0]["matching_documents_total"] == 2, result
                assert result["per_query"][0]["truncated"], result
                assert result["per_query"][0]["items"][0]["more_occurrences"], result
                assert result["per_query"][1]["items"][0]["document"]["path"].endswith("Files2.txt"), result
                assert result["per_query"][2]["items"] == [], result
                assert result == call("search_multi", args)
                args.update(limit=20, match_mode="any")
                union = call("search_multi", args)
                assert union["matching_documents_total"] == 3, union
                assert any(len(f["members"]) == 2 for f in union["families"]), union
                assert not union["family_roles_verified"], union
                assert all(not f["duplicate_content_proven"] for f in union["families"]), union
                scoped = call("search_multi", dict(queries=["alpha"], path_prefix=str(source / "missing")))
                assert scoped["documents_scanned"] == 0, scoped
                for name, text in {
                    "api.py": "def work():\n    return 'role-needle'\n",
                    "api2.py": "def unused():\n    return 'role-needle'\n",
                    "aaa.py": "def unrelated():\n    return 'role-needle'\n",
                    "use.py": "from api import work\nregistered = work\n",
                }.items():
                    (source / name).write_text(text)
                call("sync_sources", dict(path=str(source), wing="fixture"))
                role_args = dict(queries=["role-needle"], project="fixture")
                active = call("search_multi", role_args)
                member = active["families"][0]["members"][0]
                assert member["document"]["path"].endswith("api.py"), active
                assert member["role"] == "referenced_candidate", active
                assert any(e["kind"] == "incoming_reference_candidate" and e["path"].endswith("use.py") for e in member["role_evidence"]), active
                assert active == call("search_multi", role_args)
                (source / "use.py").write_text("registered = None\n")
                call("ingest_file", dict(path=str(source / "use.py"), wing="fixture"))
                stale = call("search_multi", role_args)
                assert stale["family_role_analysis"]["stale_documents"], stale
                assert all(m["usage_score"] < 3 for f in stale["families"] for m in f["members"]), stale
                call("sync_sources", dict(path=str(source), wing="fixture"))
                refreshed = call("search_multi", role_args)
                assert not refreshed["family_role_analysis"]["stale_documents"], refreshed
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", production_database_touched=False,
                    checks=["independent_query_windows", "all_and_any", "missing_query", "per_query_truncation",
                        "occurrence_truncation", "filename_families", "explicit_unverified_roles", "stable_response", "path_scope", "bound_callback_role", "activity_ordering", "stale_role_omitted", "role_refresh"])
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
