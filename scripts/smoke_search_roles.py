#!/usr/bin/env python3
"""Actual MCP multi-search role evidence over temporary decoded source files."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-search-roles-") as temporary:
        root=Path(temporary).resolve();source=root/"sources"
        fixtures={
            "alpha/copies/theme.css":"/*needle*/ body {color:red}\n",
            "alpha/copies/theme2.css":"/*needle*/ body {color:red}\n",
            "alpha/copies/mod.js":"//needle\nconst value=1;\n",
            "alpha/copies/mod2.js":"//needle\nconst value=1;\n",
            "alpha/copies/icon.svg":"<svg><text>needle</text></svg>\n",
            "alpha/copies/icon2.svg":"<svg><text>needle</text></svg>\n",
            "alpha/copies/api.py":"def work():\n return 'needle PRIVATE_BODY'\n",
            "alpha/copies/api2.py":"def work():\n return 'needle PRIVATE_BODY'\n",
            "alpha/index.html":"<link rel='stylesheet' href='copies/theme2.css'><img src='copies/icon2.svg'>",
            "alpha/use.js":"import {missing} from './copies/mod2.js';\n",
            "alpha/use.py":"from copies.api2 import work\nregistered=work\n",
            "beta/index.html":"<link rel='stylesheet' href='../alpha/copies/theme.css'>",
        }
        for name,body in fixtures.items():
            path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ("alpha","beta"):
                sync=mcp.call("sync_sources",dict(path=str(source/project),wing=project));assert not sync.get("errors"),sync
            query=dict(queries=["needle","color"],project="alpha",path_prefix=str(source/"alpha/copies"),limit=20)
            def search():return mcp.call("search_multi",query)
            def family(report,name):return next(g for g in report["families"] if g["name"]==name)
            before=search();assert before["matching_documents_total"]==2 and before["per_query_document_counts"]==[8,2] and before["families_retained_total"]==4,before
            assert before["observation_consistency"]=="single_read_transaction" and before["family_role_analysis"]["role_contract_version"]==2,before
            for name,used,kind in [("theme.css","theme2.css","incoming_file_resource_candidate"),("mod.js","mod2.js","incoming_file_import_candidate"),("icon.svg","icon2.svg","incoming_file_resource_candidate"),("api.py","api2.py","incoming_reference_candidate")]:
                group=family(before,name);member=group["members"][0]
                assert member["document"]["path"].endswith(used) and member["usage_score"]==3,member
                assert member["role_evidence_total"]>0 and any(e["kind"]==kind and e["path"].startswith(str(source/"alpha")) and e["content_hash"] and e["revision"] for e in member["role_evidence"]),member
                assert "PRIVATE_BODY" not in json.dumps(member["role_evidence"])
            assert before==search()
            any_mode=mcp.call("search_multi",dict(**query,match_mode="any"));assert any_mode["matching_documents_total"]==8,any_mode
            empty=mcp.call("search_multi",dict(queries=["absent-token"],project="alpha",path_prefix=query["path_prefix"]));assert empty["family_role_analysis"] is None and empty["matching_documents_total"]==0,empty
            for name,body in [("index.html","<main>changed</main>"),("use.js","const empty=1;\n"),("use.py","registered=None\n")]:
                path=source/"alpha"/name;path.write_text(body);mcp.call("ingest_file",dict(path=str(path),wing="alpha"))
            stale=search();assert stale["family_role_analysis"]["stale_documents"] and all(g["usage_score"]==0 for g in stale["families"]),stale
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            refreshed=search();assert all(g["usage_score"]==0 for g in refreshed["families"]),refreshed
            path=source/"alpha/use.js";path.write_text("import './copies/mod2.js';\n"*105)
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            capped=family(search(),"mod.js")["members"][0];assert capped["role_evidence_total"]==105 and len(capped["role_evidence"])==100 and capped["role_evidence_truncated"],capped
            status=mcp.call("index_status",dict(project="alpha"))
            report=dict(status="passed",versions=status["versions"],checks=["search_role_contract_v2","single_read_transaction_contract","independent_all_any_query_windows","external_non_matching_referrer_evidence","HTML_CSS_resources","published_SVG_target","missing_export_import","bound_function_value","project_prefix_isolation","deterministic_family_order","body_free_role_metadata","zero_hit_without_role_index","stale_source_exclusion","sync_replacement","total_and_retained_evidence_counts"],production_database_touched=False,source_code_executed=False,runtime_activity_proven=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+"\n")
            print(json.dumps(report))


if __name__=="__main__":main()
