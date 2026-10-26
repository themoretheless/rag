#!/usr/bin/env python3
"""Captured impl Self and inline declaration contexts: actual MCP plus Cargo."""
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
    fixture = Path(__file__).resolve().parents[1] / "tests/fixtures/code-intelligence/rust-self-context"
    labels = json.loads((fixture / "expected-call-targets.json").read_text())
    with tempfile.TemporaryDirectory(prefix="rag-rust-self-context-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        shutil.copytree(fixture, source)
        env = {k: v for k, v in os.environ.items() if k not in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_TARGET_DIR")}
        env["CARGO_TARGET_DIR"] = str(root / "cargo-target")
        command = ["cargo", "test", "--offline", "--manifest-path", str(source / "Cargo.toml"), "--no-default-features"]
        compiled = subprocess.run(command, cwd=source, env=env, capture_output=True, text=True)
        assert compiled.returncode == 0 and "1 passed" in compiled.stdout, compiled.stdout + compiled.stderr
        with McpClient(binary, root, source) as client:
            for path in [source / "Cargo.toml", source / "src/lib.rs"]:
                client.call("ingest_file", dict(path=str(path), wing="fixture"))
            indexed = client.call("index_code", {})
            symbols = client.call("symbol", dict(project="fixture", limit=500))
            assert not symbols["truncated"]
            by_id = {s["id"]: s for s in symbols["items"]}
            results = {}
            selected = []
            for label in labels:
                caller = label["caller"]
                if caller not in results:
                    results[caller] = client.call("callees", dict(project="fixture", path=str(source / "src/lib.rs"), symbol=caller, limit=500))
                    assert not results[caller]["truncated"]
                matches = [e for e in results[caller]["items"] if e["reference"]["expression"] == label["expression"]]
                assert len(matches) == 1 and len(matches[0]["candidates"]) == 1, (label, results[caller])
                target = by_id[matches[0]["candidates"][0]]
                assert target["qualified_name"] == label["target"] and Path(target["path"]) == source / "src/lib.rs", (label, target)
                selected.append(dict(expected=label, edge=matches[0], target=target))
            assert sum(len(r["items"]) for r in results.values()) == len(labels), results
            warm = client.call("index_code", {})
            assert not warm["revision_created"] and warm["indexed"] == 0, warm
            status = client.call("index_status", dict(project="fixture"))
    evidence = dict(status="passed", binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                    index_format=status["versions"]["index_format"], labels=labels, selected=selected, results=results,
                    initial_index=indexed, no_op_index=warm, compiler_command=command, compiler_exit=compiled.returncode,
                    compiler_stdout=compiled.stdout, compiler_stderr=compiled.stderr, production_database_touched=False,
                    limitations=["captured declaration types; no full generic/lifetime solver or compiler/runtime proof"])
    with output.open("x") as destination:
        destination.write(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps(dict(status="passed", labelled_targets=len(labels), executed_tests=1)))


if __name__ == "__main__":
    main()
