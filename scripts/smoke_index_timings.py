#!/usr/bin/env python3
"""Exercise the real stdio MCP binary using an isolated mock-embedding corpus.

Usage: python3 scripts/smoke_index_timings.py [path/to/rag-mcp] [report.json]
Never uses the user's database, watcher or external embedding/LLM service.
"""
import json
import math
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
    with tempfile.TemporaryDirectory(prefix="rag-index-timings-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {"alpha/a.py":"def alpha(): pass\n", "beta/b.py":"def beta(): pass\n"}
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
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="index-timings-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                initial=call("index_status",{})
                assert initial["stage_timings"]["available"] is False,initial
                for project in ("alpha","beta"):
                    sync=call("sync_sources",dict(path=str(source/project),wing=project))
                    assert not sync.get("errors"),sync
                published=call("index_status",{})
                timing=published["stage_timings"]
                assert timing["available"] and timing["scope"] == "corpus_global",published
                publication=timing["publication_run"]
                assert publication["revision_created"] is True and publication["index_rev"] == published["index_rev"],publication
                assert published["versions"]["store_schema"] == 25 and published["versions"]["index_format"] >= 24,published
                noop=call("index_code",{})
                assert not noop["revision_created"] and noop["index_revision"] == published["index_rev"],noop
                measured=noop["stage_timings"]
                assert measured["cache_hits"] == 2 and measured["cache_misses"] == 0 and measured["documents_loaded"] == 2,measured
                phases=["lock_wait_ms","source_load_decode_ms","cache_parse_serialize_ms","cache_replace_ms","revision_publish_ms","transaction_body_ms","before_metrics_write_ms"]
                assert all(math.isfinite(measured[p]) and measured[p]>=0 for p in phases),measured
                assert measured["before_metrics_write_ms"] >= measured["transaction_body_ms"],measured
                assert all(measured[f] is False for f in ["commit_included","metrics_write_included","embedding_fts_included"]),measured
                scoped=call("index_status",dict(project="alpha"))
                assert scoped["coverage"]["indexable_documents"] == 1,scoped
                runs=scoped["stage_timings"]
                assert runs["publication_run"] == publication and runs["latest_successful_run"]["run_id"] == measured["run_id"],runs
                assert runs["latest_successful_run"]["metrics"]["documents_loaded"] == 2 and runs["scope"] == "corpus_global",runs
                assert runs["mcp_queue_wait_measured"] is False,runs
                assert call("index_status",dict(project="alpha"))["stage_timings"] == runs,"status created/changed run"
                path=source/"alpha/a.py";path.write_text("def revised_alpha(): pass\n")
                call("ingest_file",dict(path=str(path),wing="alpha"))
                stale=call("index_status",{})
                assert stale["stage_timings"]["publication_run"] == publication and stale["coverage"]["stale_count"] == 1,stale
                changed=call("index_code",{})
                assert changed["revision_created"] and changed["index_revision"] != published["index_rev"],changed
                assert changed["stage_timings"]["cache_hits"] == 1 and changed["stage_timings"]["cache_misses"] == 1,changed
                current=call("index_status",{})
                assert current["stage_timings"]["publication_run"]["run_id"] == changed["stage_timings"]["run_id"],current
                assert current["versions"]["index_format"] == published["versions"]["index_format"],current
                assert "index_stage_timings" not in current["quality"]["unmeasured_checks"],current
                assert "index_commit_timing" in current["quality"]["unmeasured_checks"],current
                evidence=dict(status="passed",transport="stdio MCP JSON-RPC",production_database_touched=False,
                    checks=["unmeasured_before_index","schema_25_stable_index_format_for_timing_runs","source_sync_persists_runs","finite_nonnegative_phases","noop_keeps_revision","publication_not_overwritten","global_scope_explicit","status_does_not_append_runs","stale_source_not_new_measurement","new_revision_has_own_run","excluded_work_declared","quality_unmeasured_boundaries"],
                    measured_phases=phases,cold_publication_revision=published["index_rev"],revised_revision=changed["index_revision"],
                    measured_examples=dict(publication=publication["metrics"],noop=measured,next_publication=changed["stage_timings"]))
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
