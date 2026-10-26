#!/usr/bin/env python3
"""Qualify non-code captured resource identities using the real stdio MCP."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-resource-targets-") as temporary:
        root=Path(temporary).resolve();source=root/"sources"
        fixtures={
            "alpha/icon.svg":"<svg xmlns='http://www.w3.org/2000/svg'><script>PRIVATE_ASSET</script></svg>\n",
            "alpha/data.json":'{"ok":true}\n',
            "alpha/main.css":".a{background:url('icon.svg')}\n",
            "alpha/page.html":"<link href='main.css'><object data='data.json'></object>\n",
            "alpha/spare.py":"def spare(): pass\n",
            "beta/foreign.html":"<object data='../alpha/data.json'></object>\n",
            "beta/data.json":'{"other":true}\n',
        }
        for name,body in fixtures.items():
            path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ("alpha","beta"):
                sync=mcp.call("sync_sources",dict(path=str(source/project),wing=project));assert not sync.get("errors"),sync
            def impact(name,**kw):
                return mcp.call("impact",dict(impact_file=str(source/"alpha"/name),impact_project="alpha",**kw))
            def dead():return mcp.call("dead_code",dict(dead_code_view="files",dead_code_project="alpha"))
            result=impact("icon.svg")
            assert [r["path"] for r in result["items"]]==[str(source/"alpha/main.css"),str(source/"alpha/page.html")],result
            target=result["target"]["captured_resource_target"]
            assert target["state"]=="fresh" and target["content_hash"]==target["published_hash"],result
            assert not result["resource_target_diagnostics"]["live_filesystem_checked"],result
            assert impact("icon.svg",impact_relations="calls")["items"]==[]
            data=impact("data.json")
            assert [r["path"] for r in data["items"]]==[str(source/"alpha/page.html")],data
            status=mcp.call("index_status",dict(project="alpha"))
            assert status["coverage"]["indexable_documents"]==3,status
            assert status["resource_targets"]["diagnostics"]["fresh_targets"]==2,status
            assert not status["resource_targets"]["syntax_indexed"],status
            protected={r["file"]["path"] for r in dead()["protected"]}
            assert {str(source/"alpha/icon.svg"),str(source/"alpha/data.json")}<=protected
            assert "PRIVATE_ASSET" not in json.dumps([result,data,status])
            icon=source/"alpha/icon.svg";icon.write_text("<svg><title>changed</title></svg>\n")
            # The view describes stored corpus identity, not an implicit live read.
            assert impact("icon.svg")["target"]["captured_resource_target"]["state"]=="fresh"
            mcp.call("ingest_file",dict(path=str(icon),wing="alpha"))
            stale=impact("icon.svg")
            assert stale["target_found"] and not stale["target_available"] and not stale["traversal_complete"],stale
            assert stale["target"]["captured_resource_target"]["state"]=="stale" and stale["items"]==[],stale
            assert stale["stale_documents"]==[],stale
            status=mcp.call("index_status",dict(project="alpha"))
            assert "RESOURCE_TARGETS_UNAVAILABLE" in status["quality"]["problems"],status
            unknown=dead()
            assert str(source/"alpha/spare.py") in {r["file"]["path"] for r in unknown["not_checked"]},unknown
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            fresh=impact("icon.svg");assert len(fresh["items"])==2 and fresh["target_available"],fresh
            page=source/"alpha/page.html";page.write_text("<link href='main.css'>\n")
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            removed=impact("data.json");assert removed["target_available"] and removed["items"]==[],removed
            possible=dead()
            assert str(source/"alpha/data.json") in {r["file"]["path"] for r in possible["items"]},possible
            diff_args=dict(before=result["index_rev"],after=removed["index_rev"],project="alpha")
            frozen=mcp.call("diff",diff_args)
            assert frozen["resource_target_relations_available"],frozen
            assert any(r["kind"]=="file_resource_candidate" and r["target"]==data["target"]["document_id"] for r in frozen["relations_removed"]),frozen
            icon.write_text("<svg><title>later</title></svg>\n")
            mcp.call("ingest_file",dict(path=str(icon),wing="alpha"))
            assert mcp.call("diff",diff_args)==frozen
            report=dict(status="passed",versions=status["versions"],checks=["svg_sync_ingestion","json_resource_target","transitive_resource_impact","source_revision_and_hash","project_isolation","parser_coverage_separation","no_fabricated_calls","dead_file_protection","stored_not_implicit_live_view","stale_target_exclusion","status_diagnostics","negative_liveness_guard","sync_refresh","removed_link_dead_candidate","immutable_resource_diff","no_source_bodies"],production_database_touched=False,source_applications_executed=False,raw_binary_assets_qualified=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+"\n")
            print(json.dumps(report))


if __name__=="__main__":main()
