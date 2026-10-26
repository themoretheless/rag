#!/usr/bin/env python3
"""Actual MCP/Cargo checks for cfg fields, initializers and extern namespaces."""
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
    binary, output = [Path(p).resolve() for p in sys.argv[1:3]]
    fixture = Path(__file__).resolve().parents[1] / "tests/fixtures/code-intelligence/rust-cfg-members"
    labels = json.loads((fixture / "expected-call-targets.json").read_text())
    runs = []
    with tempfile.TemporaryDirectory(prefix="rag-cfg-members-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        shutil.copytree(fixture, source)
        manifest = source / "Cargo.toml"
        original = manifest.read_text()
        env = {k: v for k, v in os.environ.items() if k not in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_TARGET_DIR")}
        env["CARGO_TARGET_DIR"] = str(root / "cargo-target")
        previous = None
        with McpClient(binary, root, source) as client:
            for path in [manifest, source / "src/lib.rs"]:
                client.call("ingest_file", dict(path=str(path), wing="fixture"))
            for phase in ["fast", "slow"]:
                stale = None
                if phase == "slow":
                    manifest.write_text(original.replace('features = ["fast"]', 'features = []'))
                    client.call("ingest_file", dict(path=str(manifest), wing="fixture"))
                    stale = client.call("callees", dict(project="fixture", path=str(source / "src/lib.rs"), symbol="Holder::run", limit=500))
                    assert all(not edge["candidates"] for edge in stale["items"]), stale
                indexed = client.call("index_code", {})
                if phase == "slow":
                    assert indexed["indexed"] == 1 and indexed["unchanged"] == 1, indexed
                symbols = client.call("symbol", dict(project="fixture", limit=500))
                assert not symbols["truncated"]
                by_id = {s["id"]: s for s in symbols["items"]}
                selected = []
                for label in labels[phase]:
                    result = client.call("callees", dict(project="fixture", path=str(source / "src/lib.rs"), symbol=label["caller"], limit=500))
                    assert not result["truncated"] and len(result["items"]) == 1, (label, result)
                    edge = result["items"][0]
                    assert edge["reference"]["expression"] == label["expression"] and len(edge["candidates"]) == 1, (label, edge)
                    target = by_id[edge["candidates"][0]]
                    assert target["qualified_name"] == label["target"] and Path(target["path"]) == source / "src/lib.rs", (label, target)
                    selected.append(dict(expected=label, edge=edge, target=target))
                warm = client.call("index_code", {})
                assert not warm["revision_created"] and warm["indexed"] == 0, warm
                command = ["cargo", "test", "--offline", "--manifest-path", str(manifest), "--no-default-features"]
                if phase == "fast":
                    command += ["--features", "fast"]
                compiled = subprocess.run(command, cwd=source, env=env, capture_output=True, text=True)
                assert compiled.returncode == 0 and "1 passed" in compiled.stdout, compiled.stdout + compiled.stderr
                diff = None
                if previous is not None:
                    diff = client.call("diff", dict(project="fixture", before=previous, after=indexed["index_revision"]))
                    assert diff["edges_added_total"] and diff["edges_removed_total"], diff
                previous = indexed["index_revision"]
                runs.append(dict(phase=phase, selected=selected, index_report=indexed, warm_index=warm, stale=stale, diff=diff,
                                 compiler_command=command, compiler_exit=compiled.returncode, compiler_stdout=compiled.stdout, compiler_stderr=compiled.stderr))
            status = client.call("index_status", dict(project="fixture"))
    evidence = dict(status="passed", index_format=status["versions"]["index_format"], binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                    transport="actual stdio MCP", runs=runs, production_database_touched=False,
                    limitations=["captured configuration and named-field subset, not full compiler/macro/layout proof"])
    with output.open("x") as destination:
        destination.write(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps(dict(status="passed", configurations=2, labelled_targets=8, executed_tests=2)))


if __name__ == "__main__":
    main()
