#!/usr/bin/env python3
"""Real MCP key/URL impact, all direct roots and invalid-source bridge regression."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-literal-impact-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        long_url = "/" + "x" * 3000
        fixtures = {
            "alpha/api.php": "<?php function render(){Translate('key');} function alternate(){Translate('key');} function serve(){render();} function alternate_caller(){alternate();} function main(){serve();}",
            "alpha/view.html": "%{key}<a href='/items'>Link</a>",
            "alpha/site.html": "<iframe src='view.html'></iframe>",
            "alpha/url.js": "function load(){fetch('/items',{method:'POST'});} function main_client(){load();} function handler(){} function handler(){} function handler_caller(){handler();} app.get('/items',handler);",
            "alpha/use.sql": "SELECT * FROM public.logs;",
            "alpha/schema.sql": "CREATE TABLE admin.logs(id INT);",
            "alpha/client_error.js": "export function load_error(){fetch('/error');}",
            "alpha/broken.js": "import {load_error} from './client_error.js'; export function bridge(){load_error();}\nconst = ;",
            "alpha/outer.js": "import {bridge} from './broken.js'; export function outside(){bridge();}",
            "beta/view.html": "%{key}<a href='/items'>Foreign</a>",
        }
        fixtures["alpha/long_url.js"] = "function long_load(){fetch('" + long_url + "');} function long_caller(){long_load();}"
        for name, body in fixtures.items():
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
        with StartupClient(binary, root, source) as mcp:
            for project in ("alpha", "beta"):
                sync = mcp.call("sync_sources", dict(path=str(source / project), wing=project))
                assert not sync.get("errors"), sync
            symbols = mcp.call("symbol", dict(project="alpha", limit=500))["items"]
            names = {s["id"]: s["name"] for s in symbols}
            def affected(result):
                return {names[i["symbol_id"]] for i in result["items"]}
            query = dict(impact_key="key", impact_project="alpha")
            key = mcp.call("impact", query)
            assert key["direct_total"] == 3 and key["direct_owner_symbols"] == 2 and key["ownerless_direct_uses"] == 1 and key["direct_files_total"] == 2, key
            assert affected(key) == {"serve", "alternate_caller", "main"}, key
            assert key["file_impact"]["seed_documents_total"] == 2 and [i["path"] for i in key["file_impact"]["items"]] == [str(source / "alpha/site.html")], key
            assert key["impact_relations"] == "references" and not key["program_impact_complete"], key
            limited = mcp.call("impact", dict(query, limit=1))
            assert limited["direct_truncated"] and limited["direct_owner_symbols"] == 2 and limited["code_impact"]["matched_symbols"] == 2 and limited["file_impact"]["seed_documents_total"] == 2, limited
            assert limited["file_impact"]["items"][0]["path"] == str(source / "alpha/site.html"), limited
            shallow = mcp.call("impact", dict(query, depth=1))
            assert affected(shallow) == {"serve", "alternate_caller"} and shallow["depth_limited"], shallow
            calls = mcp.call("impact", dict(query, impact_relations="calls"))
            assert affected(calls) == affected(key) and calls["file_impact"]["items"] == [], calls
            template = mcp.call("impact", dict(query, path=str(source / "alpha/view.html")))
            assert template["direct_total"] == 1 and template["direct_owner_symbols"] == 0 and template["items"] == [] and len(template["file_impact"]["items"]) == 1, template
            route_query = dict(impact_route="/items", impact_project="alpha")
            route = mcp.call("impact", route_query)
            assert route["direct_total"] == 3 and route["direct_owner_symbols"] == 1 and affected(route) == {"main_client"}, route
            registration = mcp.call("impact", dict(route_query, impact_route_kind="registration"))
            assert registration["direct_total"] == 1 and registration["direct_owner_symbols"] == 0 and registration["items"] == [], registration
            assert registration["direct_occurrences"][0]["handler_candidates_total"] == 2 and not registration["direct_occurrences"][0]["handler_candidates_are_seeds"], registration
            endpoints = mcp.call("impact", dict(route_query, impact_route_kind="endpoint"))
            assert endpoints["direct_total"] == 2 and affected(endpoints) == {"main_client"}, endpoints
            assert any(o["request_method"] == "POST" for o in endpoints["direct_occurrences"]), endpoints
            for args in [dict(impact_key="key", path=str(source / "alpha/view.html")), dict(impact_route="/items", impact_route_kind="registration", path=str(source / "alpha/url.js"))]:
                ambiguous = mcp.call("impact", args)
                assert ambiguous["needs_disambiguation"] and ambiguous["target_candidates_total"] == 2, ambiguous
            schema = mcp.call("impact", dict(impact_table="logs", impact_project="alpha", path=str(source / "alpha/use.sql"), impact_table_mode="write"))
            assert schema["needs_disambiguation"] and schema["table_candidates_total"] == 2, schema
            schema_only = mcp.call("impact", dict(impact_table="admin.logs", impact_project="alpha"))
            assert schema_only["target_observed"] and schema_only["direct_total"] == 0, schema_only
            bridge = mcp.call("impact", dict(impact_route="/error", impact_project="alpha"))
            assert bridge["direct_total"] == 1 and bridge["items"] == [] and bridge["file_impact"]["items"] == [] and bridge["parse_errors"], bridge
            assert bridge["code_impact"]["invalid_input_documents_excluded"], bridge
            long_result = mcp.call("impact", dict(impact_route=long_url, impact_project="alpha"))
            assert long_result["direct_total"] == 1 and affected(long_result) == {"long_caller"}, long_result
            assert mcp.call("references", dict(route=long_url, project="alpha"))["total"] == 1

            for name, total in [("tpl:key", 3), ("route:/items", 3), ("@table:public.logs", 1)]:
                node = mcp.call("symbol", dict(name=name, project="alpha"))["items"][0]
                assert mcp.call("impact", node["impact_target"])["direct_total"] == total, node
            for occurrence in key["direct_occurrences"] + route["direct_occurrences"]:
                evidence = occurrence["evidence"]
                assert evidence["project"] == "alpha" and evidence["revision"] >= 1 and len(evidence["content_hash"]) == 64, occurrence
                raw = Path(evidence["path"]).read_bytes()
                assert raw[:evidence["start_byte"]].count(b"\n") + 1 == evidence["line"], occurrence
                if occurrence["kind"] == "uses_key":
                    assert b"key" in raw[evidence["start_byte"]:evidence["end_byte"]], occurrence
                else:
                    assert b"/items" in raw[evidence["start_byte"]:evidence["start_byte"] + 512], occurrence
            assert key == mcp.call("impact", query) and route == mcp.call("impact", route_query)
            for invalid in [dict(impact_key="key", impact_route="/items"), dict(impact_key="key", symbol="render"), dict(impact_route="/items", impact_file="x"), dict(impact_route_kind="endpoint"), dict(impact_key=""), dict(impact_route="\u0000"), dict(impact_key="key", impact_relations="unknown"), dict(impact_route="/items", impact_route_kind="unknown")]:
                assert "error" in mcp.call("impact", invalid), invalid
            # An invalid source bridge stays disconnected; a stale formerly-valid resource
            # bridge must also disappear before the next publication.
            site = source / "alpha/site.html"
            site.write_text("<main>Changed</main>")
            mcp.call("ingest_file", dict(path=str(site), wing="alpha"))
            stale = mcp.call("impact", query)
            assert stale["file_impact"]["items"] == [] and stale["stale_documents"], stale
            sync = mcp.call("sync_sources", dict(path=str(source / "alpha"), wing="alpha"))
            assert not sync.get("errors"), sync
            php = source / "alpha/api.php"
            php.write_text(fixtures["alpha/api.php"].replace("Translate('key')", "Translate('new.key')"))
            mcp.call("ingest_file", dict(path=str(php), wing="alpha"))
            stale = mcp.call("impact", query)
            assert stale["direct_total"] == 1 and stale["direct_owner_symbols"] == 0 and stale["items"] == [], stale
            sync = mcp.call("sync_sources", dict(path=str(source / "alpha"), wing="alpha"))
            assert not sync.get("errors"), sync
            # Source symbol IDs include byte positions; refresh the name map after publication.
            names.update({s["id"]: s["name"] for s in mcp.call("symbol", dict(project="alpha", limit=500))["items"]})
            updated = mcp.call("impact", dict(impact_key="new.key", impact_project="alpha"))
            assert updated["direct_total"] == 2 and updated["direct_owner_symbols"] == 2 and affected(updated) == {"serve", "alternate_caller", "main"}, updated
            report = dict(status="passed", versions=mcp.call("index_status", dict(project="alpha"))["versions"], checks=["literal_key_direct_owners_and_ownerless_templates", "indirect_code_and_resource_file_graphs", "all_roots_before_direct_output_limits", "independent_depth_result_and_relation_modes", "route_registration_vs_client_occurrences", "handler_alternatives_not_owners_or_seeds", "project_disambiguation_before_path_and_kind_filters", "schema_only_SQL_identity_before_path_access_filters", "parse_error_bridge_exclusion", "symbol_impact_target_roundtrips", "long_captured_URL_selector_alignment", "source_identity_revision_hash_evidence", "determinism", "invalid_mixed_target_rejection", "stale_resource_bridge_exclusion", "stale_direct_owner_exclusion", "sync_replaces_literal_roots"], source_code_or_SQL_executed=False, production_database_touched=False, runtime_key_or_router_impact_proven=False, representative_quality_scale_or_full_parity_proven=False)
            if len(sys.argv) > 2:
                Path(sys.argv[2]).write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report))


if __name__ == "__main__":
    main()
