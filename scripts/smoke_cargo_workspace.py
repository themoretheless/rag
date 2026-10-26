#!/usr/bin/env python3
"""Real MCP qualification on an isolated labelled Cargo workspace, never user DBs.

python3 scripts/smoke_cargo_workspace.py [binary] [report.json]
"""
import json
import hashlib
import os
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp").resolve()
    report_path = Path(sys.argv[2]) if len(sys.argv) > 2 else None
    fixture = Path(__file__).resolve().parents[1] / "tests/fixtures/code-intelligence/cargo-workspace"
    labels = json.loads((fixture / "expectations.json").read_text())
    with tempfile.TemporaryDirectory(prefix="rag-cargo-workspace-") as temporary:
        root = Path(temporary).resolve()
        source = root / "workspace"
        shutil.copytree(fixture, source)
        env = {k: v for k, v in os.environ.items() if not k.startswith("RAG_")}
        env.update(RAG_DB_PATH=str(root / "test.duckdb"), RAG_INGEST_ROOTS=str(source),
                   RAG_EMBEDDING_PROVIDER="mock", RAG_EMBEDDING_DIMS="16",
                   RAG_LLM_ENABLED="false", RAG_TOOLS="full")
        with (root / "stderr.log").open("w") as stderr:
            process = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                       stderr=stderr, text=True, env=env)
            incoming = queue.Queue()
            def receive():
                try:
                    for line in process.stdout:
                        incoming.put(json.loads(line))
                finally:
                    incoming.put(None)
            threading.Thread(target=receive, daemon=True).start()
            sequence = 0
            def request(method, params):
                nonlocal sequence
                sequence += 1
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", id=sequence, method=method, params=params)) + "\n")
                process.stdin.flush()
                deadline = time.monotonic() + 60
                while True:
                    result = incoming.get(timeout=max(.01, deadline - time.monotonic()))
                    assert result is not None, (root / "stderr.log").read_text()[-4000:]
                    if result.get("id") == sequence:
                        assert "error" not in result, result
                        return result["result"]
            def call(name, arguments):
                result = request("tools/call", dict(name=name, arguments=arguments))
                assert not result.get("isError"), result
                return json.loads(next(c["text"] for c in result["content"] if c["type"] == "text"))
            try:
                request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="cargo-workspace-qualification", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                start = time.monotonic()
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                cold_sync_ms = (time.monotonic() - start) * 1000
                assert not sync.get("errors"), sync
                symbols = call("symbol", dict(limit=500))
                assert not symbols["truncated"], symbols
                symbol_map = {s["id"]: s for s in symbols["items"]}
                outcomes = []
                for label in labels["calls"]:
                    result = call("callees", dict(path=str(source / label["file"]), symbol=label["caller"]))
                    found = [c for c in result["items"] if c["reference"]["expression"] == label["expression"]]
                    assert len(found) == 1 and not result["truncated"], result
                    edge = found[0]
                    actual = sorted(str(Path(symbol_map[i]["path"]).relative_to(source)) for i in edge["candidates"])
                    assert actual == sorted(label["targets"]), (label, actual, edge)
                    assert edge["provenance"] == "derived" and edge["resolution"] == "resolved_static", edge
                    outcomes.append(dict(**label, actual=actual, wrong_targets=0))
                status = call("index_status", dict(project="fixture"))
                targets = status["cargo_targets"]
                assert len(targets) == 5, targets
                assert all(not t["complete"] and not t["features_evaluated"] for t in targets), targets
                before = status["index_rev"]
                noop = call("index_code", {})
                assert not noop["revision_created"] and noop["indexed"] == 0, noop
                # Only the workspace dependency changes; every Rust source stays byte-identical.
                manifest = source / "Cargo.toml"
                manifest.write_text(manifest.read_text().replace('package = "fixture-engine", path = "engine"', 'package = "fixture-other", path = "other"'))
                call("ingest_file", dict(path=str(manifest), wing="fixture"))
                # The stale virtual workspace cannot supply inherited dependency facts.
                stale = call("callees", dict(path=str(source / "app/src/main.rs"), symbol="main"))
                for edge in stale["items"]:
                    if edge["reference"]["expression"].startswith("engine_alias::"):
                        assert edge["candidates"] == [], stale
                update = call("index_code", {})
                assert update["indexed"] == 1 and update["revision_created"], update
                rebound = call("callees", dict(path=str(source / "app/src/main.rs"), symbol="main"))
                alias_edges = [e for e in rebound["items"] if e["reference"]["expression"].startswith("engine_alias::")]
                assert len(alias_edges) == 2
                for edge in alias_edges:
                    assert len(edge["candidates"]) == 1, edge
                    assert Path(symbol_map[edge["candidates"][0]]["path"]).is_relative_to(source / "other"), edge
                diff_args = dict(before=before, after=update["index_revision"], project="fixture")
                diff = call("diff", diff_args)
                assert diff["edges_added_total"] == 2 and diff["edges_removed_total"] == 2, diff
                assert diff["symbol_changes_total"] == 0, diff
                manifest.write_text("[workspace]\nmembers=[]\n")
                assert call("diff", diff_args) == diff, "historical graph used live filesystem"
                evidence = dict(status="passed", transport="stdio MCP", scope=labels["scope"],
                    binary_sha256=hashlib.file_digest(binary.open("rb"), "sha256").hexdigest(),
                    index_format=status["versions"]["index_format"],
                    production_database_touched=False, source_build_scripts_executed=False,
                    labelled_calls=outcomes, cargo_targets=len(targets),
                    checks=["target_roots", "custom_library_root", "renamed_workspace_dependency", "wrong_owner_negatives",
                            "warm_reuse", "stale_workspace_blocks_inherited_dependency", "manifest_only_rebinding", "frozen_history"],
                    cold_sync_ms=cold_sync_ms, before_revision=before, after_revision=update["index_revision"],
                    representative_quality_measured=False, competitor_comparison_performed=False)
                if report_path:
                    report_path.parent.mkdir(parents=True, exist_ok=True)
                    report_path.write_text(json.dumps(evidence, indent=2) + "\n")
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
