#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_file_impact.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-file-impact-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "alpha/api.py": "def work(): return 'PRIVATE_BODY'\n",
            "alpha/registry.py": "from api import work\nregistered = work\ndef run(): pass\n",
            "alpha/client.py": "from registry import run\ndef outer(): run()\n",
            "beta/other.py": "def work(): pass\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="file-impact-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                for project in ("alpha","beta"):
                    sync=call("sync_sources",dict(path=str(source/project),wing=project))
                    assert not sync.get("errors"),sync
                target=source/"alpha/api.py"
                args=dict(impact_file=str(target),impact_project="alpha")
                result=call("impact",args)
                assert result["result_kind"] == "files" and result["target_available"],result
                assert [r["path"] for r in result["items"]] == [str(source/"alpha/registry.py"),str(source/"alpha/client.py")],result
                assert result["items"][1]["depth"] == 2 and result["ownerless_bound_occurrences_included"],result
                assert result["items"][0]["via"]["reference"]["caller"] is None,result
                assert "PRIVATE_BODY" not in json.dumps(result),result
                calls=call("impact",dict(**args,impact_relations="calls"))
                assert calls["items"] == [],calls
                shallow=call("impact",dict(**args,depth=1))
                assert shallow["depth_limited"] and len(shallow["items"]) == 1,shallow
                capped=call("impact",dict(**args,limit=1))
                assert capped["truncated"] and not capped["traversal_complete"],capped
                doc=call("impact",dict(impact_document_id=result["target"]["document_id"]))
                assert doc["items"] == result["items"],doc
                invalid=call("impact",dict(**args,impact_table="users"))
                assert "error" in invalid,invalid
                noop=call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                assert noop["code_index"]["unchanged"] == 4,noop
                registry=source/"alpha/registry.py"
                registry.write_text("registered = None\ndef run(): pass\n")
                call("ingest_file",dict(path=str(registry),wing="alpha"))
                stale=call("impact",args)
                assert stale["items"] == [] and len(stale["stale_documents"]) == 1,stale
                call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                refreshed=call("impact",args)
                assert refreshed["items"] == [] and refreshed["index_rev"] != result["index_rev"],refreshed
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",production_database_touched=False,source_applications_executed=False,
                    checks=["ownerless_module_dependency","transitive_file_callers","call_mode_separation","depth_limit","item_limit","document_id_target","mixed_target_rejected","warm_reuse","stale_bridge_excluded","sync_refresh","no_source_bodies"],runtime_impact_proven=False)
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
