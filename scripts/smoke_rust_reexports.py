#!/usr/bin/env python3
"""Actual MCP and Cargo qualification for captured Rust re-export candidates."""
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
    fixture = Path(__file__).resolve().parents[1] / "tests/fixtures/code-intelligence/rust-reexports"
    labels = json.loads((fixture / "expected-call-targets.json").read_text())
    runs = []
    with tempfile.TemporaryDirectory(prefix="rag-rust-reexports-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        shutil.copytree(fixture, source)
        inputs = sorted([*source.rglob("*.toml"), *source.rglob("*.rs")])
        engine = source / "engine/Cargo.toml"
        original = engine.read_text()
        env = {k: v for k, v in os.environ.items() if k not in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_TARGET_DIR")}
        env["CARGO_TARGET_DIR"] = str(root / "cargo-target")
        query = dict(project="fixture", path=str(source / "client/src/main.rs"), symbol="main", limit=500)
        with McpClient(binary, root, source) as client:
            for path in inputs:
                client.call("ingest_file", dict(path=str(path), wing="fixture"))
            previous_revision = None
            for feature in ["fast", "slow"]:
                if feature == "slow":
                    engine.write_text(original.replace('features = ["fast"]', 'features = []'))
                    client.call("ingest_file", dict(path=str(engine), wing="fixture"))
                    stale = client.call("callees", query)
                    assert all(not edge["candidates"] for edge in stale["items"]), stale
                indexed = client.call("index_code", {})
                if previous_revision is not None:
                    assert indexed["indexed"] == 1 and indexed["unchanged"] == len(inputs) - 1, indexed
                symbols = client.call("symbol", dict(project="fixture", limit=500))
                assert not symbols["truncated"]
                by_id = {s["id"]: s for s in symbols["items"]}
                calls = client.call("callees", query)
                assert not calls["truncated"] and len(calls["items"]) == len(labels[feature]), calls
                observed = {}
                for edge in calls["items"]:
                    assert len(edge["candidates"]) == 1, edge
                    target = by_id[edge["candidates"][0]]
                    observed[edge["reference"]["expression"]] = [str(Path(target["path"]).relative_to(source)), target["qualified_name"]]
                assert observed == labels[feature], observed
                internal = client.call("callees", dict(project="fixture", path=str(source / "engine/src/lib.rs"), symbol="internal_entry", limit=500))
                assert len(internal["items"]) == 1 and len(internal["items"][0]["candidates"]) == 1, internal
                target = by_id[internal["items"][0]["candidates"][0]]
                assert target["name"] == "restricted" and Path(target["path"]) == source / "engine/src/internal.rs", target
                command = ["cargo", "test", "--offline", "--manifest-path", str(source / "Cargo.toml"), "--workspace", "--no-default-features"]
                if feature == "fast":
                    command += ["--features", "fixture-engine/fast"]
                compiled = subprocess.run(command, cwd=source, env=env, capture_output=True, text=True)
                assert compiled.returncode == 0 and "1 passed" in compiled.stdout, compiled.stdout + compiled.stderr
                runs.append(dict(feature=feature, index_report=indexed, calls=calls, expected=labels[feature], observed=observed,
                                 internal_call=internal, compiler_command=command, compiler_exit=compiled.returncode,
                                 compiler_stdout=compiled.stdout, compiler_stderr=compiled.stderr))
                if previous_revision is not None:
                    diff_args = dict(project="fixture", before=previous_revision, after=indexed["index_revision"])
                    diff = client.call("diff", diff_args)
                    assert diff["edges_added_total"] and diff["edges_removed_total"], diff
                previous_revision = indexed["index_revision"]
            main_path = source / "client/src/main.rs"
            main_path.write_text("fn main(){engine::api::restricted();}\n")
            client.call("ingest_file", dict(path=str(main_path), wing="fixture"))
            client.call("index_code", {})
            denied = client.call("callees", query)
            assert len(denied["items"]) == 1 and denied["items"][0]["reference"]["expression"] == labels["private_call"]["expression"], denied
            assert denied["items"][0]["candidates"] == labels["private_call"]["expected_candidates"], denied
            command = ["cargo", "check", "--offline", "--manifest-path", str(source / "Cargo.toml"), "--workspace", "--no-default-features", "--message-format=json"]
            rejected = subprocess.run(command, cwd=source, env=env, capture_output=True, text=True)
            diagnostics = [json.loads(line) for line in rejected.stdout.splitlines() if line.startswith("{")]
            codes = [d["message"]["code"]["code"] for d in diagnostics if d.get("reason") == "compiler-message" and d["message"].get("code")]
            assert rejected.returncode != 0 and labels["private_call"]["compiler_error"] in codes, rejected.stdout + rejected.stderr
            status = client.call("index_status", dict(project="fixture"))
    evidence = dict(status="passed", binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                    index_format=status["versions"]["index_format"], transport="actual stdio MCP", runs=runs,
                    diff=diff, denied_call=denied, rejected_compiler_command=command, compiler_errors=codes,
                    rejected_stdout=rejected.stdout, rejected_stderr=rejected.stderr, production_database_touched=False,
                    limitations=["captured Cargo-local static candidates; not full compiler namespace or runtime proof"])
    with output.open("x") as destination:
        destination.write(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps(dict(status="passed", configurations=2, labelled_positive_calls=sum(len(labels[f]) for f in ["fast", "slow"]), compiler_rejected_private_call=True)))


if __name__ == "__main__":
    main()
