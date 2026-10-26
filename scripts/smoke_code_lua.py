#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_code_lua.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-code-lua-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {"alpha/main.lua":"-- café\nlocal function leaf(x) return x end\nlocal function worker()\n leaf(1)\n local alias=leaf\n alias(2)\nend\nworker()\n", "beta/foreign.lua":"leaf()\n"}
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="code-lua-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                for project in ("alpha","beta"):
                    sync=call("sync_sources",dict(path=str(source/project),wing=project))
                    assert not sync.get("errors"),sync
                path=str(source/"alpha/main.lua")
                symbols=call("symbol",dict(path=path))
                assert symbols["total"] == 2 and not symbols["parse_errors"],symbols
                leaf=next(s for s in symbols["items"] if s["name"] == "leaf")
                worker=next(s for s in symbols["items"] if s["name"] == "worker")
                edges=call("callees",dict(symbol=worker["id"]))
                direct=next(e for e in edges["items"] if e["reference"]["name"] == "leaf")
                alias=next(e for e in edges["items"] if e["reference"]["name"] == "alias")
                assert direct["resolution"] == "resolved_static" and direct["candidates"] == [leaf["id"]],edges
                assert alias["resolution"] == "unresolved" and not alias["candidates"],edges
                assert any(d["code"] == "LUA_TABLE_IMPORT_ENV_AND_RUNTIME_BINDING_UNVERIFIED" for d in edges["binding_diagnostics"]),edges
                callers=call("callers",dict(symbol=leaf["id"]))
                assert callers["total"] == 1 and all(e["reference"]["path"] == path for e in callers["items"]),callers
                assert call("references",dict(symbol=leaf["id"]))["total"] == 2
                body=call("code",dict(symbol=worker["id"]))
                expected=fixtures["alpha/main.lua"].encode()[worker["start_byte"]:worker["end_byte"]].decode()
                assert body["found"] and body["code"] == expected,body
                structural=call("slice",dict(symbol=worker["id"],direction="forward"))
                assert structural["detail"]["structural_only"] and structural["analysis_incomplete"],structural
                status=call("index_status",{})
                assert any(l["language"] == "lua" and l["available"] and not l["compiler_semantics_verified"] for l in status["capabilities"]["languages"]),status
                assert status["versions"]["index_format"] >= 25,status
                warm=call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                assert warm["code_index"]["unchanged"] == 2,warm
                changed=fixtures["alpha/main.lua"].replace("leaf","updated")
                Path(path).write_text(changed)
                call("ingest_file",dict(path=path,wing="alpha"))
                stale=call("symbol",dict(path=path))
                assert stale["items"] == [] and len(stale["stale_documents"]) == 1,stale
                assert call("code",dict(symbol=worker["id"]))["found"] is False
                call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                revised=call("symbol",dict(path=path))
                updated=next(s for s in revised["items"] if s["name"] == "updated")
                new_worker=next(s for s in revised["items"] if s["name"] == "worker")
                new_calls=call("callees",dict(symbol=new_worker["id"]))
                assert next(e for e in new_calls["items"] if e["reference"]["name"] == "updated")["candidates"] == [updated["id"]],new_calls
                assert "updated(1)" in call("code",dict(symbol=new_worker["id"]))["code"]
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",production_database_touched=False,lua_code_executed=False,
                    checks=["lua_capability","named_local_definitions","lexical_call_binding","value_reference","alias_unresolved","foreign_global_not_linked","unicode_byte_body","partial_structural_slice","cache_reuse","stale_exclusion","source_sync_rebind"],
                    named_symbols=symbols["total"],direct_call_candidates=len(direct["candidates"]))
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
