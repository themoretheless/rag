#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_method_relations.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-method-relations-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "alpha/base.py": "class Base:\n    def work(self): return 'PRIVATE_BODY'\n",
            "alpha/child.py": "from base import Base as Parent\nclass Child(Parent):\n    def work(self): return 2\nclass Other(Parent):\n    def work(self): return 3\n",
            "beta/other.py": "class Base:\n    def work(self): pass\nclass Foreign(Base):\n    def work(self): pass\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="method-relations-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                for project in ("alpha","beta"):
                    sync=call("sync_sources",dict(path=str(source/project),wing=project))
                    assert not sync.get("errors"),sync
                base=str(source/"alpha/base.py")
                child=source/"alpha/child.py"
                args=dict(symbol="Base::work",path=base,limit=1)
                refs=call("references",args)
                relations=refs["method_relations"]
                assert relations["total"] == 2 and relations["truncated"],refs
                assert len(relations["items"]) == 1,refs
                edge=relations["items"][0]
                assert edge["path"] == str(child) and not edge["semantic_override_proven"],edge
                assert edge["document_id"] and edge["revision"] and edge["content_hash"],edge
                inherited=call("inheritance",dict(symbol="Child",path=str(child)))
                assert inherited["total"] == 1 and inherited["method_relations"]["total"] == 1,inherited
                assert "PRIVATE_BODY" not in json.dumps(refs),refs
                before=refs["index_rev"]
                assert call("callees",dict(symbol="Child::work",path=str(child)))["total"] == 0
                lexical=call("references",dict(**args,reference_mode="lexical"))
                assert lexical["method_relations"]["total"] == 0,lexical
                noop=call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                assert noop["code_index"]["unchanged"] == 3,noop
                child.write_text("from base import Base as Parent\nclass Child(Parent): pass\nclass Other(Parent): pass\n")
                call("ingest_file",dict(path=str(child),wing="alpha"))
                stale=call("references",args)
                assert stale["method_relations"]["total"] == 0 and len(stale["stale_documents"]) == 1,stale
                call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                refreshed=call("references",args)
                assert refreshed["method_relations"]["total"] == 0,refreshed
                diffargs=dict(before=before,after=refreshed["index_rev"],project="alpha")
                diff=call("diff",diffargs)
                assert diff["method_relations_available"],diff
                removed=[e for e in diff["relations_removed"] if e["kind"] == "method_override_candidate"]
                assert len(removed) == 2 and not any(e["semantic_override_proven"] for e in removed),diff
                assert "PRIVATE_BODY" not in json.dumps(diff),diff
                child.write_text("class Replaced: pass\n")
                assert call("diff",diffargs) == diff
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",production_database_touched=False,source_applications_executed=False,
                    checks=["imported_base_method","independent_relation_budget","source_provenance","class_and_method_navigation","no_call_fabrication","project_isolation","lexical_mode_separation","warm_reuse","stale_exclusion","sync_refresh","frozen_relation_diff","no_source_bodies"],
                    semantic_override_proven=False,removed_method_candidates=len(removed))
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
