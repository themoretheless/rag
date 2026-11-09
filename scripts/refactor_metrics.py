#!/usr/bin/env python3
"""Structural metrics for the refactor baseline (docs/refactor/BASELINE.md).

Usage:
    scripts/refactor_metrics.py              # summary
    scripts/refactor_metrics.py --long 30    # also list the 30 longest production functions
    scripts/refactor_metrics.py --modules    # per top-level module table (markdown)

A small brace-matching scanner, not a Rust parser: it skips comments, strings,
raw strings and char literals, then records every `fn` with a body. A function
is "production" unless it sits in a `#[cfg(test)]` item, carries a `#[test]`-like
attribute, or lives in a test file (`tests.rs`, `*_test.rs`, `*_tests.rs`,
`testkit.rs`, a `tests/` directory). Length is first line of `fn` to the closing
brace, inclusive. Stdlib only.
"""

from __future__ import annotations

import argparse
import re
import sys
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LONG_FN = 80
TEST_ATTR = re.compile(r"#\[\s*(cfg\(\s*(all\(\s*)?test\b|test\b|tokio::test\b|rstest\b)")
IMPL_STORE = re.compile(r"^impl(\s*<[^>]*>)?\s+Store\s*$")


def is_test_file(path: Path) -> bool:
    name = path.name
    return (
        name in ("tests.rs", "testkit.rs")
        or name.endswith("_test.rs")
        or name.endswith("_tests.rs")
        or "tests" in path.parts
    )


def strip(src: str) -> str:
    """Blank out comments and literals, keeping length and newlines."""
    out = list(src)
    i, n = 0, len(src)

    def blank(a: int, b: int) -> None:
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = src[i]
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
        elif src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif src.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            blank(i, j)
            i = j
        elif c == "r" and re.match(r'r#*"', src[i:i + 12]) and (i == 0 or not (src[i - 1].isalnum() or src[i - 1] == "_")):
            hashes = len(re.match(r"r(#*)", src[i:i + 12]).group(1))
            end = '"' + "#" * hashes
            j = src.find(end, i + 2 + hashes)
            j = n if j < 0 else j + len(end)
            blank(i, j)
            i = j
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            blank(i, min(j + 1, n))
            i = j + 1
        elif c == "'":
            m = re.match(r"'(\\.[^']*|[^'\\])'", src[i:i + 12])
            if m:
                blank(i, i + m.end())
                i += m.end()
            else:
                i += 1  # lifetime
        else:
            i += 1
    return "".join(out)


def scan(path: Path):
    """Yield (name, start_line, end_line, is_test, in_impl_store) per fn with a body."""
    raw = path.read_text(encoding="utf-8", errors="replace")
    src = strip(raw)
    file_test = is_test_file(path.relative_to(ROOT))
    line_of = [0] * (len(src) + 1)
    line = 1
    for idx, ch in enumerate(src):
        line_of[idx] = line
        if ch == "\n":
            line += 1
    line_of[len(src)] = line

    # Attribute text is read from the raw source at the same offsets.
    stack = []  # (kind, test, fn_record)
    pending_test = False
    pending = None  # ("fn", name, start) | ("impl", header_start) | None
    results = []
    token = re.compile(r"#!?\[|\b(fn|impl)\b|[{};]")
    pos = 0
    while True:
        m = token.search(src, pos)
        if not m:
            break
        t = m.group(0)
        pos = m.end()
        if t.startswith("#"):
            depth, j = 1, m.end()
            while j < len(src) and depth:
                depth += {"[": 1, "]": -1}.get(src[j], 0)
                j += 1
            if TEST_ATTR.match(raw[m.start():j].replace("#!", "#")):
                pending_test = True
            pos = j
        elif t == "fn":
            nm = re.match(r"\s+([A-Za-z_][A-Za-z0-9_]*)", src[m.end():])
            if nm and pending is None:
                pending = ("fn", nm.group(1), m.start())
        elif t == "impl":
            if pending is None:
                pending = ("impl", m.start())
        elif t == ";":
            # `fn f();` in a trait, or a statement: nothing opened.
            if pending and pending[0] == "fn":
                pending = None
            if not stack or stack[-1][0] != "fn":
                pending_test = False
        elif t == "{":
            in_test = pending_test or (stack[-1][1] if stack else file_test)
            if pending and pending[0] == "fn":
                in_store = bool(stack) and stack[-1][0] == "impl_store"
                rec = [pending[1], line_of[pending[2]], None, in_test, in_store]
                stack.append(("fn", in_test, rec))
            elif pending and pending[0] == "impl":
                header = " ".join(src[pending[1]:m.start()].split())
                kind = "impl_store" if IMPL_STORE.match(header) else "impl"
                stack.append((kind, in_test, None))
            else:
                stack.append(("block", in_test, None))
            pending = None
            pending_test = False
        elif t == "}":
            if stack:
                kind, _, rec = stack.pop()
                if kind == "fn":
                    rec[2] = line_of[m.start()]
                    results.append(tuple(rec))
    return results, raw.count("\n") + (0 if raw.endswith("\n") or not raw else 1), file_test


