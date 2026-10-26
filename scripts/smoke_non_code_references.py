#!/usr/bin/env python3
"""Non-code reference targets through actual MCP; temporary sources/mock embeddings."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient
from mcp_smoke_client import McpClient


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-non-code-references-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        fixtures = {
            "alpha/api.py": "def load():\n q='SELECT * FROM users'\ndef save():\n q='INSERT INTO users(id) VALUES(1)'\ndef main(): load()\n@app.get('/items')\ndef items():\n return 'PRIVATE_BODY'\n",
            "alpha/raw.sql": 'SELECT * FROM users; SELECT * FROM public.logs; CREATE TABLE admin.logs(id INT); SELECT * FROM "Order Items";',
            "alpha/client.js": "fetch('/items',{method:'POST'}); fetch('/itemsets');\n",
            "alpha/view.html": "<a href='/items'>Items</a>",
            "alpha/ambiguous.js": "function handle() {}\nfunction handle() {}\napp.get('/ambiguous', handle);\n",
            "beta/api.py": "def load():\n q='SELECT * FROM users'\ndef use(): load()\n@app.get('/items')\ndef items(): pass\n",
        }
        for name, body in fixtures.items():
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
        with StartupClient(binary, root, source) as mcp:
            for project in ("alpha", "beta"):
                sync = mcp.call("sync_sources", dict(path=str(source / project), wing=project))
                assert not sync.get("errors"), sync
            query = dict(table="users", project="alpha")
            table = mcp.call("references", query)
            assert table["total"] == 3 and table["ownerless_total"] == 1 and table["counts_by_kind"] == {"queries": 3}, table
            assert {i["from"].get("name") for i in table["items"] if i["from"]["kind"] == "symbol"} == {"load", "save"}, table
            assert all(i["evidence"]["project"] == "alpha" and i["evidence"]["revision"] and i["evidence"]["content_hash"] for i in table["items"]), table
            read = mcp.call("references", dict(query, table_mode="read", limit=1))
            assert read["total"] == 2 and read["truncated"] and len(read["items"]) == 1, read
            write = mcp.call("references", dict(query, table_mode="write"))
            assert write["total"] == 1 and write["items"][0]["from"]["name"] == "save", write
            path = mcp.call("references", dict(query, path=str(source / "alpha/raw.sql")))
            assert path["total"] == 1 and path["ownerless_total"] == 1, path
            ambiguous = mcp.call("references", dict(table="users"))
            assert ambiguous["needs_disambiguation"] and ambiguous["target_candidates_total"] == 2, ambiguous
            schemas = mcp.call("references", dict(table="logs", project="alpha", path=str(source / "alpha/api.py"), table_mode="write", limit=1))
            assert schemas["needs_disambiguation"] and schemas["target_candidates_total"] == 2 and schemas["target_candidates_truncated"], schemas
            public = mcp.call("references", dict(table="public.logs", project="alpha"))
            assert public["total"] == 1 and not public["needs_disambiguation"], public
            declared = mcp.call("references", dict(table="admin.logs", project="alpha"))
            assert declared["target_observed"] and declared["total"] == 0 and not declared["absence_proven"], declared
            quoted = mcp.call("references", dict(table='"Order Items"', project="alpha"))
            assert quoted["total"] == 1, quoted
            route_query = dict(route="/items", project="alpha")
            route = mcp.call("references", route_query)
            assert route["total"] == 3 and route["counts_by_kind"] == {"endpoint_reference": 2, "routes": 1}, route
            registration = next(i for i in route["items"] if i["kind"] == "routes")
            assert registration["registration_method"] == "GET" and registration["request_method"] is None, registration
            client = next(i for i in route["items"] if i["reference_kind"] == "js:fetch")
            assert client["request_method"] == "POST" and client["registration_method"] is None and not client["handler_binding_from_url_proven"], client
            assert all(i["evidence"]["project"] == "alpha" and i["evidence"]["content_hash"] for i in route["items"]), route
            alternatives = mcp.call("references", dict(route="/ambiguous", project="alpha"))
            assert alternatives["total"] == 1 and alternatives["items"][0]["from"]["kind"] == "file" and alternatives["items"][0]["handler_candidates_total"] == 2, alternatives
            assert all(h["revision"] and h["content_hash"] for h in alternatives["items"][0]["handler_candidates"]), alternatives
            page = mcp.call("references", dict(route_query, limit=1))
            assert page["total"] == 3 and page["truncated"] and page["counts_by_kind"] == route["counts_by_kind"], page
            all_projects = mcp.call("references", dict(route="/items", path=str(source / "alpha/client.js")))
            assert all_projects["needs_disambiguation"] and all_projects["target_candidates_total"] == 2, all_projects
            literal = mcp.call("references", dict(route="/item%", project="alpha"))
            assert literal["total"] == 0 and not literal["target_observed"], literal
            symbols = mcp.call("references", dict(symbol="load", project="alpha"))
            assert symbols["total"] == 1 and symbols["reference_mode"] == "bound", symbols
            assert table == mcp.call("references", query) and route == mcp.call("references", route_query)
            assert "PRIVATE_BODY" not in json.dumps([table, route]), [table, route]
            for invalid in [dict(table="users; DROP TABLE users"), dict(table="users", symbol="load"), dict(table="users", route="/items"), dict(route="/items", table_mode="read"), dict(table="users", table_mode="unknown"), dict(table="users", reference_mode="lexical"), dict(route="\u0000")]:
                McpClient.request(mcp, "tools/call", dict(name="references", arguments=invalid), expect_error=True)
            api = source / "alpha/api.py"
            api.write_text(fixtures["alpha/api.py"].replace("users", "accounts").replace("'/items'", "'/other'"))
            mcp.call("ingest_file", dict(path=str(api), wing="alpha"))
            stale = mcp.call("references", query)
            assert stale["total"] == 1 and len(stale["stale_documents"]) == 1, stale
            stale_routes = mcp.call("references", route_query)
            assert stale_routes["total"] == 2 and stale_routes["counts_by_kind"] == {"endpoint_reference": 2}, stale_routes
            sync = mcp.call("sync_sources", dict(path=str(source / "alpha"), wing="alpha"))
            assert not sync.get("errors"), sync
            updated = mcp.call("references", query)
            assert updated["total"] == 1 and not updated["stale_documents"], updated
            new_route = mcp.call("references", dict(route="/other", project="alpha"))
            assert new_route["total"] == 1 and new_route["counts_by_kind"] == {"routes": 1}, new_route
            status = mcp.call("index_status", dict(project="alpha"))
            result = dict(status="passed", versions=status["versions"], checks=["queries_relation", "symbol_owners_and_ownerless_files", "read_write_and_source_filters", "counts_before_result_limit", "project_and_schema_disambiguation_before_filters", "schema_only_observed_target", "quoted_identifiers", "registration_vs_endpoint_relations", "separate_registration_request_methods", "handler_alternatives_not_fake_owners_or_duplicate_occurrences", "literal_URL_identity", "project_scoped_legacy_symbol_mode", "identity_hash_revision_evidence", "body_free_determinism", "mixed_target_and_invalid_argument_rejection", "stale_source_exclusion", "sync_replacement"], production_database_touched=False, source_code_or_SQL_executed=False, runtime_database_or_router_binding_proven=False, full_RAGRAF_parity_or_representative_quality_proven=False)
            if len(sys.argv) > 2:
                Path(sys.argv[2]).write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result))


if __name__ == "__main__":
    main()
