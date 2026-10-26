#!/usr/bin/env python3
"""Actual-MCP infrastructure paths with independent fixture labels and guards."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from mcp_smoke_client import McpClient


def main():
    binary = Path(sys.argv[1]).resolve()
    report = Path(sys.argv[2])
    fixture = Path(__file__).resolve().parents[1] / "tests/fixtures/code-intelligence/infrastructure"
    expected = json.loads((fixture / "expected.json").read_text())
    with tempfile.TemporaryDirectory(prefix="rag-infrastructure-") as temporary:
        root = Path(temporary).resolve()
        sources = root / "sources"
        primary = sources / "primary"
        foreign = sources / "foreign"
        shutil.copytree(fixture, primary)
        shutil.copytree(fixture, foreign)
        hashes = {str(p.relative_to(primary)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(primary.rglob("*")) if p.is_file()}
        compiled = subprocess.run(["cargo", "build", "--manifest-path", str(primary / "Cargo.toml"), "--release", "--bin", "api", "--offline", "--target-dir", str(root / "cargo-target")], capture_output=True, text=True)
        assert compiled.returncode == 0, compiled.stderr
        executable = root / "cargo-target/release/api"
        executed = subprocess.run([str(executable)], capture_output=True, text=True)
        assert executed.returncode == 0 and "captured infrastructure fixture" in executed.stdout
        with McpClient(binary, root, sources) as client:
            call = client.call
            sync = call("sync_sources", dict(path=str(primary), wing="infra"))
            assert not sync.get("errors"), sync
            def graph(**kwargs):
                return call("infrastructure", dict(project="infra", limit=500, **kwargs))
            initial = graph()
            assert initial == graph(), "read-only graph must be deterministic"
            def node(kind, name, result=None):
                items = [n for n in (result or initial)["nodes"] if n["kind"] == kind and n["name"] == name and n["project"] == "infra"]
                assert len(items) == 1, (kind, name, items, initial)
                return items[0]["id"]
            service = node("kubernetes_service", "public-api")
            main = node("code_symbol", "main")
            main_node = next(n for n in initial["nodes"] if n["id"] == main)
            original_symbol = call("symbol", dict(symbol=main_node["symbol_id"], project="infra"))
            assert [n["id"] for n in original_symbol["items"]] == [main_node["symbol_id"]], original_symbol
            query = {"from": service, "to": main, "max_paths": 20}
            traced = graph(**query)
            assert traced["status"] == "paths", traced
            labelled = [p for p in traced["paths"] if [n["kind"] for n in p["nodes"]] == expected["service_to_main_kinds"]]
            assert labelled, traced
            path = labelled[0]
            assert [e["relation"] for e in path["edges"]] == expected["required_relations"], path
            execution = next(e for e in path["edges"] if e["relation"] == "default_execution_candidate")
            assert execution["evidence"]["line"] == expected["execution_line"]
            binding = next(e for e in path["edges"] if e["relation"] == "references_source_file")
            docker_lines = sorted({e["line"] for e in binding["supporting_evidence"] if e["path"] == str(primary / "Dockerfile")})
            assert docker_lines == expected["supporting_docker_lines"], binding
            for edge in path["edges"]:
                assert edge["runtime_proven"] is False
                for evidence in [edge["evidence"], *edge["supporting_evidence"]]:
                    raw = Path(evidence["path"]).read_text()
                    assert 0 <= evidence["start_byte"] <= evidence["end_byte"] <= len(raw.encode())
                    assert len(raw.encode()[:evidence["start_byte"]].split(b"\n")) == evidence["line"]
                    assert evidence["content_hash"]
            wrong = node("kubernetes_workload", "wrong-namespace")
            assert not any(e["relation"] == "selector_maps_to" and e["target"] == wrong for e in initial["edges"])
            ci = node("ci_job", "image-build")
            script = node("source_file", str(primary / "scripts/check.sh"))
            assert graph(**{"from": ci, "to": script})["status"] == "paths"
            assert graph(**{**query, "max_depth": 1})["depth_limited"]
            assert graph(**{**query, "max_work": 1})["work_exhausted"]
            # The tool reads captured sources; removing a live file does not alter it.
            docker = primary / "Dockerfile"
            saved = docker.read_bytes()
            docker.unlink()
            assert graph() == initial
            docker.write_bytes(saved)
            deployment = primary / "deployment.yaml"
            deployment.write_text(deployment.read_text().replace("demo/api:v1", "demo/other:v2"))
            call("ingest_file", dict(path=str(deployment), wing="infra"))
            stale = graph(**query)
            assert stale["status"] == "no_path_in_examined_graph", stale
            assert any(d["code"] == "stale_or_unavailable_captured_input" for d in stale["diagnostics"])
            frozen = graph(**query, index_revision=initial["index_revision"])
            assert frozen["status"] == "paths" and frozen["view"] == "frozen_captured_revision"
            call("index_code", {})
            assert graph(**query)["status"] == "no_path_in_examined_graph"
            # Explicitly admitted projects add exact-image candidates; default is isolated.
            sync_foreign = call("sync_sources", dict(path=str(foreign), wing="other"))
            assert not sync_foreign.get("errors"), sync_foreign
            assert all(n["project"] == "infra" for n in graph()["nodes"])
            linked = graph(linked_projects=["other"])
            assert any(e["relation"] == "cross_project_exact_image_candidate" for e in linked["edges"])
            assert graph(**{"from": "main", "to": "main", "linked_projects": ["other"]})["status"] == "ambiguous_endpoints"
            invalid = client.request("tools/call", dict(name="infrastructure", arguments=dict(project="infra", max_depth="deep")), expect_error=True)
            assert invalid["error"]["code"] == -32602
            evidence = dict(status="passed", transport="stdio MCP", binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                            index_format=call("index_status", dict(project="infra"))["versions"]["index_format"],
                            fixture_sha256=hashes, labelled_path=path, initial_graph=initial, stale=stale, frozen=frozen,
                            linked_graph=linked, compiler=dict(exit_code=compiled.returncode, stdout=compiled.stdout, stderr=compiled.stderr),
                            compiled_fixture_run=dict(exit_code=executed.returncode, stdout=executed.stdout),
                            production_database_touched=False, docker_or_cluster_executed=False,
                            checks=["exact_eight_edge_service_to_Rust_main_path", "namespace_negative", "COPY_RUN_manifest_supporting_provenance", "ENTRYPOINT_anchor", "CI_build_and_script", "deterministic_reads", "no_live_source_dependency", "stale_bridge_exclusion", "frozen_revision", "explicit_cross_project_linking", "ambiguous_endpoints", "depth_work_limits", "typed_query"],
                            limitations=["captured static subset, not live deployment or image-content proof", "positive source binary compiled and executed on host; no Docker/cluster qualification", "fixture labels are not representative infrastructure precision/recall"])
            report.parent.mkdir(parents=True, exist_ok=True)
            report.write_text(json.dumps(evidence, indent=2) + "\n")
            print(json.dumps(dict(status="passed", report=str(report), path_edges=len(path["edges"]))))


if __name__ == "__main__":
    main()
