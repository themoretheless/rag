#!/usr/bin/env python3
"""Real MCP cfg-argument binding, feature test overlay and compiler fixture."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from mcp_smoke_client import McpClient


def main():
    binary, output = map(lambda p: Path(p).resolve(), sys.argv[1:3])
    fixture = Path(__file__).resolve().parents[1] / "tests/fixtures/code-intelligence/rust-cfg-arguments"
    runs = []
    with tempfile.TemporaryDirectory(prefix="rag-cfg-arguments-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        shutil.copytree(fixture, source)
        env = {k: v for k, v in os.environ.items() if k not in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_TARGET_DIR")}
        env["CARGO_TARGET_DIR"] = str(root / "cargo-target")
        original = (source / "Cargo.toml").read_text()
        with McpClient(binary, root, source) as client:
            for fast, target in [(True, "left::selected"), (False, "right::selected")]:
                manifest = source / "Cargo.toml"
                manifest.write_text(original if fast else original.replace('features = ["fast"]', 'features = []'))
                for path in [manifest, source / "src/lib.rs"]:
                    client.call("ingest_file", dict(path=str(path), wing="fixture"))
                indexed = client.call("index_code", {})
                symbols = client.call("symbol", dict(project="fixture", limit=500))
                assert not symbols["truncated"]
                by_id = {s["id"]: s for s in symbols["items"]}
                calls = client.call("callees", dict(project="fixture", symbol="entry", path=str(source / "src/lib.rs"), limit=500))
                assert not calls["truncated"] and len(calls["items"]) == 2, calls
                targets = []
                for edge in calls["items"]:
                    assert len(edge["candidates"]) == 1, edge
                    targets.append(by_id[edge["candidates"][0]]["qualified_name"])
                assert sorted(targets) == sorted(["sink", target]), targets
                body_calls = client.call("callees", dict(project="fixture", symbol="body_branch", path=str(source / "src/lib.rs"), limit=500))
                assert not body_calls["truncated"] and len(body_calls["items"]) == 1, body_calls
                body_edge = body_calls["items"][0]
                assert len(body_edge["candidates"]) == 1 and by_id[body_edge["candidates"][0]]["qualified_name"] == target, body_calls
                affected = client.call("affected_tests", dict(project="fixture", symbols=[target], path=str(source / "src/lib.rs"), limit=100))
                assert not affected["truncated"] and [i["symbol"]["name"] for i in affected["items"]] == ["config_test"], affected
                commands = affected["items"][0]["commands"]
                assert len(commands) == 1, commands
                command = commands[0]
                assert command["kind"] == "cargo_test", command
                compiled = subprocess.run(command["argv"], env=env, cwd=source, capture_output=True, text=True)
                assert compiled.returncode == 0 and "1 passed" in compiled.stdout, compiled.stdout + compiled.stderr
                # The real command must run this source/configuration and feature
                # selection; a passing unrelated Cargo command is not evidence.
                assert str(manifest) in command["argv"]
                assert ("--features" in command["argv"]) == fast
                runs.append(dict(fast=fast, expected=target, observed_entry_targets=targets, body_calls=body_calls,
                                 index_report=indexed, affected_tests=affected, command=command,
                                 command_exit=compiled.returncode, stdout=compiled.stdout, stderr=compiled.stderr))
    evidence = dict(status="passed", binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                    transport="actual stdio MCP", runs=runs, production_database_touched=False,
                    limitations=["fixture qualification, not compiler-complete cfg or runtime call graph"])
    with output.open("x") as destination:
        destination.write(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps(dict(status="passed", configurations=2, executed_tests=2)))


if __name__ == "__main__":
    main()
