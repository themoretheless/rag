#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_dead_code.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-dead-code-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "alpha/bound.py": "def work(): pass\nregistered = work\n",
            "alpha/unused.py": "def work(): pass\n",
            "alpha/unknown.py": "def maybe(): helper()\ndef helper(): pass\ndef unused_local(): pass\nexternal_receiver.maybe\n",
            "alpha/protected.py": "class Base:\n    def run(self): pass\nclass Child(Base):\n    def run(self): nested()\ndef nested(): pass\ndef unused(): pass\nregistered = Base.run\n",
            "beta/other.py": "unknown.work\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="dead-code-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                for project in ("alpha","beta"):
                    sync=call("sync_sources",dict(path=str(source/project),wing=project))
                    assert not sync.get("errors"),sync
                def dead(name,limit=500):
                    return call("dead_code",dict(path=str(source/name),limit=limit))
                def names(result):
                    return {r["symbol"]["qualified_name"] for r in result["items"]}
                bound=dead("alpha/bound.py")
                assert names(bound) == set(),bound
                unused=dead("alpha/unused.py")
                assert names(unused) == {"work"},unused
                unknown=dead("alpha/unknown.py")
                assert names(unknown) == {"unused_local"} and unknown["analysis_incomplete"],unknown
                assert unknown["detail"]["unresolved_name_roots"] >= 1,unknown
                protected=dead("alpha/protected.py")
                assert names(protected) == {"unused"} and protected["detail"]["override_candidate_edges"] >= 1,protected
                assert all(not r["deletion_safe"] for r in protected["items"]),protected
                assert not protected["detail"]["runtime_dispatch_proven"],protected
                assert call("callers",dict(symbol="Base::run",path=str(source/"alpha/protected.py")))["total"] == 0
                noop=call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                assert noop["code_index"]["unchanged"] == 5,noop
                path=source/"alpha/protected.py"
                path.write_text(fixtures["alpha/protected.py"].replace("registered = Base.run", "registered = None"))
                call("ingest_file",dict(path=str(path),wing="alpha"))
                stale=dead("alpha/protected.py")
                assert stale["items"] == [] and len(stale["stale_documents"]) == 1,stale
                call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                refreshed=dead("alpha/protected.py")
                assert {"Child::run","nested"} <= names(refreshed),refreshed
                capped=dead("alpha/protected.py",1)
                assert capped["truncated"] and len(capped["items"]) == 1,capped
                assert dead("alpha/protected.py",1) == capped
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",production_database_touched=False,source_applications_executed=False,
                    checks=["bound_identity_not_global_name","cross_project_unknown_names_isolated","unresolved_roots_protect_callees","override_candidates_protect_callees","no_call_fabrication","deletion_never_proven","warm_reuse","stale_exclusion","sync_refresh","bounded_stable_candidates"],runtime_dispatch_proven=False,versions=call("index_status",{})["versions"])
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
