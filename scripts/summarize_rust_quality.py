#!/usr/bin/env python3
"""Compare admitted native/MCP corpus results with identical input and labels."""
import hashlib
import json
from pathlib import Path
import sys
from qualify_rust_corpus import metrics


def summarize(root):
    repositories = {}
    combined = {phase: [] for phase in ("before", "after")}
    binaries = {phase: set() for phase in combined}
    captures = {phase: set() for phase in combined}
    for name in ("itoa", "ryu", "memchr"):
        runs = {}
        for phase in combined:
            folder = root / f"controlled-{name}-{phase}"
            report = json.loads((folder / "report.json").read_text())
            scored = json.loads((folder / "scored-cases.json").read_text())
            oracle = json.loads((folder / "oracle-labels.json").read_text())
            assert report["status"] == "measured" and report["labelled_native_mcp_parity"]
            assert report["parity_sites"] == len(scored["cases"]) == len(oracle["cases"])
            assert report["metrics"] == metrics(scored["cases"])
            assert report["label_cases_sha256"] == hashlib.sha256(json.dumps(oracle["cases"], sort_keys=True).encode()).hexdigest()
            assert [{k: c[k] for k in oracle["cases"][i]} for i, c in enumerate(scored["cases"])] == oracle["cases"]
            binaries[phase].add(report["binary_sha256"])
            captures[phase].add(report["native_capture_sha256"])
            combined[phase].extend(scored["cases"])
            runs[phase] = report
        for control in ("input_sha256", "label_cases_sha256", "oracle_sha256"):
            assert runs["before"][control] == runs["after"][control], (name, control)
        repositories[name] = {phase: dict(metrics=r["metrics"], metrics_by_syntax=r["metrics_by_syntax"],
                                         binary_sha256=r["binary_sha256"], native_capture_sha256=r["native_capture_sha256"])
                              for phase, r in runs.items()}
    for phase in combined:
        assert len(binaries[phase]) == len(captures[phase]) == 1, "mixed binaries within phase"
    return dict(contract_version=1, identical_inputs_and_labels=True, native_mcp_parity=True,
                repositories=repositories,
                combined={phase: dict(metrics=metrics(cases), metrics_by_syntax={kind: metrics([c for c in cases if c["syntax_class"] == kind])
                           for kind in sorted({c["syntax_class"] for c in cases})}) for phase, cases in combined.items()},
                limitations=["language-server source-definition correspondence, not runtime or compiler proof",
                             "small library corpus; no representative whole-program precision/recall claim",
                             "same-source quality comparison only; performance and competitors unqualified"])


if __name__ == "__main__":
    result = summarize(Path(sys.argv[1]))
    with Path(sys.argv[2]).open("x") as output:
        output.write(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result["combined"]))
