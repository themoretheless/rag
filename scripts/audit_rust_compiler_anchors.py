#!/usr/bin/env python3
"""Supplement SCIP direct-call labels with independently compiled HIR anchors.

This is an admission audit, not name/target verification. Missing anchors remain
unverified; HIR may encode valid calls with other spans. No rag parser is used.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib
from qualify_rust_corpus import metrics


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    source, result, output = [Path(p).resolve() for p in sys.argv[1:4]]
    output.mkdir(parents=True, exist_ok=False)
    report = json.loads((result / "report.json").read_text())
    cases = json.loads((result / "scored-cases.json").read_text())["cases"]
    corpus = json.loads((source.parent / "manifest.json").read_text())
    config = tomllib.loads((source / "Cargo.toml").read_text())["package"]["metadata"]["rag"]["graph"]
    def verify_sources():
        assert all(digest(source / p) == sha for p, sha in report["input_sha256"].items()), "source bytes differ from scored run"
    verify_sources()
    env = {k: v for k, v in os.environ.items() if k not in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_TARGET_DIR")}
    env["CARGO_TARGET_DIR"] = str(output / "cargo-target")
    compiler = subprocess.run(["rustc", "-vV"], cwd=source, capture_output=True, text=True, env=env, check=True).stdout
    assert compiler == corpus["rustc"], "compiler differs from corpus configuration"
    command = ["cargo", "rustc", "--offline", "--manifest-path", str(source / "Cargo.toml"), "--lib", "--target", config["target_triple"]]
    if not config["default_features"]:
        command.append("--no-default-features")
    if config["features"]:
        command += ["--features", ",".join(config["features"])]
    command += ["--", "-Zunpretty=hir-tree"]
    compiled = subprocess.run(command, cwd=source, env=env, capture_output=True, text=True)
    (output / "compiler.log").write_text(compiled.stderr)
    assert compiled.returncode == 0, compiled.stderr[-3000:]
    (output / "compiler.hir.txt").write_text(compiled.stdout)
    verify_sources()
    spans = {}
    lines = {}
    for match in re.finditer(r"^\s*span: (src/[^:\n]+):(\d+):(\d+): (\d+):(\d+) \(#0\),?$", compiled.stdout, re.M):
        relative, line, column, end_line, end_column = match.groups()
        line, column, end_line, end_column = map(int, [line, column, end_line, end_column])
        if line != end_line:
            continue
        assert relative in report["input_sha256"], "HIR references uncaptured source"
        if relative not in lines:
            lines[relative] = (source / relative).read_text().splitlines()
        text = lines[relative][line - 1]
        # rustc diagnostics use character columns; reject non-ASCII source lines
        # instead of silently comparing those columns to SCIP UTF-8 byte offsets.
        if not text.isascii():
            continue
        token = text[column - 1:end_column - 1]
        if re.fullmatch(r"(?:r#)?[A-Za-z_][A-Za-z_0-9]*(?:::(?:r#)?[A-Za-z_][A-Za-z_0-9]*)*", token):
            spans.setdefault(relative, set()).add((line - 1, column - 1, end_column - 1))
    audited = []
    admitted = []
    for case in cases:
        if case["syntax_class"] != "direct_call":
            continue
        line, column, end_line, end_column = case["range"]
        anchors = [list(span) for span in sorted(spans.get(case["path"], set()))
                   if span[0] == line == end_line and span[1] <= column and span[2] == end_column]
        if anchors:
            admitted.append(case)
        audited.append(dict(path=case["path"], range=case["range"], compiler_path_anchors=anchors,
                            status="corroborated_source_anchor" if anchors else "unverified_anchor",
                            true_positive=case["true_positive"], false_positive=case["false_positive"], false_negative=case["false_negative"]))
    evidence = dict(status="measured", command=command, compiler=compiler, compiler_exit=compiled.returncode,
                    hir_sha256=digest(output / "compiler.hir.txt"), input_sha256=report["input_sha256"],
                    scored_cases_sha256=digest(result / "scored-cases.json"), rag_binary_sha256=report["binary_sha256"],
                    direct_call_sites=len(audited), corroborated_sites=len(admitted), unverified_sites=len(audited)-len(admitted),
                    corroborated_subset_metrics=metrics(admitted), cases=audited,
                    limitations=["exact lexical HIR path anchors only; methods/macros/non-ASCII may be unverified",
                                 "missing HIR anchor does not prove inactive or invalid code",
                                 "source activity corroboration only; target labels still come from SCIP",
                                 "nightly HIR debug format tied to the recorded compiler"])
    (output / "audit.json").write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps({k: evidence[k] for k in ["direct_call_sites", "corroborated_sites", "unverified_sites", "corroborated_subset_metrics"]}))


if __name__ == "__main__":
    main()
