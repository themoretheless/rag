#!/usr/bin/env python3
"""Qualify real MCP receiver typing against independently labelled Rust calls."""
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile
from mcp_smoke_client import McpClient


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp").resolve()
    report_path = Path(sys.argv[2]) if len(sys.argv) > 2 else None
    fixture = Path(__file__).resolve().parents[1] / "tests/fixtures/code-intelligence/rust-receivers"
    labels = json.loads((fixture / "expectations.json").read_text())
    with tempfile.TemporaryDirectory(prefix="rag-rust-receivers-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        shutil.copytree(fixture, source)
        with McpClient(binary, root, source) as client:
            call = client.call
            synced = call("sync_sources", dict(path=str(source), wing="fixture"))
            assert not synced.get("errors"), synced
            args = dict(path=str(source / labels["path"]), symbol=labels["caller"], limit=500)
            result = call("callees", args)
            symbols = call("symbol", dict(limit=500))
            assert not symbols["truncated"] and not result["truncated"]
            by_id = {s["id"]: s for s in symbols["items"]}
            outcomes = []
            for label in labels["calls"]:
                matches = [e for e in result["items"] if e["reference"]["expression"] == label["expression"]]
                assert len(matches) == 1, (label, result)
                edge = matches[0]
                actual = sorted((str(Path(by_id[i]["path"]).relative_to(source)), by_id[i]["qualified_name"]) for i in edge["candidates"])
                expected = [(label["target_path"], label["target"])]
                assert actual == expected, (label, actual, edge)
                assert edge["provenance"] == "derived" and edge["resolution_basis"] == "rust_ast_receiver_type", edge
                outcomes.append(dict(**label, actual=actual, wrong_targets=0))
            # Chain calls share a starting byte, but must retain distinct callee facts.
            for expression, expected in [("service.user", "Service::user"), ("service.other", "Service::other"),
                                         ("service.user_async", "Service::user_async")]:
                edges = [e for e in result["items"] if e["reference"]["expression"] == expression]
                assert len(edges) == 1 and len(edges[0]["candidates"]) == 1, (expression, edges)
                assert by_id[edges[0]["candidates"][0]]["qualified_name"] == expected
            before = result["index_rev"]
            noop = call("index_code", {})
            assert not noop["revision_created"] and noop["indexed"] == 0, noop
            provider = source / "src/remote.rs"
            provider.write_text(provider.read_text() + "\npub struct Other; impl Other {pub fn save(&self){}}\n")
            provider.write_text(provider.read_text().replace("factory() -> User { User }", "factory() -> Other { Other }"))
            call("ingest_file", dict(path=str(provider), wing="fixture"))
            stale = call("callees", args)
            remote = [e for e in stale["items"] if e["reference"]["expression"] == "remote::factory().save"]
            assert len(remote) == 1 and remote[0]["candidates"] == [] and stale["stale_documents"], stale
            update = call("index_code", {})
            assert update["indexed"] == 1 and update["revision_created"], update
            new_symbols = call("symbol", dict(limit=500))
            new_by_id = {s["id"]: s for s in new_symbols["items"]}
            rebound = call("callees", args)
            remote = [e for e in rebound["items"] if e["reference"]["expression"] == "remote::factory().save"]
            assert len(remote) == 1 and len(remote[0]["candidates"]) == 1, rebound
            target = new_by_id[remote[0]["candidates"][0]]
            assert target["qualified_name"] == "Other::save" and target["path"] == str(provider), target
            diff_args = dict(before=before, after=update["index_revision"], project="fixture")
            diff = call("diff", diff_args)
            assert diff["edges_added_total"] >= 1 and diff["edges_removed_total"] >= 1, diff
            provider.write_text("// live files cannot rewrite the retained graph\n")
            assert call("diff", diff_args) == diff
            status = call("index_status", dict(project="fixture"))
            with binary.open("rb") as stream:
                fingerprint = hashlib.file_digest(stream, "sha256").hexdigest()
            evidence = dict(status="passed", transport="stdio MCP", scope=labels["scope"], binary_sha256=fingerprint,
                            index_format=status["versions"]["index_format"], labelled_calls=outcomes,
                            checks=["exact_owner_and_path", "nested_call_identity", "await", "explicit_generic_return", "field_types",
                                    "array_and_vec_elements", "standard_wrappers", "local_factory", "foreign_return_type_context",
                                    "warm_reuse", "stale_provider_exclusion", "return_signature_only_rebinding", "frozen_history"],
                            production_database_touched=False, compiler_verified_by_runner=False,
                            representative_quality_measured=False, competitor_comparison_performed=False)
            if report_path:
                report_path.parent.mkdir(parents=True, exist_ok=True)
                report_path.write_text(json.dumps(evidence, indent=2) + "\n")
            print(json.dumps(evidence))


if __name__ == "__main__":
    main()
