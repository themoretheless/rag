#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_code_go.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-go-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "go.mod": "module fixture.local/app\ngo 1.23\n",
            "api/api.go": "package renamed\nfunc Work() { helper() }\nfunc helper() {}\ntype Service struct{}\nfunc(s *Service) Run() { Work() }\ntype Reader interface { Read() }\n",
            "api/other.go": "package renamed\nfunc Extra(){Work()}\n",
            "main.go": "package main\nimport svc \"fixture.local/app/api\"\nfunc main(){svc.Work(); testOnly(); platform()}\nfunc use(s *svc.Service,r svc.Reader){s.Run();r.Read()}\n",
            "default.go": "package main\nimport \"fixture.local/app/api\"\nfunc useDefault(){renamed.Extra()}\n",
            "shadow.go": "package main\nimport svc \"fixture.local/app/api\"\nfunc shadow(svc interface{}) {svc.Work()}\ntype Name struct{}\nfunc Work(){}\nfunc local(){type Name int;const Work=1;_=Work;var value Name;_=value}\n",
            "main_test.go": "package main\nfunc testOnly() {}\nfunc TestEntry(){testOnly()}\n",
            "impl_linux.go": "package main\nfunc platform() {}\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="go-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                names = {tool["name"] for tool in tools["tools"]}
                assert {"references", "symbol", "callers", "callees", "sync_sources"} <= names
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not sync.get("errors"), sync
                assert sync["code_index"]["indexed"] == len(fixtures), sync
                assert not sync["code_index"]["parse_errors"], sync
                paths = {name: str(source / name) for name in fixtures}
                def calls(name, path="main.go"):
                    return call("callees",dict(symbol=name,path=paths[path]))
                def expression(result, name):
                    return next(r for r in result["items"] if r["reference"]["expression"]==name)
                initial = calls("main")
                work = expression(initial,"svc.Work")
                assert work["resolution"] == "resolved_static" and len(work["candidates"]) == 1, initial
                assert expression(initial,"testOnly")["resolution"] == "unresolved", initial
                assert expression(initial,"platform")["resolution"] == "unresolved", initial
                assert any(d["code"]=="GO_BUILD_SELECTION_UNVERIFIED" for d in initial["binding_diagnostics"])
                assert expression(calls("shadow","shadow.go"),"svc.Work")["resolution"] == "unresolved"
                local_types=call("symbol",dict(path=paths["shadow.go"],symbol="Name"))["items"]
                outer=next(r for r in local_types if r["qualified_name"]=="Name")
                inner=next(r for r in local_types if r["qualified_name"]=="local::Name")
                assert call("references",dict(symbol=outer["id"]))["total"]==0
                assert call("references",dict(symbol=inner["id"]))["total"]==1
                assert call("references",dict(path=paths["shadow.go"],symbol="Work"))["total"]==0
                assert expression(calls("useDefault","default.go"),"renamed.Extra")["resolution"] == "resolved_static"
                use = calls("use")
                assert expression(use,"s.Run")["resolution"] == "resolved_static", use
                assert expression(use,"r.Read")["resolution_basis"] == "go_interface_contract", use
                work_id = work["candidates"][0]
                callers = call("callers",dict(symbol=work_id))
                assert any(r["reference"]["path"]==paths["main.go"] for r in callers["items"]), callers
                refs = call("references",dict(symbol=work_id))
                assert any(r["expression"]=="svc.Work" and r["path"]==paths["main.go"] for r in refs["items"]), refs
                run = call("symbol",dict(path=paths["api/api.go"],symbol="Service::Run"))
                assert run["total"] == 1, run
                body = call("code",dict(symbol=run["items"][0]["id"]))
                assert "Run() { Work() }" in body["code"], body
                entries = call("entrypoints",{})
                assert any(r["kind"]=="conventional_main" and r["path"]==paths["main.go"] for r in entries["items"]), entries
                assert any(r["kind"]=="go_test_candidate" for r in entries["items"]), entries
                warm = call("sync_sources",dict(path=str(source),wing="fixture"))
                assert warm["code_index"]["unchanged"] == len(fixtures), warm
                manifest = source / "go.mod"
                manifest.write_text("module changed.local/app\n")
                call("ingest_file",dict(path=str(manifest),wing="fixture"))
                stale = calls("main")
                assert stale["stale_documents"] and not expression(stale,"svc.Work")["candidates"], stale
                changed = call("sync_sources",dict(path=str(source),wing="fixture"))
                assert not changed.get("errors"), changed
                changed_calls = calls("main")
                assert not expression(changed_calls,"svc.Work")["candidates"], changed_calls
                assert changed_calls["index_rev"] > initial["index_rev"]
                manifest.write_text(fixtures["go.mod"])
                call("sync_sources",dict(path=str(source),wing="fixture"))
                restored = calls("main")
                assert expression(restored,"svc.Work")["candidates"] == [work_id], restored
                nested = source / "api/go.mod"
                nested.write_text("module foreign.local/api\n")
                call("ingest_file",dict(path=str(nested),wing="fixture"))
                blocked = calls("main")
                assert blocked["unindexed_documents"] and not expression(blocked,"svc.Work")["candidates"], blocked
                assert any(d["code"]=="GO_MANIFEST_BOUNDARY_UNVERIFIED" for d in blocked["binding_diagnostics"]), blocked
                call("sync_sources",dict(path=str(source),wing="fixture"))
                nested.write_text("module changed.local/api\n")
                call("ingest_file",dict(path=str(nested),wing="fixture"))
                blocked = calls("main")
                assert blocked["stale_documents"] and not expression(blocked,"svc.Work")["candidates"], blocked
                nested.unlink()
                call("sync_sources",dict(path=str(source),wing="fixture",remove_deleted=True))
                assert expression(calls("main"),"svc.Work")["candidates"] == [work_id]
                evidence = dict(status="passed",transport="stdio MCP JSON-RPC",embedding="mock",
                    external_database=False,production_database_used=False,
                    tools_verified=["symbol","callers","callees","references","code","entrypoints","sync_sources","ingest_file"],
                    checks=["Go_grammar","methods_and_types","same_package_calls","explicit_module_alias",
                            "default_import_uses_package_declaration","typed_pointer_receiver","interface_contract_only",
                            "parameter_shadows_import","local_type_and_constant_shadowing","production_excludes_test_targets","build_selection_unverified",
                            "bound_references","explicit_code_body","entrypoints","warm_reuse","stale_manifest_omitted",
                            "manifest_change_rebinds_unchanged_Go_files","manifest_restore",
                            "new_nested_manifest_blocks_parent_before_index","stale_nested_manifest_blocks_parent",
                            "deleted_nested_manifest_restores_parent_scope"],
                    initial_index_rev=initial["index_rev"],changed_index_rev=changed_calls["index_rev"],restored_index_rev=restored["index_rev"])
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
