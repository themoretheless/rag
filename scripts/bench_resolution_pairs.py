#!/usr/bin/env python3
"""Alternating same-binary incremental/full controls; retains every raw run."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--pairs", type=int, default=3)
    args = parser.parse_args()
    binary = args.binary.resolve()
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    args.output.mkdir(parents=True, exist_ok=False)
    runs = []
    with tempfile.TemporaryDirectory(prefix="rag-resolution-pairs-") as temporary:
        source = Path(temporary).resolve() / "sources"
        for pair in range(args.pairs):
            order = ("full", "incremental") if pair % 2 == 0 else ("incremental", "full")
            for mode in order:
                assert hashlib.sha256(binary.read_bytes()).hexdigest() == digest, "binary changed during controls"
                path = args.output / f"pair-{pair}-{mode}.json"
                cmd = [sys.executable, str(Path(__file__).with_name("bench_incremental_index.py")), str(binary), str(path), "--source", str(source)]
                if mode == "full":
                    cmd.append("--force-full")
                load_before = os.getloadavg()
                result = subprocess.run(cmd, capture_output=True, text=True)
                path.with_suffix(".log").write_text(result.stdout + result.stderr)
                assert result.returncode == 0, result.stderr
                run = json.loads(path.read_text())
                assert run["binary_sha256"] == digest
                for record in run["records"]:
                    if mode == "full" and record["case"] != "cold_sync":
                        assert record["result"]["resolution"]["forced_full"]
                runs.append(dict(pair=pair, mode=mode, path=str(path), load_before=load_before, load_after=os.getloadavg(), fixture=run["fixture_sha256"], records=run["records"]))
                print(json.dumps(dict(pair=pair, mode=mode, completed=True)), flush=True)
    assert len({r["fixture"] for r in runs}) == 1
    summary = {}
    controls_stable = True
    for case in ("cold_sync", "warm_noop", "body", "signature", "manifest"):
        groups = {mode: [next(x for x in r["records"] if x["case"] == case)["mcp_round_trip_ms"] for r in runs if r["mode"] == mode] for mode in ("full", "incremental")}
        medians = {mode: statistics.median(values) for mode, values in groups.items()}
        spreads = {mode: (max(values) - min(values)) / medians[mode] for mode, values in groups.items()}
        stable = all(spread <= .15 for spread in spreads.values())
        controls_stable &= stable
        summary[case] = dict(ms=groups, medians_ms=medians, full_over_incremental=medians["full"] / medians["incremental"], relative_ranges=spreads,
                             control_stable=stable, full_control_is_audit_not_ordinary_noop=(case == "warm_noop"))
    report = dict(status="measured", binary_sha256=digest, fixture_sha256=runs[0]["fixture"], pairs=args.pairs,
                  order=[dict(pair=r["pair"], mode=r["mode"]) for r in runs], summary=summary,
                  controls_stable=controls_stable, production_speed_qualified=False, raw_runs_retained=True,
                  limitations=["synthetic Cargo workspace", "same binary full-binding control, not competitor comparison", "warm full control deliberately recomputes bindings", "sampled RSS is not an exact peak", "three exploratory pairs do not qualify representative production speed"])
    (args.output / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(dict(summary=str(args.output / "summary.json"), controls_stable=controls_stable)))


if __name__ == "__main__":
    main()
