#!/usr/bin/env python3
"""Compiler-valid generic pointer calls through the actual MCP transport."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from mcp_smoke_client import McpClient


SOURCE = """pub trait Mark { fn mark(self) -> usize; }
impl<T> Mark for *const T { fn mark(self) -> usize { 11 } }
impl<T> Mark for *mut T { fn mark(self) -> usize { 22 } }
pub fn constant<T>(p: *const T) -> usize { p.mark() }
pub fn mutable<U>(q: *mut U) -> usize { q.mark() }
pub fn cast<V>(r: *mut V) -> usize { (r as *const V).mark() }
#[test] fn values() {
    let mut value = 0_u8;
    let p = &mut value as *mut u8;
    assert_eq!([constant(p), mutable(p), cast(p)], [11, 22, 11]);
}
"""


def main():
    binary, output = map(lambda p: Path(p).resolve(), sys.argv[1:])
    with tempfile.TemporaryDirectory(prefix="rag-generic-parameters-") as directory:
        root = Path(directory).resolve()
        source = root / "source"
        (source / "src").mkdir(parents=True)
        manifest = source / "Cargo.toml"
        manifest.write_text('[package]\nname="generic-parameters"\nversion="0.1.0"\nedition="2021"\n')
        rust = source / "src/lib.rs"
        rust.write_text(SOURCE)
        env = {k: v for k, v in os.environ.items() if k not in
               ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_TARGET_DIR")}
        env["CARGO_TARGET_DIR"] = str(root / "cargo-target")
        command = ["cargo", "test", "--offline", "--manifest-path", str(manifest)]
        compiled = subprocess.run(command, env=env, capture_output=True, text=True)
        assert compiled.returncode == 0 and "1 passed" in compiled.stdout, compiled.stdout + compiled.stderr
        negative_controls = []
        for name, declaration in [
                ("inline", "fn run<T: ?Sized>(p: *const T) { p.mark(); }"),
                ("where", "fn run<T>(p: *const T) where T: ?Sized { p.mark(); }")]:
            control_source = "trait Mark { fn mark(self); }\nimpl<T> Mark for *const T { fn mark(self) {} }\n" + declaration
            control = root / f"negative_{name}.rs"
            control.write_text(control_source)
            arguments = ["rustc", "--edition=2021", "--crate-type=lib", "--emit=metadata",
                         str(control), "-o", str(root / f"negative_{name}.rmeta")]
            rejected = subprocess.run(arguments, env=env, capture_output=True, text=True)
            assert rejected.returncode != 0 and "Sized" in rejected.stderr and "E0599" in rejected.stderr, rejected
            negative_controls.append(dict(source=control_source, command=arguments, exit_code=rejected.returncode,
                                          stdout=rejected.stdout, stderr=rejected.stderr))
        selected = []
        with McpClient(binary, root, source) as client:
            for path in (manifest, rust):
                client.call("ingest_file", dict(path=str(path), wing="fixture"))
            indexed = client.call("index_code", {})
            symbols = client.call("symbol", dict(project="fixture", limit=500))
            assert not symbols["truncated"]
            by_id = {s["id"]: s for s in symbols["items"]}
            for caller, expression, pointer, line in [
                    ("constant", "p.mark", "*const T", 2),
                    ("mutable", "q.mark", "*mut T", 3),
                    ("cast", "(r as *const V).mark", "*const T", 2)]:
                result = client.call("callees", dict(project="fixture", path=str(rust), symbol=caller, limit=500))
                assert not result["truncated"] and len(result["items"]) == 1, result
                edge = result["items"][0]
                assert edge["reference"]["expression"] == expression and len(edge["candidates"]) == 1, edge
                target = by_id[edge["candidates"][0]]
                assert pointer in target["qualified_name"] and target["name"] == "mark", target
                assert Path(target["path"]) == rust and target["start_line"] == line, target
                selected.append(dict(caller=caller, expression=expression, expected_owner=pointer,
                                     expected_line=line, edge=edge, target=target))
            warm = client.call("index_code", {})
            assert not warm["revision_created"] and warm["indexed"] == 0, warm
            status = client.call("index_status", dict(project="fixture"))
        result = dict(status="passed", binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                      source=SOURCE, source_sha256=hashlib.sha256(SOURCE.encode()).hexdigest(),
                      selected=selected, indexed=indexed, warm=warm, index_format=status["versions"]["index_format"],
                      compiler=dict(command=command, exit_code=compiled.returncode,
                                    stdout=compiled.stdout, stderr=compiled.stderr),
                      negative_compiler_controls=negative_controls,
                      limitations=["static candidates; no runtime dispatch or general generic compiler proof"])
        with output.open("x") as destination:
            json.dump(result, destination, indent=2)
            destination.write("\n")
        print(json.dumps(dict(status="passed", targets=len(selected), executed_tests=1)))


if __name__ == "__main__":
    main()
