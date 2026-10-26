#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_impact_files.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-impact-files-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "alpha/core.py": "def core(): return 'PRIVATE_BODY'\n",
            "alpha/callback.py": "from core import core\ndef callback():\n    return core\n",
            "alpha/wrapper.py": "from callback import callback\ndef wrapper():\n    return callback()\n",
            "alpha/models.py": "class Base:\n    def work(self): pass\nclass Child(Base):\n    def work(self): pass\n",
            "beta/other.py": "def core(): pass\ndef foreign(): return core\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="impact-files-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                for project in ("alpha","beta"):
                    sync=call("sync_sources",dict(path=str(source/project),wing=project))
                    assert not sync.get("errors"),sync
                path=source/"alpha/core.py"
                args=dict(symbol="core",path=str(path))
                symbols=call("symbol",dict(limit=500))["items"]
                names={s["id"]:s["qualified_name"] for s in symbols}
                default=call("impact",args)
                assert default["items"] == [] and default["impact_relations"] == "calls",default
                value=call("impact",dict(**args,impact_relations="references"))
                assert [names[r["symbol_id"]] for r in value["items"]] == ["callback","wrapper"],value
                assert [r["via_kind"] for r in value["items"]] == ["bound_reference","call"],value
                assert value["items"][1]["depth"] == 2 and not value["runtime_impact_proven"],value
                assert "PRIVATE_BODY" not in json.dumps(value),value
                files=value["affected_files"]
                assert len(files) == 2 and {f["path"] for f in files} == {str(source/"alpha/callback.py"),str(source/"alpha/wrapper.py")},value
                assert all(f["project"] == "alpha" and f["content_hash"] and f["document_id"] for f in files),value
                assert sum(f["symbol_count"] for f in files) == len(value["items"]) == 2,value
                assert value["file_summary_scope"] == "returned_symbols" and not value["program_impact_complete"],value
                assert call("callers",args)["total"] == 0
                shallow=call("impact",dict(**args,impact_relations="references",depth=1))
                assert shallow["depth_limited"] and len(shallow["items"]) == 1,shallow
                capped=call("impact",dict(**args,impact_relations="references",depth=10,limit=1))
                assert capped["truncated"] and not capped["traversal_complete"],capped
                assert sum(f["symbol_count"] for f in capped["affected_files"]) == 1,capped
                method=call("impact",dict(symbol="Base::work",path=str(source/"alpha/models.py"),impact_relations="all"))
                assert [names[r["symbol_id"]] for r in method["items"]] == ["Child::work"],method
                assert method["items"][0]["via_kind"] == "override_candidate",method
                assert not method["items"][0]["via"]["semantic_override_proven"],method
                invalid=call("impact",dict(**args,impact_relations="invalid"))
                assert "error" in invalid,invalid
                noop=call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                assert noop["code_index"]["unchanged"] == 5,noop
                changed=source/"alpha/callback.py"
                changed.write_text(fixtures["alpha/callback.py"].replace("return core", "return None"))
                call("ingest_file",dict(path=str(changed),wing="alpha"))
                stale=call("impact",dict(**args,impact_relations="references"))
                assert stale["items"] == [] and len(stale["stale_documents"]) == 1,stale
                call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                refreshed=call("impact",dict(**args,impact_relations="references"))
                assert refreshed["items"] == [] and refreshed["index_rev"] != value["index_rev"],refreshed
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",production_database_touched=False,source_applications_executed=False,
                    checks=["multi_file_summary","bounded_summary_counts","document_provenance","legacy_call_default","value_to_caller_chain","relation_witnesses","no_call_fabrication","override_candidate_traversal","depth_limit","item_limit","invalid_mode","project_isolation","warm_reuse","stale_exclusion","sync_refresh","no_source_bodies"],
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
