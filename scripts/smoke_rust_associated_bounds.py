#!/usr/bin/env python3
"""Actual MCP/Cargo qualification of trait associated bounds and cfg switching."""
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
    binary, output = [Path(p).resolve() for p in sys.argv[1:]]
    fixture = Path(__file__).resolve().parents[1] / "tests/fixtures/code-intelligence/rust-associated-bounds"
    runs = []
    with tempfile.TemporaryDirectory(prefix="rag-associated-bounds-") as directory:
        root = Path(directory).resolve()
        source = root / "source"
        shutil.copytree(fixture, source)
        manifest = source / "Cargo.toml"
        original = manifest.read_text()
        rust = source / "src/lib.rs"
        env = {k: v for k, v in os.environ.items() if k not in
               ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_TARGET_DIR")}
        env["CARGO_TARGET_DIR"] = str(root / "cargo-target")
        previous = None
        with McpClient(binary, root, source) as client:
            for path in (manifest, rust):
                client.call("ingest_file", dict(path=str(path), wing="fixture"))
            for phase, owner in [("alternate", "OtherDone"), ("default", "Done")]:
                stale = None
                if phase == "default":
                    manifest.write_text(original.replace('features = ["alternate"]', 'features = []'))
                    client.call("ingest_file", dict(path=str(manifest), wing="fixture"))
                    stale = client.call("callees", dict(project="fixture", path=str(rust), symbol="run", limit=500))
                    assert all(not e["candidates"] for e in stale["items"]), stale
                indexed = client.call("index_code", {})
                if phase == "default":
                    assert indexed["indexed"] == 1 and indexed["unchanged"] == 1, indexed
                symbols = client.call("symbol", dict(project="fixture", limit=500))
                assert not symbols["truncated"]
                by_id = {s["id"]: s for s in symbols["items"]}
                calls = client.call("callees", dict(project="fixture", path=str(rust), symbol="run", limit=500))
                assert not calls["truncated"] and len(calls["items"]) == 4, calls
                expected = {"T::make": "Factory::make", "T::make().output": "Factory::output",
                            "T::make().output().again": f"{owner}::again",
                            "T::make().output().again().ready": f"{owner}::ready"}
                selected = []
                for edge in calls["items"]:
                    expression = edge["reference"]["expression"]
                    target_name = expected.pop(expression)
                    assert len(edge["candidates"]) == 1, edge
                    target = by_id[edge["candidates"][0]]
                    assert target["qualified_name"] == target_name and Path(target["path"]) == rust, target
                    selected.append(dict(expected=target_name, edge=edge, target=target))
                assert not expected
                warm = client.call("index_code", {})
                assert not warm["revision_created"] and warm["indexed"] == 0, warm
                command = ["cargo", "test", "--offline", "--manifest-path", str(manifest), "--no-default-features"]
                if phase == "alternate":
                    command += ["--features", "alternate"]
                compiled = subprocess.run(command, env=env, capture_output=True, text=True)
                assert compiled.returncode == 0 and "1 passed" in compiled.stdout, compiled.stdout + compiled.stderr
                diff = None
                if previous is not None:
                    diff = client.call("diff", dict(project="fixture", before=previous, after=indexed["index_revision"]))
                    assert diff["edges_added_total"] and diff["edges_removed_total"], diff
                previous = indexed["index_revision"]
                runs.append(dict(phase=phase, selected=selected, indexed=indexed, warm=warm, stale=stale, diff=diff,
                                 compiler=dict(command=command, exit_code=compiled.returncode,
                                               stdout=compiled.stdout, stderr=compiled.stderr)))
            status = client.call("index_status", dict(project="fixture"))
    result = dict(status="passed", binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                  fixture_sha256={str(p.relative_to(fixture)): hashlib.sha256(p.read_bytes()).hexdigest()
                                  for p in fixture.rglob("*") if p.is_file()},
                  index_format=status["versions"]["index_format"], runs=runs,
                  limitations=["trait declaration candidates, not concrete dispatch; GAT/generic-trait bounds unsupported"])
    with output.open("x") as destination:
        json.dump(result, destination, indent=2)
        destination.write("\n")
    print(json.dumps(dict(status="passed", configurations=2, labelled_targets=8, executed_tests=2)))


if __name__ == "__main__":
    main()
