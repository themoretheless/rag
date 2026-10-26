#!/usr/bin/env python3
"""Exercise captured Rust build configuration through real stdio MCP."""
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
    fixture = Path(__file__).resolve().parents[1] / "tests/fixtures/code-intelligence/rust-cfg"
    with tempfile.TemporaryDirectory(prefix="rag-rust-cfg-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        shutil.copytree(fixture, source)
        with McpClient(binary, root, source) as client:
            call = client.call
            sync = call("sync_sources", dict(path=str(source), wing="fixture"))
            assert not sync.get("errors"), sync
            symbols = call("symbol", dict(limit=500))
            assert not symbols["truncated"]
            names = {s["name"] for s in symbols["items"]}
            assert "fast" in names and "leaf" in names and "slow" not in names and "main" not in names, symbols
            by_id = {s["id"]: s for s in symbols["items"]}
            status = call("index_status", dict(project="fixture"))
            context = status["rust_build_contexts"][0]
            assert context["available"] and context["features"] == ["fast", "leaf"], context
            initial_id = context["configuration_id"]
            assert all(s["build_context"] == initial_id for s in symbols["items"])
            args = dict(path=str(source / "src/lib.rs"), symbol="run", limit=500)
            outgoing = call("callees", args)
            assert len(outgoing["items"]) == 2, outgoing
            targets = []
            for edge in outgoing["items"]:
                assert len(edge["candidates"]) == 1 and edge["reference"]["build_context"] == initial_id, edge
                targets.extend(by_id[i]["name"] for i in edge["candidates"])
            assert sorted(targets) == ["fast", "leaf"], outgoing
            fast = next(s for s in symbols["items"] if s["name"] == "fast")
            body = call("code", dict(symbol=fast["id"]))
            assert body["found"] and "pub fn fast" in body["code"], body
            before = outgoing["index_rev"]
            noop = call("index_code", {})
            assert not noop["revision_created"] and noop["indexed"] == 0, noop
            manifest = source / "Cargo.toml"
            manifest.write_text(manifest.read_text().replace('features = ["fast"]', 'features = []').replace("default_features = false", "default_features = true"))
            call("ingest_file", dict(path=str(manifest), wing="fixture"))
            stale = call("callees", args)
            assert stale["stale_documents"] and all(not e["candidates"] for e in stale["items"]), stale
            update = call("index_code", {})
            assert update["indexed"] == 1 and update["revision_created"], update
            current_symbols = call("symbol", dict(limit=500))
            assert any(s["name"] == "slow" for s in current_symbols["items"])
            assert not any(s["name"] in ("fast", "leaf", "main") for s in current_symbols["items"]), current_symbols
            current_context = call("index_status", dict(project="fixture"))["rust_build_contexts"][0]
            assert current_context["configuration_id"] != initial_id and current_context["features"] == ["default", "slow"], current_context
            outgoing = call("callees", args)
            assert len(outgoing["items"]) == 1 and outgoing["items"][0]["reference"]["expression"] == "slow" and len(outgoing["items"][0]["candidates"]) == 1, outgoing
            diff_args = dict(before=before, after=update["index_revision"], project="fixture")
            diff = call("diff", diff_args)
            assert diff["rust_build_contexts"]["available"] and diff["rust_build_contexts"]["changed"], diff
            assert diff["edges_added_total"] > 0 and diff["edges_removed_total"] > 0, diff
            assert any("::build:" in s["symbol_key"] for s in diff["symbols"]), diff
            manifest.write_text("[package]\nname='live-change'\n")
            assert call("diff", diff_args) == diff
            with binary.open("rb") as stream:
                fingerprint = hashlib.file_digest(stream, "sha256").hexdigest()
            evidence = dict(status="passed", transport="stdio MCP", binary_sha256=fingerprint, index_format=status["versions"]["index_format"],
                            scope="One explicit captured compilation target per package, local feature/cfg subset",
                            checks=["explicit_features", "local_feature_expansion", "default_features", "conditional_module", "target_exclusion",
                                    "contextual_symbol_and_reference_ids", "code_body_source_identity", "warm_reuse", "stale_manifest_blocks_calls",
                                    "manifest_only_configuration_switch", "frozen_context_and_edge_diff"],
                            initial_context=context, next_context=current_context, production_database_touched=False,
                            dependency_feature_unification_verified=False, multi_target_instantiation=False, compiler_verified_by_runner=False)
            if report:
                report.parent.mkdir(parents=True, exist_ok=True)
                report.write_text(json.dumps(evidence, indent=2) + "\n")
            print(json.dumps(evidence))


if __name__ == "__main__":
    main()
