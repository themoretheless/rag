#!/usr/bin/env python3
"""Actual MCP and aarch64 compiler execution for standard SIMD type identities."""
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile

from mcp_smoke_client import McpClient


def main():
    binary, output = [Path(p).resolve() for p in sys.argv[1:]]
    if platform.machine() not in ("arm64", "aarch64"):
        raise ValueError("this compiler/runtime fixture requires an aarch64 host")
    fixture = Path(__file__).resolve().parents[1] / "tests/fixtures/code-intelligence/rust-stdarch-types"
    labels = [("src/lib.rs", "Bytes::value", "self.0.get", "src/generic.rs", "get", 4),
              ("src/lib.rs", "Words::value", "self.0.get", "src/generic.rs", "get", 4),
              ("src/lib.rs", "selected", "<ByteVector as generic::Value>::value", "src/implementations.rs", "value", 4)]
    selected = []
    with tempfile.TemporaryDirectory(prefix="rag-stdarch-types-") as temporary:
        root = Path(temporary).resolve()
        source = root / "source"
        shutil.copytree(fixture, source)
        manifest = source / "Cargo.toml"
        env = {k: v for k, v in os.environ.items() if k not in
               ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_TARGET_DIR")}
        env["CARGO_TARGET_DIR"] = str(root / "cargo-target")
        command = ["cargo", "test", "--offline", "--manifest-path", str(manifest)]
        compiled = subprocess.run(command, env=env, capture_output=True, text=True)
        assert compiled.returncode == 0 and "1 passed" in compiled.stdout, compiled.stdout + compiled.stderr
        with McpClient(binary, root, source) as client:
            for path in [manifest, *sorted((source / "src").glob("*.rs"))]:
                client.call("ingest_file", dict(path=str(path), wing="fixture"))
            indexed = client.call("index_code", {})
            symbols = client.call("symbol", dict(project="fixture", limit=500))
            assert not symbols["truncated"]
            by_id = {s["id"]: s for s in symbols["items"]}
            for path, caller, expression, target_path, name, line in labels:
                response = client.call("callees", dict(project="fixture", path=str(source / path), symbol=caller, limit=500))
                assert not response["truncated"] and len(response["items"]) == 1, response
                edge = response["items"][0]
                assert edge["reference"]["expression"] == expression and len(edge["candidates"]) == 1, edge
                target = by_id[edge["candidates"][0]]
                assert target["name"] == name and target["start_line"] == line and Path(target["path"]) == source / target_path, target
                selected.append(dict(caller=caller, expected_path=target_path, expected_line=line, edge=edge, target=target))
            warm = client.call("index_code", {})
            assert not warm["revision_created"] and warm["indexed"] == 0, warm
            status = client.call("index_status", dict(project="fixture"))
    result = dict(status="passed", binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                  index_format=status["versions"]["index_format"], selected=selected, indexed=indexed, warm=warm,
                  fixture_sha256={str(p.relative_to(fixture)): hashlib.sha256(p.read_bytes()).hexdigest()
                                  for p in fixture.rglob("*") if p.is_file()},
                  compiler=dict(command=command, exit_code=compiled.returncode, stdout=compiled.stdout, stderr=compiled.stderr),
                  limitations=["catalog identity and captured local impls; no intrinsic bodies or full compiler/ISA verification",
                               "compiler/runtime execution only on this aarch64 fixture, not every catalog type/target"])
    with output.open("x") as destination:
        json.dump(result, destination, indent=2)
        destination.write("\n")
    print(json.dumps(dict(status="passed", targets=len(selected), executed_tests=1)))


if __name__ == "__main__":
    main()
