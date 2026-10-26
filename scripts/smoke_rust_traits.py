#!/usr/bin/env python3
"""Real MCP Rust trait/generic/UFCS qualification; isolated DB and fixed labels."""
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile
from mcp_smoke_client import McpClient


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp").resolve()
    report = Path(sys.argv[2]) if len(sys.argv) > 2 else None
    fixture = Path(__file__).resolve().parents[1] / "tests/fixtures/code-intelligence/rust-traits"
    labels = json.loads((fixture / "expectations.json").read_text())
    with tempfile.TemporaryDirectory(prefix="rag-rust-traits-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        shutil.copytree(fixture, source)
        with McpClient(binary, root, source) as client:
            call = client.call
            sync = call("sync_sources", dict(path=str(source), wing="fixture"))
            assert not sync.get("errors"), sync
            symbols = call("symbol", dict(limit=500))
            assert not symbols["truncated"]
            by_id = {s["id"]: s for s in symbols["items"]}
            outputs = {}
            outcomes = []
            for label in labels["calls"]:
                caller = label["caller"]
                if caller not in outputs:
                    outputs[caller] = call("callees", dict(path=str(source / "src/lib.rs"), symbol=caller, limit=500))
                result = outputs[caller]
                assert not result["truncated"], result
                edges = [e for e in result["items"] if e["reference"]["expression"] == label["expression"]]
                assert len(edges) == 1, (label, result)
                edge = edges[0]
                actual = sorted((str(Path(by_id[i]["path"]).relative_to(source)), by_id[i]["qualified_name"]) for i in edge["candidates"])
                assert actual == [(label["target_path"], label["target"])], (label, actual, edge)
                assert edge["provenance"] == "derived", edge
                if caller in ("bounded", "dynamic"):
                    assert edge["resolution_basis"] == "rust_trait_declaration", edge
                    assert all(by_id[i]["kind"] == "function_signature_item" for i in edge["candidates"]), edge
                outcomes.append(dict(**label, actual=actual, wrong_targets=0))
            before = outputs["concrete"]["index_rev"]
            noop = call("index_code", {})
            assert not noop["revision_created"] and noop["indexed"] == 0, noop
            implementation = source / "src/implementations.rs"
            implementation.write_text(implementation.read_text().replace("type Item = Other;", "type Item = User;").replace("-> Self::Item { Other }", "-> Self::Item { User }"))
            call("ingest_file", dict(path=str(implementation), wing="fixture"))
            stale = call("callees", dict(path=str(source / "src/lib.rs"), symbol="concrete", limit=500))
            assert stale["stale_documents"], stale
            for edge in stale["items"]:
                if edge["reference"]["expression"] in ("user.save", "user.render", "user.provide().save"):
                    assert edge["candidates"] == [], stale
            # Trait-bound declarations survive loss of concrete impl facts.
            bounded = call("callees", dict(path=str(source / "src/lib.rs"), symbol="bounded"))
            bounded_edge = next(e for e in bounded["items"] if e["reference"]["expression"] == "value.render")
            assert len(bounded_edge["candidates"]) == 1 and by_id[bounded_edge["candidates"][0]]["path"].endswith("/contracts.rs"), bounded
            update = call("index_code", {})
            assert update["indexed"] == 1 and update["revision_created"], update
            current_symbols = call("symbol", dict(limit=500))
            current = {s["id"]: s for s in current_symbols["items"]}
            projection = call("callees", dict(path=str(source / "src/lib.rs"), symbol="projection", limit=500))
            for expression in ["selected::<User>().save", "item.save"]:
                edge = next(e for e in projection["items"] if e["reference"]["expression"] == expression)
                assert len(edge["candidates"]) == 1 and current[edge["candidates"][0]]["qualified_name"] == "User::save", projection
            diff_args = dict(before=before, after=update["index_revision"], project="fixture")
            diff = call("diff", diff_args)
            assert diff["edges_added_total"] >= 1 and diff["edges_removed_total"] >= 1, diff
            implementation.write_text("// retained history must not consult current files\n")
            assert call("diff", diff_args) == diff
            status = call("index_status", dict(project="fixture"))
            capability = status["capabilities"]["rust_trait_typing"]
            assert capability["bound_receiver_targets"] == "trait_declarations_only" and not capability["runtime_dispatch_verified"] and not capability["compiler_verified"], capability
            with binary.open("rb") as stream:
                fingerprint = hashlib.file_digest(stream, "sha256").hexdigest()
            evidence = dict(status="passed", transport="stdio MCP", scope=labels["scope"], binary_sha256=fingerprint,
                            index_format=status["versions"]["index_format"], labelled_calls=outcomes,
                            checks=["cross_file_impl_owner", "generic_impl_return", "trait_import_alias", "explicit_ufcs",
                                    "default_method", "generic_bound_declaration", "dyn_trait_declaration", "associated_projection",
                                    "warm_reuse", "stale_impl_exclusion", "declarations_survive_stale_impl", "associated_type_only_rebinding", "frozen_history"],
                            production_database_touched=False, runtime_dispatch_verified=False,
                            representative_quality_measured=False, competitor_comparison_performed=False)
            if report:
                report.parent.mkdir(parents=True, exist_ok=True)
                report.write_text(json.dumps(evidence, indent=2) + "\n")
            print(json.dumps(evidence))


if __name__ == "__main__":
    main()
