#!/usr/bin/env python3
"""Audit unchanged corpus scores and inventory misses without guessing causes."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path

from qualify_rust_corpus import metrics


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def owned_path(root, relative):
    path = Path(relative)
    if path.is_absolute() or ".." in path.parts:
        raise ValueError(f"unsafe relative input: {relative}")
    resolved = (root / path).resolve()
    if not resolved.is_relative_to(root.resolve()):
        raise ValueError(f"source escapes root: {relative}")
    return resolved


def excerpt(root, path, span):
    lines = owned_path(root, path).read_bytes().splitlines()
    row, column, end_row, end_column = span
    if not (0 <= row <= end_row < len(lines)):
        raise ValueError("source row outside captured input")
    if not (0 <= column <= len(lines[row]) and 0 <= end_column <= len(lines[end_row])):
        raise ValueError("source column outside captured input")
    if row == end_row and column > end_column:
        raise ValueError("reversed source span")
    token = (lines[row][column:end_column] if row == end_row else
             b"\n".join([lines[row][column:], *lines[row + 1:end_row], lines[end_row][:end_column]]))
    return dict(token=token.decode("utf-8"), line=row + 1,
                context=[dict(line=i + 1, text=lines[i].decode("utf-8"))
                         for i in range(max(0, row - 3), min(len(lines), end_row + 4))])


def category(case):
    if not case["observations"]:
        return "no_observation"
    bases = {o["basis"] for o in case["observations"]}
    if bases == {"rust_macro_expansion_unverified"}:
        return "macro_expansion_unverified"
    if bases == {"rust_ast_receiver_type"}:
        return "receiver_unresolved"
    return "other_resolution"


def audit_run(folder, source):
    report = json.loads((folder / "report.json").read_text())
    oracle = json.loads((folder / "oracle-labels.json").read_text())
    scored = json.loads((folder / "scored-cases.json").read_text())
    if report["status"] != "measured" or not report["labelled_native_mcp_parity"]:
        raise ValueError("unqualified corpus run")
    for relative, expected in report["input_sha256"].items():
        if digest(owned_path(source, relative)) != expected:
            raise ValueError(f"changed corpus input: {relative}")
    labels = oracle["cases"]
    cases = scored["cases"]
    label_hash = hashlib.sha256(json.dumps(labels, sort_keys=True).encode()).hexdigest()
    if label_hash != report["label_cases_sha256"] or len(labels) != len(cases):
        raise ValueError("changed oracle labels")
    if report["parity_sites"] != len(cases):
        raise ValueError("incomplete parity scope")
    seen = set()
    misses = []
    for label, case in zip(labels, cases):
        if any(case.get(key) != value for key, value in label.items()):
            raise ValueError("scored case differs from oracle")
        identity = (case["path"], tuple(case["range"]), case["symbol"])
        if identity in seen:
            raise ValueError("duplicate labelled case")
        seen.add(identity)
        expected = {(t["path"], *t["range"][:2]) for t in case["expected"]}
        observed = {tuple(t) for t in case["observed_definitions"]}
        recomputed = dict(true_positive=len(expected & observed),
                          false_positive=len(observed - expected),
                          false_negative=len(expected - observed), exact=expected == observed)
        if any(case[key] != value for key, value in recomputed.items()):
            raise ValueError("invalid per-case score")
        if case["false_negative"]:
            paths = [case["path"], *(t["path"] for t in case["expected"])]
            if any(path not in report["input_sha256"] for path in paths):
                raise ValueError("miss evidence outside hashed inputs")
            misses.append(dict(**case, category=category(case),
                               source=excerpt(source, case["path"], case["range"]),
                               targets=[dict(**t, source=excerpt(source, t["path"], t["range"]))
                                        for t in case["expected"]]))
    measured = metrics(cases)
    if measured != report["metrics"] or measured != scored["metrics"]:
        raise ValueError("aggregate scores differ")
    return dict(metrics=measured, misses=misses,
                groups=dict(sorted(Counter(c["category"] for c in misses).items())),
                artifacts={name: digest(folder / name) for name in
                           ("report.json", "oracle-labels.json", "scored-cases.json")})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("qualification", type=Path)
    parser.add_argument("sources", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    repositories = {name: audit_run(args.qualification / f"controlled-{name}-after", args.sources / name)
                    for name in ("itoa", "ryu", "memchr")}
    groups = Counter()
    for run in repositories.values():
        groups.update(run["groups"])
    result = dict(contract_version=1, repositories=repositories, groups=dict(sorted(groups.items())),
                  missed_sites=sum(len(r["misses"]) for r in repositories.values()),
                  limitations=["categories describe captured observations, not established root causes",
                               "cfg-disabled labels remain in the original denominator",
                               "source-definition correspondence, not runtime dispatch proof"])
    with args.output.open("x") as output:
        json.dump(result, output, indent=2)
        output.write("\n")
    print(json.dumps(dict(missed_sites=result["missed_sites"], groups=result["groups"])))


if __name__ == "__main__":
    main()
