#!/usr/bin/env python3
"""Real MCP duplicate-role evidence with temporary sources and mock embeddings."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-duplicate-roles-") as temporary:
        root=Path(temporary).resolve();source=root/"sources"
        fixtures={
            "alpha/copies/theme.css":"body {color:red}\n",
            "alpha/copies/theme2.css":"body {color:red}\n",
            "alpha/copies/mod.js":"const value=1; // PRIVATE_BODY\n",
            "alpha/copies/mod2.js":"const value=1; // PRIVATE_BODY\n",
            "alpha/copies/icon.svg":"<svg><path d='M0 0'/></svg>\n",
            "alpha/copies/icon2.svg":"<svg><path d='M0 0'/></svg>\n",
            "alpha/copies/api.py":"def work():\n return 'PRIVATE_BODY'\n",
            "alpha/copies/api2.py":"def work():\n return 'PRIVATE_BODY'\n",
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
            query=dict(project="alpha",path_prefix=str(source/"alpha/copies"),max_members=1,min_similarity=1.0)
            def duplicates():return mcp.call("duplicates",query)
            def selected(report,suffix):
                return next(g for g in report["groups"] if g["members"][0]["document"]["path"].endswith(suffix))
            before=duplicates();assert before["role_contract_version"]==2 and before["groups_total"]==4,before
            assert "PRIVATE_BODY" not in json.dumps(before),before
            expected={"theme2.css":"incoming_file_resource_candidate","mod2.js":"incoming_file_import_candidate","icon2.svg":"incoming_file_resource_candidate","api2.py":"incoming_reference_candidate"}
            for name,kind in expected.items():
                group=selected(before,name);member=group["members"][0]
                assert group["primary_candidate"]==member["document"]["document_id"] and not group["primary_is_proven"] and not group["deletion_safe"],group
                assert group["members_total"]==2 and group["members_truncated"] and member["usage_score"]==3,group
                assert any(e["kind"]==kind and e["path"].startswith(str(source/"alpha")) and e["content_hash"] and e["revision"] for e in member["evidence"]),member
            assert before==duplicates()
            # Removing sources must exclude old evidence before the next index publication.
            for name,body in [("index.html","<main>changed</main>"),("use.js","const empty=1;\n"),("use.py","registered=None\n")]:
                path=source/"alpha"/name;path.write_text(body);mcp.call("ingest_file",dict(path=str(path),wing="alpha"))
            stale=duplicates();assert all(g["usage_score"]==0 for g in stale["groups"]),stale
            assert stale["stale_documents"],stale
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            refreshed=duplicates();assert all(g["usage_score"]==0 for g in refreshed["groups"]),refreshed
            path=source/"alpha/use.js";path.write_text("import './copies/mod2.js';\n"*105)
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            capped=selected(duplicates(),"mod2.js");member=capped["members"][0]
            assert member["evidence_total"]==105 and len(member["evidence"])==100 and member["evidence_truncated"],member
            status=mcp.call("index_status",dict(project="alpha"))
            report=dict(status="passed",versions=status["versions"],checks=["duplicate_role_contract_v2","exact_decoded_text_groups","target_prefix_keeps_external_source_evidence","HTML_CSS_resource_usage","non_code_SVG_identity","missing_export_file_import","function_value_reference","project_isolation","primary_visible_under_member_limit","evidence_source_revision_hash_anchors","deterministic_response","body_free_metadata","stale_source_exclusion_before_sync","source_sync_replacement","evidence_total_and_cap"],production_database_touched=False,source_code_executed=False,runtime_liveness_or_deletion_safety_proven=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+"\n")
            print(json.dumps(report))


if __name__=="__main__":main()
