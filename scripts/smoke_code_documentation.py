#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_code_documentation.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-code-docs-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {"alpha/a.py":"def documented():\n    \"\"\"SECRET_DOCUMENTATION α\"\"\"\n    return 1\ndef bare(): pass\n", "beta/b.rs":"/// Rust documentation\nfn helper() {}\n", "gamma/c.lua":"local function other() end\n"}
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="code-documentation-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                for project in ("alpha","beta","gamma"):
                    sync=call("sync_sources",dict(path=str(source/project),wing=project))
                    assert not sync.get("errors"),sync
                path=str(source/"alpha/a.py")
                symbols=call("symbol",dict(path=path))
                documented=next(s for s in symbols["items"] if s["name"] == "documented")
                bare=next(s for s in symbols["items"] if s["name"] == "bare")
                assert documented["documentation_examined"] and bare["documentation_examined"],symbols
                assert "documentation" not in bare,symbols
                doc=documented["documentation"]
                assert doc["kind"] == "python_literal_docstring" and "SECRET_DOCUMENTATION α" in doc["text"],doc
                segment=doc["segments"][0]
                assert segment["start_line"] == 2,segment
                raw=fixtures["alpha/a.py"].encode()[segment["start_byte"]:segment["end_byte"]].decode()
                assert raw == doc["text"],doc
                assert call("code",dict(symbol=documented["id"]))["symbol"]["documentation"] == doc
                local=call("index_status",dict(project="alpha"))
                measured=local["quality"]["docblocks"]
                assert measured["examined_symbols"] == 2 and measured["documented_symbols"] == 1 and measured["coverage"] == 0.5,measured
                assert "SECRET_DOCUMENTATION" not in json.dumps(local),"documentation text leaked into status"
                assert "full_docblock_coverage" in local["quality"]["unmeasured_checks"],local
                global_status=call("index_status",{})
                total=global_status["quality"]["docblocks"]
                assert total["examined_symbols"] == 3 and total["documented_symbols"] == 2 and total["uninspected_symbols"] == 1,total
                assert global_status["versions"]["index_format"] >= 27,global_status
                warm=call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                assert warm["code_index"]["unchanged"] == 3,warm
                assert call("symbol",dict(path=path,symbol="documented"))["items"][0]["documentation"] == doc
                Path(path).write_text(fixtures["alpha/a.py"].replace("SECRET_DOCUMENTATION","UPDATED_DOCUMENTATION"))
                call("ingest_file",dict(path=path,wing="alpha"))
                stale=call("index_status",dict(project="alpha"))
                assert stale["quality"]["docblocks"]["coverage"] is None and not stale["quality"]["docblocks"]["available"],stale
                assert call("symbol",dict(path=path))["items"] == []
                call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                updated=call("symbol",dict(path=path,symbol="documented"))["items"][0]["documentation"]
                assert "UPDATED_DOCUMENTATION" in updated["text"] and updated["content_hash"] != doc["content_hash"],updated
                assert call("index_status",dict(project="alpha"))["quality"]["docblocks"]["coverage"] == 0.5
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",production_database_touched=False,source_applications_executed=False,
                    checks=["source_bound_docstrings","byte_span_fidelity","missing_vs_uninspected","symbol_code_consistency","scoped_coverage","unsupported_language_not_missing","status_no_doc_text","full_coverage_unverified","warm_persistence","stale_coverage_unknown","source_sync_doc_refresh"],
                    local_coverage=0.5,global_examined=total["examined_symbols"],global_uninspected=total["uninspected_symbols"])
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
