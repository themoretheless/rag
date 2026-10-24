#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_code_java.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-java-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "api/Base.java": "package api; public class Base {public void work(){} public void work(int n){} public void baseOnly(){}}",
            "api/Api.java": "package api; public interface Api {void read();}",
            "api/Service.java": "package api; @interface Note {String value();} public class Service extends Base implements Api {public Service(){} public Service(int n){} public void work(){} @Note(\"static private\") public void read(){} public void ambiguous(int n){} public void ambiguous(String s){} public void delegated(){super.work();baseOnly();}}",
            "app/Main.java": "package app; import api.Service; import api.Api; class Main {Service field; public static void main(String[] args){Service s=new Service();s.work();} public void use(Service s,Api api){s.work();s.work(1);s.ambiguous(1);this.field.read();api.read();Runnable cb=s::work;java.util.function.Consumer<Service> unbound=Service::read;java.util.function.Supplier<Service> ctor=Service::new;new Pair(1);}}",
            "api/Utility.java": "package api; public class Utility {public static void ping(){}}",
            "app/Statics.java": "package app; import static api.Utility.ping; class Statics {void use(){ping();}}",
            "app/Check.java": "package app; @interface Test {} class Check {@Test void check(){}} record Pair(int x){Pair{}} enum Mode {ONE}",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="java-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                names = {tool["name"] for tool in tools["tools"]}
                assert {"references","symbol","callers","callees","inheritance","code","sync_sources"} <= names
                index_description=next(t["description"] for t in tools["tools"] if t["name"]=="index_code")
                assert "Go/Java" in index_description,index_description
                sync=call("sync_sources",dict(path=str(source),wing="fixture"))
                assert not sync.get("errors") and sync["code_index"]["indexed"]==len(fixtures),sync
                assert not sync["code_index"]["parse_errors"],sync
                paths={name:str(source/name) for name in fixtures}
                def callees(symbol,path="app/Main.java"):
                    return call("callees",dict(path=paths[path],symbol=symbol))
                def expressions(result,name):return [r for r in result["items"] if r["reference"]["expression"]==name]
                constructors=call("symbol",dict(path=paths["api/Service.java"],symbol="Service::Service"))
                assert constructors["total"]==2 and all(r["kind"]=="constructor_declaration" for r in constructors["items"]),constructors
                initial=callees("Main::use")
                work=expressions(initial,"s.work")
                assert len(work)==2 and all(len(r["candidates"])==1 for r in work),initial
                assert work[0]["candidates"]!=work[1]["candidates"],work
                ambiguous=expressions(initial,"s.ambiguous")[0]
                assert ambiguous["resolution"]=="ambiguous" and len(ambiguous["candidates"])==2,ambiguous
                assert expressions(initial,"api.read")[0]["resolution_basis"]=="java_interface_contract",initial
                assert expressions(initial,"this.field.read")[0]["resolution"]=="resolved_static",initial
                assert expressions(initial,"new Pair")[0]["resolution"]=="resolved_static",initial
                main=callees("Main::main")
                assert expressions(main,"new Service")[0]["resolution"]=="resolved_static",main
                body=call("code",dict(symbol=work[0]["candidates"][0]))
                assert "void work()" in body["code"],body
                callers=call("callers",dict(symbol=work[0]["candidates"][0]))
                assert any(r["reference"]["path"]==paths["app/Main.java"] for r in callers["items"]),callers
                references=call("references",dict(path=paths["api/Service.java"],symbol="Service::work"))
                assert any(r["expression"]=="s::work" for r in references["items"]),references
                constructor_refs=call("references",dict(path=paths["api/Service.java"],symbol="Service::Service"))
                assert any(r["expression"]=="Service::new" and r["resolution"]=="ambiguous" for r in constructor_refs["items"]),constructor_refs
                assert call("references",dict(path=paths["api/Service.java"],symbol="Service::read"))["total"]>=2
                hierarchy=call("inheritance",dict(path=paths["api/Service.java"],symbol="Service"))
                assert hierarchy["total"]==2 and all(r["hierarchy_status"]=="complete_static" for r in hierarchy["items"]),hierarchy
                delegated=callees("Service::delegated","api/Service.java")
                assert expressions(delegated,"super.work")[0]["resolution"]=="resolved_static",delegated
                assert expressions(callees("Statics::use","app/Statics.java"),"ping")[0]["resolution_basis"]=="java_static_import"
                entries=call("entrypoints",{})
                assert any(r["kind"]=="conventional_main_candidate" for r in entries["items"]),entries
                assert any(r["kind"]=="java_test_candidate" for r in entries["items"]),entries
                warm=call("sync_sources",dict(path=str(source),wing="fixture"))
                assert warm["code_index"]["unchanged"]==len(fixtures),warm
                base=source/"api/Base.java"
                base.write_text(fixtures["api/Base.java"].replace("work(int n)","renamed(int n)"))
                call("ingest_file",dict(path=str(base),wing="fixture"))
                stale=callees("Main::use")
                assert stale["stale_documents"] and not expressions(stale,"s.work")[1]["candidates"],stale
                changed=call("sync_sources",dict(path=str(source),wing="fixture"))
                assert not changed.get("errors"),changed
                after=callees("Main::use")
                assert not expressions(after,"s.work")[1]["candidates"] and not after["stale_documents"],after
                assert after["index_rev"]>initial["index_rev"]
                base.write_text(fixtures["api/Base.java"])
                call("sync_sources",dict(path=str(source),wing="fixture"))
                restored=callees("Main::use")
                assert expressions(restored,"s.work")[1]["candidates"]==work[1]["candidates"],restored
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",embedding="mock",
                    production_database_used=False,javac_executed=False,
                    tools_verified=["symbol","callers","callees","references","inheritance","code","entrypoints","sync_sources","ingest_file"],
                    checks=["Java_tool_discovery","Java_grammar","constructors","overload_arity","same_arity_ambiguity","inherited_overloads", "override",
                            "typed_fields","annotation_modifiers_are_structural","interface_contract","static_import","super_method","method_references",
                            "constructor_references","record_compact_constructor","main_and_test_candidates",
                            "source_body","warm_reuse","stale_base_omitted","source_sync_rebinds_hierarchy","restore"],
                    initial_index_rev=initial["index_rev"],changed_index_rev=after["index_rev"],restored_index_rev=restored["index_rev"])
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
