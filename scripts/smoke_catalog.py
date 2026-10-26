#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_catalog.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-catalog-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {"alpha/a.py":"def alpha_a(): pass\n", "alpha/b.py":"def alpha_b(): pass\n", "beta/b.py":"def beta(): pass\n"}
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="catalog-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools=request("tools/list",{})
                schema=next(t["inputSchema"] for t in tools["tools"] if t["name"] == "catalog")
                assert {"view","cursor","index_revision","project","document_id","path","limit"} <= set(schema["properties"]),schema
                call("ingest_file",dict(path=str(source/"alpha/a.py"),wing="alpha"))
                registered=call("catalog",dict(view="projects",project="alpha"))
                assert registered["index_rev"] is None and registered["items"][0]["state"] == "unindexed",registered
                for project in ("alpha","beta"):
                    sync=call("sync_sources",dict(path=str(source/project),wing=project))
                    assert not sync.get("errors"),sync
                legacy=call("catalog",{})
                assert legacy["total"] == 3 and legacy["view"] == "immutable_revision_manifest",legacy
                assert "def alpha_a" not in json.dumps(legacy) and legacy["source_text_loaded_into_rust"] is False
                baseline=call("catalog",dict(project="alpha",limit=10))
                first=call("catalog",dict(project="alpha",limit=1))
                project_first=call("catalog",dict(view="projects",limit=1))
                item=project_first["items"][0]
                assert item["project"] == "alpha" and item["captured_hashes_match"] is True,item
                assert item["hash_matches"] is None and item["filesystem_verified"] is False,item
                assert item["roots"][0]["canonical_root"] == str(source/"alpha"),item
                second_project=call("catalog",dict(view="projects",cursor=project_first["next_cursor"],limit=1))
                assert second_project["items"][0]["project"] == "beta",second_project
                tail=baseline["items"][1]
                Path(tail["path"]).write_text("def changed_after_page(): pass\n")
                call("ingest_file",dict(path=tail["path"],wing="alpha"))
                drift=call("catalog",dict(view="projects",project="alpha"))
                assert drift["items"][0]["state"] == "captured_drift" and drift["items"][0]["captured_hashes_match"] is False,drift
                def failure(arguments,message):
                    result=request("tools/call",dict(name="catalog",arguments=arguments),allow_error=True)
                    assert result.get("isError") or ("code" in result and "message" in result),result
                    assert message in json.dumps(result),result
                failure(dict(view="projects",cursor=project_first["next_cursor"],limit=1),"CATALOG_OBSERVATION_CHANGED")
                failure(dict(project="beta",cursor=first["next_cursor"]),"does not match")
                call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
                latest=call("catalog",dict(project="alpha"))
                assert latest["index_rev"] != first["index_rev"],latest
                continued=call("catalog",dict(project="alpha",cursor=first["next_cursor"],limit=2))
                assert continued["index_rev"] == first["index_rev"] and continued["items"][0]["content_hash"] == tail["content_hash"],continued
                assert len(continued["items"]) == 1 and not continued["has_more"],continued
                restored=call("catalog",dict(view="projects",project="alpha"))
                assert restored["items"][0]["captured_hashes_match"] is True,restored
                historical=call("catalog",dict(view="projects",project="alpha",index_revision=first["index_rev"]))
                assert historical["items"][0]["captured_hashes_match"] is False,historical
                assert call("catalog",dict(view="projects",project="alpha' OR 1=1 --"))["total"] == 0
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",production_database_touched=False,
                    checks=["public_schema","legacy_file_manifest","registered_before_index","project_registry","captured_hashes_not_live_proof","sync_roots","project_pages","captured_drift","observation_conflict","cursor_scope_guard","retained_revision_file_pages","historical_project_comparison","metadata_only_file_payload","parameter_binding"],
                    initial_files=3,project_page_lengths=[len(project_first["items"]),len(second_project["items"])])
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
