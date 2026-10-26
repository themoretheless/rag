#!/usr/bin/env python3
"""Shared captured symbol nodes through actual MCP with temporary/mock data."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient
from mcp_smoke_client import McpClient


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-symbol-nodes-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        fixtures = {
            "alpha/render.php": "<?php function render(){Translate('key'); Translate('literal%_key'); return 'PRIVATE_BODY';}",
            "alpha/view.html": "<a href='/items'>%{key} %{key} {{OtherKey}}</a>",
            "alpha/api.js": "function handler(){} function handler(){} app.get('/items',handler); fetch('/items');",
            "alpha/schema.sql": "CREATE TABLE public.logs(id INT); CREATE TABLE admin.logs(id INT); CREATE TABLE empty_table(id INT);",
            "alpha/use.sql": 'SELECT * FROM public.logs; SELECT * FROM users; SELECT * FROM "Order Items";',
            "beta/view.html": "%{key}",
            "beta/use.sql": "SELECT * FROM users;",
        }
        for name, body in fixtures.items():
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
        with StartupClient(binary, root, source) as mcp:
            for project in ("alpha", "beta"):
                sync = mcp.call("sync_sources", dict(path=str(source / project), wing=project))
                assert not sync.get("errors"), sync
            query = dict(symbol_kind="translation_key", project="alpha", name="key", evidence_limit=1)
            result = mcp.call("symbol", query)
            assert result["total"] == 1, result
            node = result["items"][0]
            assert node["observed_usage_total"] == node["source_evidence_total"] == 3 and len(node["source_evidence"]) == 1 and node["source_evidence_truncated"], node
            assert node["path"] is None and node["start_line"] is None and not node["definition_location_available"], node
            assert node["reference_target"] == dict(project="alpha", key="key"), node
            assert mcp.call("references", node["reference_target"])["total"] == node["observed_usage_total"], node
            assert mcp.call("symbol", dict(symbol_kind="translation_key", project="alpha", symbol=node["id"]))["items"][0]["id"] == node["id"]
            assert mcp.call("symbol", dict(project="alpha", name="tpl:key"))["items"][0]["id"] == node["id"]
            path = mcp.call("symbol", dict(query, path=str(source / "alpha/view.html")))
            assert path["items"][0]["id"] == node["id"] and path["items"][0]["observed_usage_total"] == 2, path
            projects = mcp.call("symbol", dict(symbol_kind="translation_key", name="key"))
            assert projects["total"] == 2 and len({i["id"] for i in projects["items"]}) == 2 and {i["project"] for i in projects["items"]} == {"alpha", "beta"}, projects
            tables = mcp.call("symbol", dict(project="alpha", symbol_kind="table", name="logs", limit=1))
            assert tables["total"] == 2 and tables["truncated"], tables
            declared = mcp.call("symbol", dict(project="alpha", name="@table:admin.logs"))["items"][0]
            assert declared["captured_schema_evidence"] and declared["observed_usage_total"] == 0 and declared["schema_evidence_total"] == 1, declared
            assert mcp.call("references", declared["reference_target"])["target_observed"], declared
            used = mcp.call("symbol", dict(project="alpha", symbol_kind="table", name="users"))["items"][0]
            assert not used["captured_schema_evidence"] and used["observed_usage_total"] == 1, used
            quoted = mcp.call("symbol", dict(project="alpha", name='@table:"Order Items"'))["items"][0]
            assert mcp.call("references", quoted["reference_target"])["total"] == 1, quoted
            route = mcp.call("symbol", dict(project="alpha", name="route:/items"))["items"][0]
            assert route["observed_usage_total"] == 3 and route["counts_by_relation"] == {"routes": 1, "endpoint_reference": 2}, route
            assert mcp.call("references", route["reference_target"])["total"] == 3, route
            code = mcp.call("symbol", dict(project="alpha"))
            assert code["total"] == 3 and all(i["path"] and i["start_line"] for i in code["items"]), code
            assert mcp.call("symbol", dict(project="alpha", name="render"))["items"][0]["name"] == "render"
            for qualified in ("tpl::member", "route::member"):
                guarded = mcp.call("symbol", dict(project="alpha", name=qualified))
                assert guarded["symbol_kind"] == "code" and guarded["items"] == [], guarded
            explicit = mcp.call("symbol", dict(project="alpha", name="tpl:key", symbol_kind="code"))
            assert explicit["symbol_kind"] == "code" and explicit["items"] == [], explicit
            all_nodes = mcp.call("symbol", dict(project="alpha", symbol_kind="all"))
            assert all_nodes["counts_by_domain"] == {"code": 3, "translation_key": 3, "table": 5, "route": 1}, all_nodes
            capped = mcp.call("symbol", dict(project="alpha", symbol_kind="all", limit=1))
            assert capped["total"] == all_nodes["total"] and capped["counts_by_domain"] == all_nodes["counts_by_domain"] and capped["truncated"], capped
            literal = mcp.call("symbol", dict(project="alpha", symbol_kind="translation_key", match_mode="contains", name="%"))
            assert literal["total"] == 1 and literal["items"][0]["name"] == "literal%_key", literal
            assert mcp.call("symbol", dict(project="alpha", symbol_kind="all", match_mode="contains", name=".*"))["total"] == 0
            for item in all_nodes["items"]:
                if item["node_domain"] != "code":
                    assert all(e["revision"] and e["content_hash"] and e["project"] == "alpha" for e in item["source_evidence"]), item
            assert "PRIVATE_BODY" not in json.dumps(all_nodes), all_nodes
            assert result == mcp.call("symbol", query)
            for invalid in [dict(name="render", symbol="render"), dict(symbol_kind="unknown"), dict(match_mode="regex")]:
                McpClient.request(mcp, "tools/call", dict(name="symbol", arguments=invalid), expect_error=True)
            # Filesystem changes alone cannot mutate captured identities.
            php = source / "alpha/render.php"
            php.write_text(fixtures["alpha/render.php"].replace("Translate('key')", "Translate('new.key')"))
            assert result == mcp.call("symbol", query)
            mcp.call("ingest_file", dict(path=str(php), wing="alpha"))
            stale = mcp.call("symbol", query)
            assert stale["items"][0]["id"] == node["id"] and stale["items"][0]["observed_usage_total"] == 2 and stale["stale_documents"], stale
            sync = mcp.call("sync_sources", dict(path=str(source / "alpha"), wing="alpha"))
            assert not sync.get("errors"), sync
            updated = mcp.call("symbol", query)
            assert updated["items"][0]["id"] == node["id"] and updated["items"][0]["observed_usage_total"] == 2 and not updated["stale_documents"], updated
            assert mcp.call("symbol", dict(project="alpha", name="tpl:new.key"))["total"] == 1
            status = mcp.call("index_status", dict(project="alpha"))
            report = dict(status="passed", versions=status["versions"], checks=["shared_key_no_last_writer_location", "stable_project_scoped_typed_ids", "source_proof_retention_and_full_counts", "reference_target_roundtrips", "exact_id_and_prefixed_FQN_lookup", "namespace_prefix_guards_and_explicit_code_override", "path_scoped_counts_stable_identity", "schema_only_usage_only_and_quoted_tables", "route_alternatives_not_duplicate_uses", "legacy_code_default_and_name_alias", "all_domains_and_counts_before_limit", "literal_contains_not_SQL_or_regex", "source_revision_hash_identity", "body_free_determinism", "invalid_input_rejection", "stored_snapshot_without_filesystem_reads", "stale_exclusion", "sync_replacement"], source_code_or_SQL_executed=False, production_database_touched=False, runtime_binding_or_catalogue_definitions_proven=False, representative_quality_scale_or_full_parity_proven=False)
            if len(sys.argv) > 2:
                Path(sys.argv[2]).write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report))


if __name__ == "__main__":
    main()