def top_module(rel: Path) -> str:
    parts = rel.parts
    if parts[0] == "src":
        return "src/" + (parts[1][:-3] if len(parts) == 2 else parts[1])
    if parts[0] == "crates":
        return "/".join(parts[:2])
    return parts[0]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--long", type=int, default=0, help="list the N longest production functions")
    ap.add_argument("--modules", action="store_true", help="per-module markdown table")
    ap.add_argument("--top", type=int, default=3, help="largest functions per module in --modules")
    args = ap.parse_args()

    files = sorted(
        p for base in ("src", "crates", "tests") for p in (ROOT / base).rglob("*.rs")
        if "target" not in p.parts
    )
    loc = defaultdict(int)
    prod_loc = defaultdict(int)
    fns = []  # (len, rel, name, start, module, in_store)
    store_methods = defaultdict(int)
    for path in files:
        rel = path.relative_to(ROOT)
        records, lines, file_test = scan(path)
        mod = top_module(rel)
        loc[mod] += lines
        if not file_test:
            prod_loc[mod] += lines
        for name, start, end, is_test, in_store in records:
            if is_test:
                continue
            fns.append((end - start + 1, str(rel), name, start, mod, in_store))
            if in_store:
                store_methods["db" if str(rel).startswith("src/db/") else "outside"] += 1

    fns.sort(reverse=True)
    long_fns = [f for f in fns if f[0] > LONG_FN]
    if args.modules:
        print("| module | LOC | fns >80 | largest production functions |")
        print("|---|---:|---:|---|")
        for mod in sorted(loc, key=lambda m: -loc[m]):
            mine = [f for f in fns if f[4] == mod]
            top = "; ".join(f"`{f[2]}` {f[0]} ({f[1]}:{f[3]})" for f in mine[:args.top])
            print(f"| {mod} | {loc[mod]} | {sum(1 for f in mine if f[0] > LONG_FN)} | {top} |")
        return 0

    root_loc = sum(v for k, v in loc.items() if k.startswith("src/"))
    print(f"rust LOC: src={root_loc} crates={sum(v for k, v in loc.items() if k.startswith('crates/'))} tests={loc['tests']}")
    print(f"production functions (with body): {len(fns)}")
    print(f"production functions > {LONG_FN} lines: {len(long_fns)}")
    for scope in ("src/", "crates/"):
        print(f"  under {scope}: {sum(1 for f in long_fns if f[1].startswith(scope))}")
    print(f"Store methods (fn directly in `impl Store`): db={store_methods['db']} outside={store_methods['outside']} "
          f"total={store_methods['db'] + store_methods['outside']}")
    if args.long:
        print()
        for length, rel, name, start, _, _ in fns[:args.long]:
            print(f"{length:5d}  {rel}:{start}  {name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
