#!/usr/bin/env python3
"""Actual-MCP dependency cache equivalence and stale-source qualification."""
import hashlib
import json
from pathlib import Path
import sys
import tempfile
from bench_incremental_index import create_fixture
from mcp_smoke_client import McpClient


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp").resolve()
    report = Path(sys.argv[2]) if len(sys.argv) > 2 else None
    with tempfile.TemporaryDirectory(prefix="rag-resolution-cache-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        create_fixture(source, 3, 3)
        noise = source / "noise/src/m0.rs"
        noise.write_text(noise.read_text() + "pub fn extra(){f0();}\n")
        with McpClient(binary, root, source) as client:
            call = client.call
            sync = call("sync_sources", dict(path=str(source), wing="fixture"))
            assert not sync.get("errors"), sync
            cases = []
            def audit(label):
                result = call("index_code", dict(force_full_resolution=True))
                assert not result["revision_created"] and result["resolution"]["equivalent_to_existing"] is True, result
                assert result["resolution"]["documents_reused"] == 0
                cases.append(dict(case=label, full_audit=result))
            audit("cold")
            noop = call("index_code", {})
            assert noop["resolution"]["no_resolution_required"] and not noop["revision_created"]
            query = dict(path=str(source / "app/src/m0.rs"), symbol="f0")
            initial = call("callees", query)
            assert len(initial["items"]) == 1 and len(initial["items"][0]["candidates"]) == 1, initial
            core = source / "core/src/m0.rs"
            core.write_text(core.read_text().replace("{0}", "{1}", 1))
            call("ingest_file", dict(path=str(core), wing="fixture"))
            stale = call("callees", query)
            assert stale["stale_documents"] and all(not e["candidates"] for e in stale["items"]), stale
            body = call("index_code", {})
            assert body["resolution"]["documents_resolved"] == 1 and body["resolution"]["reference_sites_reused"] > 0, body
            assert body["completion"]["commit_included"] and body["completion"]["native_total_ms"] >= body["stage_timings"]["before_metrics_write_ms"]
            assert call("callees", query)["items"] == initial["items"]
            audit("body")
            cases[-1]["incremental"] = body
            core.write_text(core.read_text().replace("f0()->i32", "f0()->u32", 1))
            app = source / "app/src/m0.rs"
            app.write_text(app.read_text().replace("core_alias::m0::f0()", "core_alias::m0::f0() as i32", 1))
            for path in (core, app):
                call("ingest_file", dict(path=str(path), wing="fixture"))
            signature = call("index_code", {})
            assert signature["resolution"]["documents_resolved"] > 1 and signature["resolution"]["reference_sites_reused"] > 0, signature
            audit("signature")
            cases[-1]["incremental"] = signature
            manifest = source / "app/Cargo.toml"
            manifest.write_text(manifest.read_text() + '[package.metadata.probe]\nrevision=2\n')
            call("ingest_file", dict(path=str(manifest), wing="fixture"))
            stale_manifest = call("callees", query)
            assert stale_manifest["stale_documents"] and all(not e["candidates"] for e in stale_manifest["items"]), stale_manifest
            changed = call("index_code", {})
            assert changed["indexed"] == 1 and changed["resolution"]["reference_sites_resolved"] > 0, changed
            audit("manifest")
            cases[-1]["incremental"] = changed
            invalid = client.request("tools/call", dict(name="index_code", arguments=dict(force_full_resolution="yes")), expect_error=True)
            assert invalid["error"]["code"] == -32602
            evidence = dict(status="passed", transport="stdio MCP", binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                            index_format=call("index_status", dict(project="fixture"))["versions"]["index_format"], cases=cases,
                            production_database_touched=False, checks=["cold_full_equivalence", "unchanged_revision_skip", "body_dependency_reuse",
                                "signature_dependent_invalidation", "manifest_only_invalidation", "stale_provider_and_manifest_barriers",
                                "query_cache_evidence_identity", "post_commit_completion_metrics", "typed_force_option"],
                            limitations=["reference bindings only; other graph families recomputed", "sample fixture, not universal compiler proof", "no competitor speed claim"])
            if report:
                report.parent.mkdir(parents=True, exist_ok=True)
                report.write_text(json.dumps(evidence, indent=2) + "\n")
            print(json.dumps(dict(status="passed", cases=len(cases), report=str(report))))


if __name__ == "__main__":
    main()
