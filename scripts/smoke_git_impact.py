#!/usr/bin/env python3
"""Actual Git + stdio MCP qualification; no production database or repository."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from mcp_smoke_client import McpClient


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp").resolve()
    report = Path(sys.argv[2]) if len(sys.argv) > 2 else None
    fixture = Path(__file__).resolve().parents[1] / "tests/fixtures/code-intelligence/git-impact"
    labels = json.loads((fixture / "expected.json").read_text())
    with tempfile.TemporaryDirectory(prefix="rag-git-impact-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        shutil.copytree(fixture, source)
        def git(*args, input=None):
            return subprocess.run(["git", "-C", str(source), *args], input=input, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, check=True).stdout.decode().strip()
        def commit(message):
            git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-qm", message)
        git("init", "-q", "--object-format=sha1")
        git("add", ".")
        commit("base")
        before = git("rev-parse", "HEAD")
        worker = source / "src/worker.rs"
        worker.write_text(labels["staged_worker"])
        git("add", "src/worker.rs")
        staged_oid = git("rev-parse", ":src/worker.rs")
        worker.write_text(labels["working_worker"])
        index_before = (source / ".git/index").read_bytes()
        with McpClient(binary, root, source) as client:
            call = client.call
            args = dict(repository=str(source), project="fixture")
            stage = call("detect_changes", dict(args, scope="staged"))
            changes = {s["name"]: c["kind"] for c in stage["changes"] for s in c["before"] + c["after"]}
            assert changes == labels["changes"], stage
            assert not stage["persistent_index_used"] and stage["graph_origin"] == "ephemeral_git_source_snapshots"
            paths = []
            for side in ("before_impact", "after_impact"):
                actual = []
                for item in stage[side]["items"]:
                    path = item["path"]
                    names = [s["name"] for s in path["symbols"]]
                    actual.append(names)
                    assert path["complete_to_seed"] and item["seed_change_key"], item
                    for step, symbol in zip(path["steps"], path["symbols"]):
                        ref = step["evidence"]["reference"]
                        assert step["relation"] == "call" and ref["document_id"] == symbol["document_id"], step
                        assert ref["content_hash"] == symbol["content_hash"] and ref["line"] > 0, step
                assert sorted(actual) == sorted(labels["witnesses"]), stage
                paths.append(actual)
            staged_file = next(f for f in stage["files"] if f["path"] == "src/worker.rs")
            assert staged_file["after"]["evidence"]["blob_oid"] == staged_oid
            work = call("detect_changes", dict(args, scope="working_tree"))
            working_file = next(f for f in work["files"] if f["path"] == "src/worker.rs")
            assert working_file["before"]["evidence"]["blob_oid"] == staged_oid
            assert working_file["before"]["source_hash"] == staged_file["after"]["source_hash"]
            assert working_file["after"]["source_hash"] != working_file["before"]["source_hash"]
            assert (source / ".git/index").read_bytes() == index_before
            assert worker.read_text() == labels["working_worker"]
            assert work == call("detect_changes", dict(args, scope="working_tree"))
            limited = call("detect_changes", dict(args, scope="staged", limit=1, max_seeds=1, max_affected=1))
            assert limited["changes_truncated"] and len(limited["changes"]) == 1
            shallow = call("detect_changes", dict(args, scope="staged", depth=1))
            assert shallow["before_impact"]["depth_limited"] and len(shallow["before_impact"]["items"]) == 1
            exhausted = client.request("tools/call", dict(name="detect_changes", arguments=dict(args, max_files=1)), expect_error=True)
            assert "budget" in exhausted["error"]["message"], exhausted
            commit("staged")
            after = git("rev-parse", "HEAD")
            compare = dict(args, scope="commits", before=before, after=after)
            frozen = call("detect_changes", compare)
            assert frozen["before"]["git"]["commit"] == before and frozen["after"]["git"]["commit"] == after
            worker.write_text("pub fn value()->i32{99}\n")
            assert call("detect_changes", compare) == frozen
            # External clean filters must never run during working-source capture.
            marker = source / "FILTER_EXECUTED"
            git("config", "filter.evil.clean", f"touch '{marker}'; cat")
            (source / ".gitattributes").write_text("*.rs filter=evil\n")
            (source / "src/untracked file.rs").write_text("pub fn new_file() {}\n")
            all_changes = call("detect_changes", dict(args, scope="all"))
            assert any(c["kind"] == "added" and any(s["name"] == "new_file" for s in c["after"]) for c in all_changes["changes"])
            assert not marker.exists()
            # Create real unmerged index stages without invoking conversion filters.
            zeros = "0" * len(staged_oid)
            records = f"0 {zeros}\tsrc/worker.rs\n100644 {staged_oid} 1\tsrc/worker.rs\n100644 {staged_oid} 2\tsrc/worker.rs\n"
            git("update-index", "--index-info", input=records.encode())
            conflict = client.request("tools/call", dict(name="detect_changes", arguments=dict(args, scope="staged")), expect_error=True)
            assert "unmerged" in conflict["error"]["message"], conflict
            invalid = client.request("tools/call", dict(name="detect_changes", arguments=dict(args, scope="guessed")), expect_error=True)
            assert invalid["error"]["code"] == -32602
            evidence = dict(status="passed", transport="stdio MCP", git_version=git("--version"), index_format=stage["index_format"],
                            binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(), changes=changes, labelled_witnesses=paths,
                            fixture_sha256={str(p.relative_to(fixture)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(fixture.rglob("*")) if p.is_file()},
                            checks=["staged_vs_working_source_identity", "added_removed_modified", "two_sided_multihop_witnesses", "occurrence_provenance",
                                    "index_and_source_not_mutated", "deterministic", "return_depth_seed_budgets", "capture_budget_error", "pinned_commit_comparison",
                                    "untracked_sources", "external_filters_not_executed", "unmerged_index_refusal", "typed_scope_validation"],
                            persistent_index_used=False, production_database_touched=False, production_repository_touched=False,
                            limitations=["supported source/manifest formats", "ephemeral native graph", "no runtime or absence proof", "no competitor performance claim"])
            if report:
                report.parent.mkdir(parents=True, exist_ok=True)
                report.write_text(json.dumps(evidence, indent=2) + "\n")
            print(json.dumps(evidence))


if __name__ == "__main__":
    main()
