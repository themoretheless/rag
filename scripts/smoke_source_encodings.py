#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_source_encodings.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-encoding-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        cp_text = "# Разрешённые форматы\ndef work():\n    return 'Привет'\n"
        fixtures = {
            "legacy.py": cp_text.encode("cp1251"),
            "native.py": "# Разрешённые форматы\ndef helper():\n    pass\n",
            "bom.txt": "Разрешённые форматы".encode("utf-8-sig"),
            "wide.txt": "Разрешённые форматы".encode("utf-16"),
            "latin.txt": "café".encode("cp1252"),
            "page.html": "<p>Разрешённые форматы</p>".encode("cp1251"),
        }
        for name, content in fixtures.items():
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(content) if isinstance(content, bytes) else path.write_text(content)
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="encoding-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                names = {tool["name"] for tool in tools["tools"]}
                assert {"references", "symbol", "sync_sources"} <= names
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not sync.get("errors"), sync
                def results():
                    response = call("search_multi", dict(queries=["Разрешённые форматы"], project="fixture"))
                    return {Path(item["document"]["path"]).name: item for item in response["per_query"][0]["items"]}
                found = results()
                assert len(found) == 5, found
                expected = {"legacy.py":"windows-1251", "native.py":"utf-8", "bom.txt":"utf-8-sig", "wide.txt":"utf-16le", "page.html":"windows-1251"}
                for name, encoding in expected.items():
                    assert found[name]["source_encoding"] == encoding, found
                    assert len(found[name]["source_bytes_hash"]) == 64, found
                assert found["legacy.py"]["source_encoding_basis"] == "heuristic_cyrillic_share", found
                assert found["wide.txt"]["source_encoding_basis"] == "bom", found
                assert len(call("symbol", dict(path=str(source / "legacy.py"), symbol="work"))["items"]) == 1
                western = call("search_multi", dict(queries=["café"]))
                assert western["per_query"][0]["items"][0]["source_encoding"] == "windows-1252", western
                warm = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert warm["counters"]["extracted"] == 0 and len(warm["skipped"]) == len(fixtures), warm
                before = found["legacy.py"]
                (source / "legacy.py").write_text(cp_text)
                refreshed = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not refreshed.get("errors"), refreshed
                after = results()["legacy.py"]
                assert after["source_encoding"] == "utf-8", after
                assert after["document"]["content_hash"] == before["document"]["content_hash"], (before, after)
                assert after["source_bytes_hash"] != before["source_bytes_hash"], (before, after)
                assert after["document"]["revision"] > before["document"]["revision"], (before, after)
                assert len(call("symbol", dict(path=str(source / "legacy.py"), symbol="work"))["items"]) == 1
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", production_database_touched=False,
                    checks=["cp1251_search_and_symbols", "utf8_search", "bom_utf8", "bom_utf16", "cp1252", "preserved_html",
                        "encoding_basis", "raw_byte_fingerprint", "warm_no_extraction", "encoding_only_sync_refresh"])
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
