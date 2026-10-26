#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_index_status.py [path/to/rag-mcp] [report.json]
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
    with tempfile.TemporaryDirectory(prefix="rag-index-status-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {
            "alpha/a.py": "def work(value):\n    return value + 1\n\ndef main():\n    return work(3)\n",
            "beta/b.py": "def foreign(): return 1\n",
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="index-status-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                schema = next(t["inputSchema"] for t in tools["tools"] if t["name"] == "index_status")
                assert {"project", "sample_limit", "max_chunk_tokens", "limit"} <= set(schema["properties"]), schema
                empty = call("index_status", {})
                assert empty["state"] == "not_indexed" and empty["quality"]["healthy"] is False, empty
                assert empty["quality"]["chunks"]["norm_p50"] is None, empty
                legacy = request("tools/call", dict(name="index_status"))
                assert not legacy.get("isError"), legacy
                legacy_data = json.loads(next(c["text"] for c in legacy["content"] if c["type"] == "text"))
                assert legacy_data["state"] == empty["state"] and legacy_data["index_rev"] == empty["index_rev"], legacy_data
                for project in ("alpha", "beta"):
                    sync = call("sync_sources", dict(path=str(source / project), wing=project))
                    assert not sync.get("errors"), sync
                first = call("index_status", dict(project="alpha"))
                assert first["contract_version"] == 1 and first["coverage"]["fresh_documents"] == 1, first
                assert first["quality"]["healthy"] is None and first["quality"]["observed_checks_passed"], first
                assert first["quality"]["embedding_identity"]["matches_config"] is True, first
                assert first["quality"]["chunks"]["sampled"] > 0 and first["quality"]["chunks"]["valid_vectors"] > 0, first
                assert first["capabilities"]["cpg"] == dict(available=False, reason="backend_not_implemented"), first
                assert first["capabilities"]["graph_compute"]["backend"] == "native_cpu" and not first["capabilities"]["graph_compute"]["gpu_available"], first
                assert first["capabilities"]["languages"] and all(x["available"] for x in first["capabilities"]["languages"]), first
                assert first["observation_consistency"] == "single_read_transaction", first
                assert "def work" not in json.dumps(first), "source body leaked"
                bounded = call("index_status", dict(project="alpha", sample_limit=1, max_chunk_tokens=1))
                assert bounded["quality"]["chunks"]["sampled"] == 1, bounded
                assert bounded["quality"]["chunks"]["oversized_estimated_tokens"] > 0, bounded
                assert bounded["quality"]["healthy"] is False, bounded
                revisions_before = call("index_revisions", {})
                again = call("index_status", dict(project="alpha"))
                assert first["quality"] == again["quality"] and first["retrieval"] == again["retrieval"], again
                assert call("index_revisions", {}) == revisions_before, "status published a revision"
                beta = source / "beta/b.py"
                beta.write_text("def foreign_changed(): return 2\n")
                call("ingest_file", dict(path=str(beta), wing="beta"))
                local = call("index_status", dict(project="alpha"))
                assert first["coverage"] == local["coverage"] and first["quality"] == local["quality"], local
                global_status = call("index_status", {})
                assert global_status["coverage"]["stale_count"] == 1 and global_status["quality"]["healthy"] is False, global_status
                missing = call("index_status", dict(project="missing"))
                assert missing["state"] == "empty_scope" and missing["quality"]["healthy"] is False, missing
                evidence = dict(status="passed", transport="stdio MCP JSON-RPC", production_database_touched=False,
                    checks=["public_versioned_schema", "legacy_no_arguments", "unindexed_and_empty_not_healthy", "sampled_vectors_and_tokens",
                            "explicit_cpg_degradation", "graph_backend_declared", "bounded_sample", "source_body_not_returned", "no_revision_or_fts_rebuild",
                            "project_isolation", "global_stale_state", "single_read_transaction", "unmeasured_health_is_null"],
                    contract_version=first["contract_version"], extractor_entries=len(first["capabilities"]["languages"]),
                    sampled_chunks=first["quality"]["chunks"]["sampled"])
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
