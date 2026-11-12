#!/usr/bin/env python3
"""Verify declared callback outputs and ? receivers through actual MCP/Cargo."""
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
    fixture = Path(__file__).resolve().parents[1] / "tests/fixtures/code-intelligence/rust-callback-try"
    labels = [("optional", "value.save", "User::save"),
              ("pointer", "f(1)?.save", "Other::save"),
              ("generic", "f()?.save", "User::save"),
              ("borrowed", "f()?.save", "Other::save"),
              ("direct", "factory()?.save", "User::save")]
    with tempfile.TemporaryDirectory(prefix="rag-callback-try-") as temporary:
        root = Path(temporary).resolve()
        source = root / "source"
        shutil.copytree(fixture, source)
        manifest = source / "Cargo.toml"
        rust = source / "src/lib.rs"
        env = {k: v for k, v in os.environ.items() if k not in
               ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_TARGET_DIR")}
        env["CARGO_TARGET_DIR"] = str(root / "cargo-target")
        command = ["cargo", "test", "--offline", "--manifest-path", str(manifest)]
        compiled = subprocess.run(command, env=env, capture_output=True, text=True)
        assert compiled.returncode == 0 and "1 passed" in compiled.stdout, compiled.stdout + compiled.stderr
        selected = []
        unresolved_callbacks = []
        with McpClient(binary, root, source) as client:
            for path in (manifest, rust):
                client.call("ingest_file", dict(path=str(path), wing="fixture"))
            indexed = client.call("index_code", {})
            symbols = client.call("symbol", dict(project="fixture", limit=500))
            assert not symbols["truncated"]
            by_id = {s["id"]: s for s in symbols["items"]}
            for caller, expression, name in labels:
                response = client.call("callees", dict(project="fixture", path=str(rust), symbol=caller, limit=500))
                assert not response["truncated"]
                if caller != "direct":
                    callbacks = [e for e in response["items"] if e["reference"]["expression"] == "f"]
                    assert len(callbacks) == 1 and not callbacks[0]["candidates"], (caller, callbacks)
                    unresolved_callbacks.append(dict(caller=caller, edge=callbacks[0]))
                edges = [e for e in response["items"] if e["reference"]["expression"] == expression]
                assert len(edges) == 1 and len(edges[0]["candidates"]) == 1, (caller, response)
                edge = edges[0]
                target = by_id[edge["candidates"][0]]
                assert target["qualified_name"] == name and Path(target["path"]) == rust, target
                selected.append(dict(caller=caller, expected_expression=expression, expected_target=name, edge=edge, target=target))
            warm = client.call("index_code", {})
            assert not warm["revision_created"] and warm["indexed"] == 0, warm
            status = client.call("index_status", dict(project="fixture"))
    result = dict(status="passed", binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                  index_format=status["versions"]["index_format"], indexed=indexed, warm=warm, selected=selected,
                  unresolved_callback_bodies=unresolved_callbacks,
                  fixture_sha256={str(p.relative_to(fixture)): hashlib.sha256(p.read_bytes()).hexdigest()
                                  for p in fixture.rglob("*") if p.is_file()},
                  compiler=dict(command=command, exit_code=compiled.returncode, stdout=compiled.stdout, stderr=compiled.stderr),
                  limitations=["declared callback output only; callback body targets, argument types, custom Try and HRTB unsupported"])
    with output.open("x") as destination:
        json.dump(result, destination, indent=2)
        destination.write("\n")
    print(json.dumps(dict(status="passed", targets=len(selected), executed_tests=1)))


if __name__ == "__main__":
    main()
