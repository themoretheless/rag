#!/usr/bin/env python3
"""Actual MCP family inventory with a pinned pure-function reference oracle."""
import ast
import hashlib
import json
import posixpath
from pathlib import Path
import re
import sys
import tempfile
from types import SimpleNamespace
from smoke_file_imports import StartupClient


def oracle(reference, texts):
    namespace = dict(re=re, posixpath=posixpath, FileInfo=object)
    util_namespace = dict(Path=Path)
    util_path = reference / "src/ragraf/util.py"
    analysis_path = reference / "src/ragraf/analysis.py"
    # Extract only these pinned pure definitions; no repository initializer or ingested code executes.
    util_tree = ast.parse(util_path.read_text())
    exec(compile(ast.Module(body=[n for n in util_tree.body if isinstance(n, ast.FunctionDef) and n.name == "to_posix"], type_ignores=[]), str(util_path), "exec"), util_namespace)
    namespace["util"] = SimpleNamespace(to_posix=util_namespace["to_posix"])
    tree = ast.parse(analysis_path.read_text())
    names = {"_norm", "family_key", "family_groups", "exact_groups"}
    exec(compile(ast.Module(body=[n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name in names], type_ignores=[]), str(analysis_path), "exec"), namespace)
    files = {p: SimpleNamespace(sha256=hashlib.sha256(b.encode()).hexdigest()) for p, b in texts.items()}
    exact = namespace["exact_groups"](files)
    families = namespace["family_groups"](files)
    exact_sets = {tuple(v) for v in exact.values()}
    return {tuple(sorted(v)) for v in exact.values()} | {tuple(sorted(v)) for v in families.values() if tuple(v) not in exact_sets}


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp"
    reference = Path(sys.argv[3]) if len(sys.argv) > 3 else Path("/tmp/ragraf-compare-20261023")
    with tempfile.TemporaryDirectory(prefix="rag-duplicate-families-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        fixtures = {
            "alpha/Files.txt": "PRIVATE_ALPHA\n",
            "alpha/Files2.txt": "PRIVATE_ALPHA\n",
            "alpha/Files3.txt": "PRIVATE_OMEGA\n",
            "alpha/clone.txt": "PRIVATE_ALPHA\n",
            "alpha/Other.txt": "PRIVATE_DELTA\n",
            "alpha/Other2.txt": "PRIVATE_EPSILON\n",
            "alpha/Other3.txt": "PRIVATE_ZETA\n",
            "alpha/nested/Other4.txt": "PRIVATE_NESTED\n",
            "alpha/other5.txt": "PRIVATE_CASE\n",
            "alpha/Exact.txt": "PRIVATE_EXACT\n",
            "alpha/Exact2.txt": "PRIVATE_EXACT\n",
            "alpha/import_stdin.txt": "PRIVATE_DISTINCT\n",
            "beta/Other4.txt": "PRIVATE_FOREIGN\n",
        }
        for name, body in fixtures.items():
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
        expected = oracle(reference, {str(source / p): b for p, b in fixtures.items() if p.startswith("alpha/")})
        with StartupClient(binary, root, source) as mcp:
            for project in ("alpha", "beta"):
                result = mcp.call("sync_sources", dict(path=str(source / project), wing=project))
                assert not result.get("errors"), result
            args = dict(project="alpha", grouping="exact_and_name_families", literal_mentions=dict(enabled=False))
            result = mcp.call("duplicates", args)
            observed = {tuple(sorted(m["document"]["path"] for m in g["members"])) for g in result["groups"]}
            assert observed == expected and result["groups_total"] == 4, result
            assert result["comparisons"] == 0 and not result["pairwise_token_grouping_performed"], result
            assert all(g["kind"] == "exact_stored_text" for g in result["groups"][:2]), result
            def family(report, filename):
                return next(g for g in report["groups"] if g["family_key"] and g["family_key"].endswith("/" + filename))
            assert family(result, "Files.txt")["kind"] == "family", result
            assert family(result, "Other.txt")["kind"] == "same_name", result
            assert family(result, "Other.txt")["family_classification"]["complete"], result
            assert "PRIVATE_" not in json.dumps(result), result
            assert result == mcp.call("duplicates", args)
            default = mcp.call("duplicates", dict(project="alpha", literal_mentions=dict(enabled=False)))
            assert all(m["document"]["path"] != str(source / "alpha/Other.txt") for g in default["groups"] for m in g["members"]), default
            for options in [dict(max_members=1), dict(primary_comparison=dict(enabled=False)), dict(primary_comparison=dict(max_file_bytes=1)), dict(primary_comparison=dict(max_pairs=1))]:
                limited = mcp.call("duplicates", dict(args, **options))
                group = family(limited, "Other.txt")
                assert group["kind"] == "not_checked" and not group["family_classification"]["complete"], limited
            page = mcp.call("duplicates", dict(args, limit=1))
            assert page["groups_total"] == 4 and page["truncated"] and len(page["groups"]) == 1, page
            scoped = mcp.call("duplicates", dict(args, path_prefix=str(source / "alpha/nested")))
            assert scoped["groups_total"] == 0, scoped
            changed = source / "alpha/Other2.txt"
            changed.write_text(fixtures["alpha/Other.txt"])
            sync = mcp.call("sync_sources", dict(path=str(source / "alpha"), wing="alpha"))
            assert not sync.get("errors"), sync
            updated = mcp.call("duplicates", args)
            assert family(updated, "Other.txt")["kind"] == "family", updated
            assert updated["groups_total"] == 5, updated
            status = mcp.call("index_status", dict(project="alpha"))
            report = dict(status="passed", versions=status["versions"], reference_commit="075caf86dc2f6274b9c48487e521915bcdfeaf2c", reference_oracle="only_five_pinned_pure_functions_on_finite_fixture", checks=["reference_group_member_sets", "independent_exact_subgroups", "identical_member_set_deduplication", "dissimilar_same_name_inventory", "directory_case_project_isolation", "numbered_filename_contract", "exact_first_order", "primary_similarity_classification", "disabled_and_budget_unknowns", "member_and_group_caps", "prefix_scope", "body_free_determinism", "sync_reclassification", "default_mode_preserved"], production_database_touched=False, source_code_executed=False, representative_quality_or_full_parity_proven=False)
            if len(sys.argv) > 2:
                Path(sys.argv[2]).write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report))


if __name__ == "__main__":
    main()
