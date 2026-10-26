#!/usr/bin/env python3
"""Real MCP test recommendations and actual execution of every Rust command."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
from mcp_smoke_client import McpClient


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp").resolve()
    report = Path(sys.argv[2]) if len(sys.argv) > 2 else None
    fixture = Path(__file__).resolve().parents[1] / "tests/fixtures/code-intelligence/affected-tests"
    labels = json.loads((fixture / "expected.json").read_text())
    command_evidence = []
    command_logs = []
    with tempfile.TemporaryDirectory(prefix="rag-affected-tests-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        shutil.copytree(fixture, source)
        with McpClient(binary, root, source) as client:
            call = client.call
            sync = call("sync_sources", dict(path=str(source), wing="fixture"))
            assert not sync.get("errors"), sync
            args = dict(symbols=[labels["seed"]], project="fixture")
            result = call("affected_tests", args)
            names = sorted(item["symbol"]["name"] for item in result["items"])
            assert names == labels["references_tests"], result
            assert not result["coverage_complete"] and not result["safe_to_skip_unlisted_tests"] and result["fallback_commands"]
            assert result == call("affected_tests", args)
            for item in result["items"]:
                path = item["path"]
                assert path["complete_to_seed"] and path["symbols"][-1]["name"] == "api"
                assert item["commands"] and not item["commands_truncated"]
                for step, symbol in zip(path["steps"], path["symbols"]):
                    ref = step["evidence"]["reference"]
                    assert ref["document_id"] == symbol["document_id"] and ref["content_hash"] == symbol["content_hash"] and ref["line"] > 0
                if item["symbol"]["name"] == "callback_value":
                    assert path["steps"][0]["relation"] == "bound_reference"
                for command in item["commands"]:
                    assert command["kind"] == "cargo_test" and command["filter_mode"] == "substring"
                    env = dict(os.environ, CARGO_TARGET_DIR=str(root / "cargo-target"), CARGO_NET_OFFLINE="true")
                    run = subprocess.run(command["argv"], cwd=command["cwd"], env=env, text=True, stdout=subprocess.PIPE,
                                         stderr=subprocess.PIPE, timeout=120)
                    command_logs.append(json.dumps(command["argv"]) + "\n" + run.stdout + run.stderr)
                    assert run.returncode == 0 and re.search(r"test result: ok\. 1 passed; 0 failed", run.stdout), command_logs[-1]
                    command_evidence.append(dict(test=item["symbol"]["name"], argv=command["argv"], exit_code=run.returncode,
                                                 executed_tests=1, stdout_sha256=hashlib.sha256(run.stdout.encode()).hexdigest()))
            calls = call("affected_tests", dict(args, relations="calls"))
            assert sorted(i["symbol"]["name"] for i in calls["items"]) == labels["calls_tests"]
            deeper = call("affected_tests", dict(args, symbols=["value"]))
            assert sorted(i["symbol"]["name"] for i in deeper["items"]) == labels["references_tests"]
            assert all(len(i["path"]["steps"]) == 2 for i in deeper["items"])
            shallow = call("affected_tests", dict(args, symbols=["value"], depth=1))
            assert not shallow["items"] and shallow["depth_limited"]
            limited = call("affected_tests", dict(args, limit=1))
            assert len(limited["items"]) == 1 and limited["truncated"]
            exhausted = call("affected_tests", dict(args, max_work=1))
            assert exhausted["work_exhausted"] and not exhausted["safe_to_skip_unlisted_tests"]
            direct = call("affected_tests", dict(args, symbols=["unit_direct"]))
            assert direct["items"][0]["reason"] == "changed_test" and not direct["items"][0]["path"]["steps"]
            invalid = client.request("tools/call", dict(name="affected_tests", arguments=dict(args, relations="guessed")), expect_error=True)
            assert invalid["error"]["code"] == -32602
            lib = source / "src/lib.rs"
            lib.write_text(lib.read_text() + "\n// Captured-source refresh probe.\n")
            assert call("affected_tests", args) == result
            call("ingest_file", dict(path=str(lib), wing="fixture"))
            stale = call("affected_tests", args)
            assert stale["status"] == "seed_unavailable" and stale["stale_documents"] and not stale["items"], stale
            call("index_code", {})
            rebound = call("affected_tests", args)
            assert sorted(i["symbol"]["name"] for i in rebound["items"]) == labels["references_tests"]
            def git(*params):
                subprocess.run(["git", "-C", str(source), *params], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            git("init", "-q")
            git("add", ".")
            git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", "base")
            (source / "src/worker.rs").write_text("pub fn value() -> i32 { 2 }\n")
            git_result = call("detect_changes", dict(repository=str(source), scope="all", include_tests=True, depth=10))
            for side in ("before_impact", "after_impact"):
                tests = git_result[side]["affected_tests"]
                assert sorted(i["symbol"]["name"] for i in tests["items"]) == labels["references_tests"], git_result
                assert all(len(i["path"]["steps"]) == 2 for i in tests["items"])
                assert not tests["safe_to_skip_unlisted_tests"]
            evidence = dict(status="passed", transport="stdio MCP", index_format=call("index_status", dict(project="fixture"))["versions"]["index_format"],
                            binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(), labelled_tests=names, executed_commands=command_evidence,
                            fixture_sha256={str(p.relative_to(fixture)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(fixture.rglob("*")) if p.is_file()},
                            checks=["unit_alias_and_super_paths", "cfg_test_external_module", "integration_hyphen_target", "callback_reference", "ignored_test_execution",
                                    "all_suggested_commands_execute_one_test", "multihop_witnesses", "calls_vs_references", "depth_result_work_limits", "direct_changed_test",
                                    "typed_input", "captured_source_semantics", "stale_exclusion_refresh", "git_before_after_test_recommendations"],
                            production_database_touched=False, safe_to_skip_unlisted_tests=False, runtime_coverage_measured=False)
            if report:
                report.parent.mkdir(parents=True, exist_ok=True)
                report.write_text(json.dumps(evidence, indent=2) + "\n")
                report.with_name("commands.log").write_text("\n".join(command_logs))
            print(json.dumps(evidence))


if __name__ == "__main__":
    main()
