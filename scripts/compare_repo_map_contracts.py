#!/usr/bin/env python3
"""Run the pinned reference repo_map method against owned SQLite fixtures.

Only the named method and payload-size helper are compiled from the reference;
no reference package initialization, service, network, or user DB is used.
This tests two controlled contracts, not whole-repository quality/performance.
Usage: python3 scripts/compare_repo_map_contracts.py REFERENCE_CHECKOUT [report.json]
"""
import __future__
import ast
import hashlib
import json
from pathlib import Path
import sqlite3
import subprocess
import sys
from types import SimpleNamespace


def main():
    checkout = Path(sys.argv[1]).resolve()
    revision = subprocess.check_output(["git", "-C", str(checkout), "rev-parse", "HEAD"], text=True).strip()
    assert revision == "075caf86dc2f6274b9c48487e521915bcdfeaf2c", revision
    source = subprocess.check_output(["git", "-C", str(checkout), "show", revision + ":src/ragraf/facade.py"], text=True)
    tree = ast.parse(source)
    declarations = [node for node in ast.walk(tree) if isinstance(node, ast.FunctionDef)
                    and node.name in {"op_repo_map", "_payload_weight_list"}]
    assert len(declarations) == 2
    namespace = {"util": SimpleNamespace(to_posix=lambda path: path.replace("\\", "/"))}
    module = ast.Module(body=declarations, type_ignores=[])
    exec(compile(ast.fix_missing_locations(module), "pinned_reference_methods", "exec",
                 flags=__future__.annotations.compiler_flag), namespace)

    class Envelope:
        def __init__(self):
            self.result, self.extra, self.limits = {}, {}, []

        def to_dict(self):
            return dict(result=self.result, extra=self.extra, limits=self.limits)

    def run(paths, **query):
        with sqlite3.connect(":memory:") as graph:
            graph.row_factory = sqlite3.Row
            graph.execute("CREATE TABLE node(rev_id INT, path TEXT, name TEXT, kind TEXT, start_line INT, fqn TEXT)")
            graph.executemany("INSERT INTO node VALUES(1, ?, 'work', 'function', 1, 'work')", [(p,) for p in paths])
            facade = SimpleNamespace(graph=graph, make_env=lambda _: Envelope(), current_rev=lambda: 1,
                                     _centrality=lambda _: {"scores": {}})
            return namespace["op_repo_map"](facade, **query)

    long_first = run(["/p/" + "x" * 800 + ".py", "/p/short.py"], budget_tokens=1)
    boundary = run(["/src/lib-copy/outside.py"], scope="/src/lib")
    evidence = dict(reference_revision=revision, fixture_adapter="owned in-memory SQLite; centrality unavailable fallback",
                    extracted_source_sha256=hashlib.sha256("\n".join(ast.get_source_segment(source, d) for d in declarations).encode()).hexdigest(),
                    first_entry_over_budget=dict(chars_used=long_first["result"]["chars_used"],
                        budget_chars=long_first["result"]["budget_chars"], budget_ok=long_first["extra"]["budget_ok"],
                        files_included=long_first["result"]["files_included"]),
                    component_prefix_false_positive=bool(boundary["result"]["files"]),
                    scope="controlled reference rendering/filter contracts only; no representative ranking or performance claim")
    assert evidence["first_entry_over_budget"]["chars_used"] > evidence["first_entry_over_budget"]["budget_chars"]
    assert evidence["component_prefix_false_positive"]
    if len(sys.argv) > 2:
        output = Path(sys.argv[2])
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(evidence, indent=2) + "\n")
    print(json.dumps(evidence))


if __name__ == "__main__":
    main()
