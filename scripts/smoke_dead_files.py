#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_dead_files.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-dead-files-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "alpha/used.py": "def used(): pass\n",
            "alpha/unused.py": "def unused(): pass\n",
            "alpha/api.py": "def work(): pass\n",
            "alpha/main.py": "from api import work\ndef main(): work()\n",
            "alpha/tool.sh": "#!/bin/sh\necho ready\n",
            "alpha/view.html": "<p>Index view</p><script>const PRIVATE_BODY='used.py'; const ambiguous='same.py'; const obsolete='unused.py.bak';</script>",
            "alpha/a/same.py": "def first(): pass\n",
            "alpha/b/same.py": "def second(): pass\n",
            "beta/other.html": "<p>Foreign view</p><script>const foreign='unused.py';</script>",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="dead-files-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                for project in ("alpha","beta"):
                    sync=call("sync_sources",dict(path=str(source/project),wing=project))
                    assert not sync.get("errors"),sync
                def files(**kw):
                    return call("dead_code",dict(dead_code_view="files",dead_code_project="alpha",**kw))
                def paths(result,key):
                    return {row["file"]["path"] for row in result[key]}
                result=files()
                assert str(source/"alpha/used.py") in paths(result,"refuted"),result
                assert str(source/"alpha/unused.py") in paths(result,"items"),result
                assert {str(source/"alpha/a/same.py"),str(source/"alpha/b/same.py")} <= paths(result,"items"),result
                assert str(source/"alpha/api.py") not in paths(result,"items"),result
                assert str(source/"alpha/tool.sh") not in paths(result,"items"),result
                assert result["protected_files"] >= 3 and not result["deletion_safe"],result
                assert "PRIVATE_BODY" not in json.dumps(result),result
                hit=next(r for r in result["refuted"] if r["file"]["path"] == str(source/"alpha/used.py"))
                assert hit["evidence"]["path"] == str(source/"alpha/view.html") and hit["evidence"]["content_hash"],hit
                assert not hit["evidence"]["binding_proven"],hit
                allprojects=call("dead_code",dict(dead_code_view="files"))
                assert str(source/"alpha/unused.py") not in paths(allprojects,"refuted"),allprojects
                budget=files(dead_code_max_bytes=1)
                assert budget["items"] == [] and budget["not_checked_total"] > 0 and budget["literal_bytes_loaded"] == 0,budget
                assert not budget["budgets"]["graph_source_validation_included"],budget
                capped=files(dead_code_max_candidates=1)
                assert capped["not_checked_total"] > 0,capped
                assert files(dead_code_max_candidates=1) == capped
                noop=call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                assert noop["code_index"]["unchanged"] == 9,noop
                api=source/"alpha/api.py"
                api.write_text("def changed(): pass\n")
                call("ingest_file",dict(path=str(api),wing="alpha"))
                stale=files(path=str(source/"alpha/unused.py"))
                assert stale["items"] == [] and stale["not_checked_total"] == 1,stale
                call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                refreshed=files(path=str(source/"alpha/unused.py"))
                assert str(source/"alpha/unused.py") in paths(refreshed,"items"),refreshed
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",production_database_touched=False,source_applications_executed=False,
                    checks=["literal_refutation_from_preserved_html","suffix_boundary","ambiguous_basename_not_bound","project_isolation","known_incoming_protection","shebang_protection","no_source_body_disclosure","literal_byte_budget_unknown","candidate_budget_unknown","stable_results","warm_reuse","stale_project_indeterminate","sync_refresh"],deletion_safe=False,versions=call("index_status",{})["versions"])
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
