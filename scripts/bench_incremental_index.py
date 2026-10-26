#!/usr/bin/env python3
"""Isolated, reproducible Cargo index workloads. Never opens production DuckDB."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
from mcp_smoke_client import McpClient


def create_fixture(source, modules, functions):
    marker = source / ".rag-benchmark-fixture"
    if source.exists() and any(source.iterdir()) and not marker.exists():
        raise ValueError("refuse to overwrite a directory not owned by this benchmark")
    source.mkdir(parents=True, exist_ok=True)
    marker.write_text("owned synthetic benchmark fixture\n")
    (source / "Cargo.toml").write_text('[workspace]\nmembers=["core","app","noise"]\nresolver="2"\n')
    for package in ("core", "app", "noise"):
        root = source / package
        (root / "src").mkdir(parents=True, exist_ok=True)
        manifest = f'[package]\nname="bench-{package}"\nversion="0.1.0"\nedition="2021"\n'
        if package == "app":
            manifest += '[dependencies]\ncore_alias={package="bench-core",path="../core"}\n'
        (root / "Cargo.toml").write_text(manifest)
        (root / "src/lib.rs").write_text("".join(f"pub mod m{i};\n" for i in range(modules)))
        for i in range(modules):
            body = "".join(f'pub fn f{j}()->i32{{' + (f'core_alias::m{i}::f{j}()' if package == "app" else "0") + '}\n' for j in range(functions))
            (root / f"src/m{i}.rs").write_text(body)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("report", type=Path)
    parser.add_argument("--modules", type=int, default=24)
    parser.add_argument("--functions", type=int, default=12)
    parser.add_argument("--source", type=Path)
    parser.add_argument("--force-full", action="store_true")
    parser.add_argument("--check-fixture", action="store_true")
    parser.add_argument("--verify", action="store_true", help="full-equivalence audit outside timing; not for paired timing controls")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="rag-index-bench-") as temporary:
        root = Path(temporary).resolve()
        source = args.source.resolve() if args.source else root / "sources"
        create_fixture(source, args.modules, args.functions)
        input_hash = hashlib.sha256(b"".join(str(p.relative_to(source)).encode() + p.read_bytes() for p in sorted(source.rglob("*")) if p.is_file() and (p.suffix == ".rs" or p.name == "Cargo.toml"))).hexdigest()
        compiler = []
        def check(label):
            if args.check_fixture:
                run = subprocess.run(["cargo", "check", "--manifest-path", str(source / "Cargo.toml"), "--workspace", "--offline", "--target-dir", str(root / "compiler-target")], capture_output=True, text=True)
                assert run.returncode == 0, run.stderr
                compiler.append(dict(case=label, exit_code=run.returncode))
        with McpClient(args.binary.resolve(), root, source) as client:
            records = []
            equivalence = []
            def verify(label):
                if args.verify:
                    result = client.call("index_code", dict(force_full_resolution=True))
                    assert result.get("resolution", {}).get("equivalent_to_existing") is True, result
                    equivalence.append(dict(case=label, result=result))
            def measure(label, operation):
                stop = threading.Event()
                rss = []
                def sample():
                    while not stop.is_set():
                        p = subprocess.run(["ps", "-o", "rss=", "-p", str(client.process.pid)], capture_output=True, text=True)
                        if p.returncode == 0 and p.stdout.strip().isdigit():
                            rss.append(int(p.stdout.strip()) * 1024)
                        stop.wait(.05)
                sampler = threading.Thread(target=sample)
                sampler.start()
                start = time.perf_counter()
                try:
                    result = operation()
                finally:
                    elapsed = (time.perf_counter() - start) * 1000
                    stop.set()
                    sampler.join()
                records.append(dict(case=label, mcp_round_trip_ms=elapsed, sampled_process_peak_rss_bytes=max(rss) if rss else None,
                                    rss_samples=len(rss), rss_sample_period_seconds=.05, result=result))
                return result
            check("cold")
            measure("cold_sync", lambda: client.call("sync_sources", dict(path=str(source), wing="bench")))
            verify("cold")
            options = dict(force_full_resolution=True) if args.force_full else {}
            measure("warm_noop", lambda: client.call("index_code", options))
            core = source / "core/src/m0.rs"
            core.write_text(core.read_text().replace("{0}", "{1}", 1))
            client.call("ingest_file", dict(path=str(core), wing="bench"))
            measure("body", lambda: client.call("index_code", options))
            verify("body")
            check("body")
            core.write_text(core.read_text().replace("f0()->i32", "f0()->u32", 1))
            app = source / "app/src/m0.rs"
            app.write_text(app.read_text().replace("core_alias::m0::f0()", "core_alias::m0::f0() as i32", 1))
            for path in (core, app):
                client.call("ingest_file", dict(path=str(path), wing="bench"))
            measure("signature", lambda: client.call("index_code", options))
            verify("signature")
            check("signature")
            manifest = source / "app/Cargo.toml"
            manifest.write_text(manifest.read_text() + '[package.metadata.benchmark]\nnonce=1\n')
            client.call("ingest_file", dict(path=str(manifest), wing="bench"))
            measure("manifest", lambda: client.call("index_code", options))
            verify("manifest")
            check("manifest")
            evidence = dict(binary_sha256=hashlib.sha256(args.binary.read_bytes()).hexdigest(), fixture_sha256=input_hash,
                            modules=args.modules, functions_per_module=args.functions, force_full=args.force_full,
                            records=records, compiler_checks=compiler, full_equivalence_audits=equivalence, production_database_touched=False,
                            limitations=["synthetic Cargo workspace", "sampled RSS is not exact operation peak", "cold_sync includes ingest/FTS; index-only phases exclude ingest", "no controlled speed claim from one run"])
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(evidence, indent=2) + "\n")
            print(json.dumps({"report": str(args.report), "timings": [{"case": r["case"], "ms": r["mcp_round_trip_ms"]} for r in records]}))


if __name__ == "__main__":
    main()
