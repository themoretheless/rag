#!/usr/bin/env python3
"""Real MCP inline-style resource qualification; temporary DB and mock embeddings."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-inline-style-") as temporary:
        root=Path(temporary).resolve();source=root/"sources"
        page="<p>Привет</p>\n<div style=\"background:url(&quot;a.svg?x=1&amp;y=2&quot;);mask-image:image-set('a.svg' 1x,'b.svg' 2x)\"></div>"
        fixtures={
            "alpha/a.svg":"<svg><title>PRIVATE_ASSET</title></svg>\n",
            "alpha/b.svg":"<svg/>\n",
            "alpha/phantom.svg":"<svg/>\n",
            "alpha/ghost.css":".ghost{color:red}\n",
            "alpha/page.html":page,
            "alpha/top.html":"<iframe src='page.html'></iframe>\n",
            "alpha/recovery.html":"<div style=\"};background:url('a.svg'); .bad{background:url('phantom.svg')}; @import 'ghost.css'; cursor:url('b.svg'),auto;broken tokens\"></div>",
            "alpha/duplicate.html":"<p style=\"background:url('b.svg')\" STYLE=\"mask-image:url('b.svg')\"></p>",
            "alpha/base.html":"<p style=\"background:url('b.svg')\"></p><base href='/deploy/'>",
            "alpha/xmlbase.html":"<svg xml:base='/deploy/'><g style=\"fill:url('b.svg')\"></g></svg>",
            "beta/foreign.html":"<p style=\"background:url('../alpha/b.svg')\"></p>",
        }
        for name,body in fixtures.items():
            path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ("alpha","beta"):
                sync=mcp.call("sync_sources",dict(path=str(source/project),wing=project));assert not sync.get("errors"),sync
            def impact(name,**kw):
                return mcp.call("impact",dict(impact_file=str(source/"alpha"/name),impact_project="alpha",**kw))
            result=impact("b.svg")
            expected={str(source/"alpha/page.html"),str(source/"alpha/recovery.html"),str(source/"alpha/top.html")}
            assert {r["path"] for r in result["items"]}==expected,result
            witness=next(r for r in result["items"] if r["path"]==str(source/"alpha/page.html"))["via"]
            assert witness["reference_kind"]=="html:style:image-set",witness
            assert witness["inline"]["property"]=="mask-image" and witness["inline"]["source_anchor"]=="html_style_attribute_value",witness
            start,end=witness["start_byte"],witness["inline"]["source_end_byte"]
            assert page.encode()[start:end].startswith(b'"background:url(&quot;'),witness
            assert witness["line"]==2 and witness["choice"]["source_end_byte"]==end and witness["choice"]["descriptors"]==["2x"],witness
            assert next(r for r in result["items"] if r["path"]==str(source/"alpha/top.html"))["depth"]==2,result
            assert impact("b.svg",depth=1)["depth_limited"]
            assert impact("b.svg",impact_relations="calls")["items"]==[]
            assert impact("phantom.svg")["items"]==[] and impact("ghost.css")["items"]==[]
            assert "PRIVATE_ASSET" not in json.dumps(result)
            dead=mcp.call("dead_code",dict(dead_code_view="files",dead_code_project="alpha"))
            assert str(source/"alpha/b.svg") in {r["file"]["path"] for r in dead["protected"]},dead
            path=source/"alpha/page.html";path.write_text(page.replace("2x","3x"))
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            changed=impact("b.svg");args=dict(before=result["index_rev"],after=changed["index_rev"],project="alpha")
            frozen=mcp.call("diff",args);assert frozen["inline_style_relations_available"],frozen
            assert any(r.get("choice",{}).get("descriptors")==["3x"] and r["inline"]["property"]=="mask-image" for r in frozen["relations_added"]),frozen
            path.write_text("\n\n"+path.read_text().replace("&quot;","&#34;"))
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            moved=impact("b.svg");diff=mcp.call("diff",dict(before=changed["index_rev"],after=moved["index_rev"],project="alpha"))
            assert diff["relations_added_total"]==0 and diff["relations_removed_total"]==0,diff
            assert mcp.call("diff",args)==frozen
            path.write_text("<p>removed style</p>\n")
            mcp.call("ingest_file",dict(path=str(path),wing="alpha"))
            stale=impact("b.svg");assert stale["stale_documents"],stale
            assert [r["path"] for r in stale["items"]]==[str(source/"alpha/recovery.html")],stale
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            refreshed=impact("b.svg");assert [r["path"] for r in refreshed["items"]]==[str(source/"alpha/recovery.html")] and not refreshed["stale_documents"],refreshed
            report=dict(status="passed",versions=mcp.call("index_status",dict(project="alpha"))["versions"],checks=["inline_url_and_image_set","unicode_entity_attribute_anchor","declaration_error_recovery","non_declaration_exclusion","duplicate_base_xmlbase_project_guards","transitive_impact","depth_and_call_mode","dead_file_protection","descriptor_diff","position_and_entity_spelling_no_churn","frozen_diff","stale_source_bridge_exclusion","sync_replaces_inline_links","no_source_bodies"],production_database_touched=False,source_applications_executed=False,browser_or_cascade_proven=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+"\n")
            print(json.dumps(report))


if __name__=="__main__":main()
