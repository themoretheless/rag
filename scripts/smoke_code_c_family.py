#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_code_c_family.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-c-family-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "c/api.h": "#ifndef C_API_H\n#define C_API_H\nint work(int value);\nvoid callback(void);\n#endif\n",
            "c/api.c": "#include \"api.h\"\nint work(int value){return value;} void callback(void){}\n",
            "c/main.c": "#include \"api.h\"\nstatic int local(void){return 1;} int main(void){callback();void(*cb)(void)=callback;cb();return work(local());}\n",
            "cpp/api.h": "#ifndef CPP_API_H\n#define CPP_API_H\nnamespace api {struct Base {void work();void work(int x);};struct Service:Base {void run();static void stat();};void over(int x);void over(double x); }\n#endif\n",
            "cpp/api.cpp": "#include \"api.h\"\nvoid api::Base::work(){} void api::Base::work(int x){} void api::Service::run(){} void api::Service::stat(){} void api::over(int x){} void api::over(double x){}\n",
            "cpp/main.cpp": "/* ghost(); */\n#include \"api.h\"\nvoid use(api::Service &s){s.run();s.work();api::Service::stat();api::over(1);api::Service *value=new api::Service();delete value;}\nvoid query(){const char *sql=R\"SQL(SELECT *\nFROM users WHERE path='c:\\work')SQL\";}\n",
        }
        for name, content in fixtures.items():
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        import shutil
        compiler = {}
        for name, command, standard, sources in [("C", "clang", "c11", ["c/api.c", "c/main.c"]),("C++", "clang++", "c++17", ["cpp/api.cpp", "cpp/main.cpp"])]:
            executable=shutil.which(command)
            assert executable, f"{command} is required for this qualification fixture"
            result=subprocess.run([executable,"-fsyntax-only",f"-std={standard}"]+[str(source/p) for p in sources],capture_output=True,text=True,timeout=30)
            assert result.returncode==0,(name,result.stderr)
            compiler[name]=dict(version=subprocess.run([executable,"--version"],capture_output=True,text=True,check=True).stdout.splitlines()[0],syntax_only_passed=True,executed=False)
        missing=root/"missing.c";missing.write_text("void use(void){work();}")
        probe=subprocess.run([shutil.which("clang"),"-fsyntax-only","-std=c11","-Werror=implicit-function-declaration",str(missing)],capture_output=True,text=True,timeout=30)
        assert probe.returncode!=0,probe.stdout
        hiding=root/"hiding.cpp";hiding.write_text("void work(int){} struct Base{void work(int){}};struct Child:Base{void work(){}void use(){work(1);}};")
        probe=subprocess.run([shutil.which("clang++"),"-fsyntax-only","-std=c++17",str(hiding)],capture_output=True,text=True,timeout=30)
        assert probe.returncode!=0,probe.stdout
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="c-family-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                names = {tool["name"] for tool in tools["tools"]}
                assert {"references","symbol","callers","callees","inheritance","code","table_usage","sync_sources"} <= names
                index_description=next(t["description"] for t in tools["tools"] if t["name"]=="index_code")
                assert "C/C++" in index_description,index_description
                sync=call("sync_sources",dict(path=str(source),wing="fixture"))
                assert not sync.get("errors") and sync["code_index"]["indexed"]==len(fixtures),sync
                assert not sync["code_index"]["parse_errors"],sync
                paths={name:str(source/name) for name in fixtures}
                def callees(symbol,path):return call("callees",dict(symbol=symbol,path=paths[path]))
                def expr(result,name):return next(r for r in result["items"] if r["reference"]["expression"]==name)
                c=callees("main","c/main.c")
                work=expr(c,"work");assert work["resolution"]=="resolved_static" and len(work["candidates"])==1,c
                assert not expr(c,"cb")["candidates"],c
                assert not call("symbol",dict(symbol="cb"))["items"]
                body=call("code",dict(symbol=work["candidates"][0]));assert "return value" in body["code"],body
                cpp=callees("use","cpp/main.cpp")
                for name in ("s.run","s.work","api::Service::stat","new api::Service"):
                    assert expr(cpp,name)["resolution"]=="resolved_static",(name,cpp)
                assert expr(cpp,"api::over")["resolution"]=="ambiguous" and len(expr(cpp,"api::over")["candidates"])==2,cpp
                run_id=expr(cpp,"s.run")["candidates"][0]
                assert call("symbol",dict(symbol=run_id))["items"][0]["path"]==paths["cpp/api.cpp"]
                assert call("callers",dict(symbol=run_id))["total"]==1
                assert call("references",dict(symbol=run_id))["total"]==1
                callback=call("references",dict(path=paths["c/api.c"],symbol="callback"))
                assert any(r["is_call"]==False for r in callback["items"]),callback
                hierarchy=call("inheritance",dict(path=paths["cpp/api.h"],symbol="api::Service"))
                assert hierarchy["total"]==1 and hierarchy["items"][0]["candidates"],hierarchy
                sql=call("table_usage",dict(project="fixture",table="users",mode="read"))
                expected_line=next(i for i,line in enumerate(fixtures["cpp/main.cpp"].splitlines(),1) if "FROM users" in line)
                assert sql["total"]==1 and sql["items"][0]["evidence"]["line"]==expected_line,sql
                assert not call("symbol",dict(symbol="ghost"))["items"]
                warm=call("sync_sources",dict(path=str(source),wing="fixture"))
                assert warm["code_index"]["unchanged"]==len(fixtures),warm
                api=source/"c/api.h";api.write_text(fixtures["c/api.h"].replace("work(int value)","renamed(int value)"))
                call("ingest_file",dict(path=str(api),wing="fixture"))
                stale=callees("main","c/main.c")
                assert stale["stale_documents"] and not expr(stale,"work")["candidates"],stale
                call("sync_sources",dict(path=str(source),wing="fixture"))
                after=callees("main","c/main.c")
                assert not expr(after,"work")["candidates"] and not after["stale_documents"],after
                api.write_text(fixtures["c/api.h"])
                call("sync_sources",dict(path=str(source),wing="fixture"))
                restored=callees("main","c/main.c")
                assert expr(restored,"work")["candidates"]==work["candidates"],restored
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",embedding="mock",
                    production_database_used=False,compiler=compiler,compiler_missing_declaration_rejected=True,compiler_member_hiding_rejected=True,
                    tools_verified=["symbol","code","callers","callees","references","inheritance","table_usage","sync_sources","ingest_file"],
                    checks=["tool_discovery","C_and_Cpp_parsing","header_guard","header_dialect_selection","external_definition_from_visible_prototype",
                            "static_local_target","indirect_function_pointer_unresolved","namespace_methods","inherited_methods","static_member_body",
                            "overload_ambiguity","type_creation_without_invented_body","callback_reference","comment_not_call","raw_sql_line_evidence",
                            "warm_reuse","stale_header_omitted","changed_header_does_not_guess_global_definition","header_restore"],
                    initial_index_rev=c["index_rev"],changed_index_rev=after["index_rev"],restored_index_rev=restored["index_rev"])
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
