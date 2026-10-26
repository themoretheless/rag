#!/usr/bin/env python3
"""Real-MCP symbol paths with independent expected paths and stale bridge checks."""
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
    fixture = Path(__file__).resolve().parents[1] / "tests/fixtures/code-intelligence/symbol-paths"
    labels = json.loads((fixture / "expected.json").read_text())
    with tempfile.TemporaryDirectory(prefix="rag-symbol-paths-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        shutil.copytree(fixture, source)
        with McpClient(binary, root, source) as client:
            call = client.call
            sync = call("sync_sources", dict(path=str(source), wing="paths"))
            assert not sync.get("errors"), sync
            args = dict(project="paths", **{k: labels[k] for k in ("from", "to")})
            result = call("trace_path", args)
            assert result["status"] == "found", result
            actual = [[s["name"] for s in p["symbols"]] for p in result["paths"]]
            assert sorted(actual) == sorted(labels["paths"]), result
            for path in result["paths"]:
                assert path["length"] == 3 and len(path["steps"]) == 3
                for step in path["steps"]:
                    assert step["relation"] == "call" and not step["reversed"] and not step["ambiguous_target"], step
                    ref = step["evidence"]["reference"]
                    assert ref["document_id"] == step["source"]["document_id"] and ref["revision"] == step["source"]["revision"], step
                    assert ref["content_hash"] == step["source"]["content_hash"] and ref["line"] > 0, step
            assert result == call("trace_path", args)
            invalid = client.request("tools/call", dict(name="trace_path", arguments=dict(args, relations="guessed")), expect_error=True)
            assert invalid["error"]["code"] == -32602 and "unknown variant" in invalid["error"]["message"], invalid
            assert not result["runtime_path_proven"] and not result["absence_proven"]
            assert call("trace_path", dict(args, max_depth=1))["depth_limit_reached"]
            limited = call("trace_path", dict(args, max_work=1))
            assert limited["status"] == "work_limit" and limited["work"] == 1
            limited = call("trace_path", dict(args, max_paths=1))
            assert len(limited["paths"]) == 1 and limited["result_limit_reached"]
            reverse = call("trace_path", dict(project="paths", **{"from": "finish", "to": "start"}, direction="back"))
            assert reverse["status"] == "found" and all(s["reversed"] for p in reverse["paths"] for s in p["steps"])
            ambiguous = call("trace_path", dict(args, **{"from": "duplicate"}))
            assert ambiguous["status"] == "ambiguous_endpoints" and ambiguous["from"]["total"] == 2
            disambiguated = call("trace_path", dict(args, **{"from": ambiguous["from"]["candidates"][0]["id"]}))
            assert disambiguated["status"] == "not_found_in_bounded_graph"
            value = dict(args, **{"from": "value"})
            assert call("trace_path", value)["status"] == "not_found_in_bounded_graph"
            values = call("trace_path", dict(value, relations="references"))
            assert [s["name"] for s in values["paths"][0]["symbols"]] == labels["value_reference"], values
            assert values["paths"][0]["steps"][0]["relation"] == "bound_reference"
            other = source / "other.rs"
            other.write_text("pub fn foreign() {}\n")
            call("ingest_file", dict(path=str(other), wing="other"))
            call("index_code", {})
            mismatch = call("trace_path", {"from": "start", "to": "foreign"})
            assert mismatch["status"] == "project_mismatch", mismatch
            bridge = source / labels["stale_bridge_document"]
            bridge.write_text("pub fn bridge() {}\n")
            # Query follows captured evidence until the source is explicitly ingested.
            assert call("trace_path", args)["status"] == "found"
            call("ingest_file", dict(path=str(bridge), wing="paths"))
            stale = call("trace_path", args)
            assert stale["status"] == "not_found_in_bounded_graph" and stale["stale_documents"] and not stale["paths"], stale
            call("index_code", {})
            rebound = call("trace_path", args)
            assert rebound["status"] == "not_found_in_bounded_graph" and not rebound["stale_documents"] and not rebound["paths"], rebound
            evidence = dict(status="passed", transport="stdio MCP", binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                            index_format=call("index_status", dict(project="paths"))["versions"]["index_format"],
                            fixture_sha256={str(p.relative_to(fixture)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(fixture.rglob("*")) if p.is_file()},
                            labelled_paths=len(labels["paths"]), paths=actual, production_database_touched=False,
                            checks=["exact_occurrence_provenance", "deterministic", "typed_relation_validation", "depth_budget", "work_budget", "result_budget",
                                    "reverse", "ambiguous_endpoints", "exact_id_disambiguation", "value_reference", "project_isolation",
                                    "captured_inputs", "stale_bridge_exclusion", "refresh_rebinding"],
                            limitations=["static graph only", "no absence proof", "no representative corpus quality or speed claim"])
            if report:
                report.parent.mkdir(parents=True, exist_ok=True)
                report.write_text(json.dumps(evidence, indent=2) + "\n")
            print(json.dumps(evidence))


if __name__ == "__main__":
    main()
