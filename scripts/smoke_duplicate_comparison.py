#!/usr/bin/env python3
"""Real MCP comparison metrics; temporary source files and mock embeddings only."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-duplicate-comparison-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        shared = "".join(f"shared private content line {i}\n" for i in range(10))
        for name, unique in [("sample.txt", "PRIVATE_ALPHA"), ("sample2.txt", "PRIVATE_BETA"), ("sample3.txt", "PRIVATE_GAMMA")]:
            (source / name).write_text(shared + unique + "\n")
        with StartupClient(binary, root, source) as mcp:
            def sync():
                result = mcp.call("sync_sources", dict(path=str(source), wing="compare"))
                assert not result.get("errors"), result
            sync()
            query = dict(project="compare", min_similarity=0.5, literal_mentions=dict(enabled=False))
            result = mcp.call("duplicates", query)
            assert len(result["groups"]) == 1, result
            group = result["groups"][0]
            assert len(group["members"]) == 3, group
            primary = group["primary_candidate"]
            for member in group["members"]:
                comparison = member["primary_comparison"]
                assert comparison["primary"]["document_id"] == primary, member
                assert comparison["member"]["content_hash"] and comparison["member"]["revision"], member
                if member["document"]["document_id"] == primary:
                    assert comparison["status"] == "self" and member["similarity_to_primary"] == 1, member
                else:
                    assert member["similarity_to_primary"] == 0.833, member
                    assert member["lines_only_here"] == member["lines_only_in_primary"] == 1, member
                    assert comparison["shared_unique_lines"] == 10 and comparison["union_unique_lines"] == 12, member
                    assert not comparison["exact_stored_text_equal"], member
            assert result["primary_comparison"]["pairs_checked"] == 2, result
            assert "PRIVATE_" not in json.dumps(result) and "shared private content" not in json.dumps(result), result
            assert result == mcp.call("duplicates", query)
            capped = mcp.call("duplicates", dict(query, max_members=1))
            assert capped["primary_comparison"]["pairs_requested"] == 0 and capped["primary_comparison"]["decoded_input_bytes_charged"] == 0, capped
            for option, reason in [(dict(enabled=False), "disabled"), (dict(max_file_bytes=1), "file_byte_limit"), (dict(max_total_bytes=1), "total_byte_budget"), (dict(max_lines=1), "line_limit"), (dict(max_unique_lines=1), "unique_line_limit"), (dict(max_line_bytes=1), "line_byte_limit")]:
                limited = mcp.call("duplicates", dict(query, primary_comparison=option))
                nonself = [m for m in limited["groups"][0]["members"] if m["document"]["document_id"] != primary]
                assert all(m["similarity_to_primary"] is None and m["lines_only_here"] is None and m["primary_comparison"]["reason"] == reason for m in nonself), limited
                assert not limited["primary_comparison"]["complete"], limited
            limited = mcp.call("duplicates", dict(query, primary_comparison=dict(max_pairs=1)))
            assert limited["primary_comparison"]["pairs_checked"] == 1 and limited["primary_comparison"]["pairs_not_checked"] == 1, limited
            # Stored text publication must replace metrics and identity, not reuse old normalization.
            changed = source / "sample3.txt"
            changed.write_text(shared + "PRIVATE_GAMMA\nPRIVATE_EXTRA\n")
            sync()
            updated = mcp.call("duplicates", query)
            pairs = [m["primary_comparison"] for m in updated["groups"][0]["members"] if m["primary_comparison"]["status"] == "checked"]
            assert any(p["member"]["path"] == str(changed) or p["primary"]["path"] == str(changed) for p in pairs), updated
            assert any(p["similarity"] == 0.769 for p in pairs), updated
            status = mcp.call("index_status", dict(project="compare"))
            report = dict(status="passed", versions=status["versions"], checks=["normalized_unique_line_jaccard", "directional_line_deltas", "rounding", "source_identity", "body_free_output", "determinism", "member_window_scope", "disabled_and_all_phase_limits", "pair_budget", "sync_replaces_metrics"], production_database_touched=False, source_code_executed=False, full_parity_or_deletion_safety_proven=False)
            if len(sys.argv) > 2:
                Path(sys.argv[2]).write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report))


if __name__ == "__main__":
    main()
