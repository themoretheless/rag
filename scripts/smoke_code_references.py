#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_code_references.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-bound-references-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "py/api.py": "def work():\n    helper()\ndef helper():\n    pass\nclass Base:\n    def run(self):\n        pass\nclass Child(Base):\n    pass\n",
            "py/main.py": "from api import work as execute, Child\nregistered = execute\ndef shadow(execute=None):\n    return execute\ndef use(c: Child):\n    return c.run\n",
            "py/other.py": "def work():\n    pass\ndef use():\n    return work\n",
            "rust/src/api.rs": "pub fn work() {} pub struct Service; impl Service { pub fn run(&self) {} }",
            "rust/src/main.rs": "use crate::api::{work as execute, Service}; fn main() { let cb = execute; } fn use_it(s: Service) { s.run(); }",
            "ts/api.ts": "export default function work() {} export class Base { run() {} } export class Child extends Base {}",
            "ts/main.ts": "import execute, {Child} from './api'; const cb = execute; function use(c: Child) { return c.run; } function run() {} const x = unknown.run;",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="bound-references-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                names = {tool["name"] for tool in tools["tools"]}
                assert {"references", "symbol", "sync_sources"} <= names
                schema = next(t["inputSchema"] for t in tools["tools"] if t["name"] == "references")
                assert "reference_mode" in schema["properties"]
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not sync.get("errors"), sync
                paths = {name: str(source / name) for name in fixtures}
                results = {}
                for name in ("py/api.py", "rust/src/api.rs", "ts/api.ts"):
                    symbols = call("symbol", dict(path=paths[name], symbol="work"))["items"]
                    assert len(symbols) == 1, (name, symbols, call("symbol",dict(symbol="work")),sync)
                    symbol_id = symbols[0]["id"]
                    refs = call("references", dict(symbol=symbol_id))
                    assert refs["reference_mode"] == "bound" and refs["references_are_lexical"] is False
                    assert any(r["expression"] == "execute" and not r["is_call"] for r in refs["items"]), refs
                    assert all(r["candidates"] == [symbol_id] for r in refs["items"]), refs
                    assert not any(r["path"] == paths["py/other.py"] for r in refs["items"]), refs
                    assert all(r["content_hash"] and r["revision"] for r in refs["items"])
                    results[name] = dict(target=symbol_id,total=refs["total"])
                py_id = results["py/api.py"]["target"]
                before = call("references", dict(symbol=py_id))
                assert not any(r["line"] == 4 for r in before["items"]), before
                lexical = call("references", dict(symbol=py_id, reference_mode="lexical"))
                assert lexical["references_are_lexical"] and any(r["path"] == paths["py/other.py"] for r in lexical["items"])
                bounded = call("references", dict(symbol=py_id, limit=1))
                assert bounded["truncated"] and len(bounded["items"]) == 1
                assert bounded == call("references", dict(symbol=py_id, limit=1))
                for name in ("py/api.py", "ts/api.ts"):
                    refs = call("references", dict(path=paths[name], symbol="Base::run"))
                    assert any(r["expression"] == "c.run" for r in refs["items"]), refs
                assert call("references", dict(path=paths["ts/main.ts"], symbol="run"))["total"] == 0
                dead = call("dead_code", {})
                assert not any(r["symbol"]["path"] == paths["py/api.py"] and r["symbol"]["name"] == "helper" for r in dead["items"]), dead
                warm = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert warm["code_index"]["unchanged"] == len(fixtures), warm
                main = source / "py/main.py"
                main.write_text("def unrelated():\n    pass\n")
                call("ingest_file", dict(path=str(main), wing="fixture"))
                stale = call("references", dict(symbol=py_id))
                assert stale["stale_documents"] and not any(r["path"] == str(main) for r in stale["items"]), stale
                refreshed = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not refreshed.get("errors"), refreshed
                after = call("references", dict(symbol=py_id))
                assert after["total"] == 0 and not after["stale_documents"], after
                assert after["index_rev"] > before["index_rev"]
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", protocol=init["protocolVersion"],
                    embedding="mock", external_database=False,
                    tools_verified=["references","symbol","dead_code","sync_sources","ingest_file"],
                    checks=["tool_schema", "Python_alias_callback", "Rust_import_value", "TS_default_import",
                            "typed_inherited_member_values", "same_name_isolation", "parameter_default_shadowing",
                            "lexical_mode", "stable_pagination", "callback_transitive_liveness", "warm_index_reuse",
                            "stale_input_omitted", "source_sync_reference_refresh"],
                    initial_index_rev=before["index_rev"], updated_index_rev=after["index_rev"], fixture_results=results)
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
