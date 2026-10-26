#!/usr/bin/env python3
"""Real MCP responsive-image resource checks with isolated data/mock embeddings."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-responsive-") as temporary:
        root=Path(temporary).resolve();source=root/"sources"
        page="<link href='main.css'><picture><source srcset='small.svg 400w, large.svg 800w'><img srcset='small.svg 1x, large.svg 2x'></picture>"
        fixtures={
            "alpha/small.svg":"<svg><title>PRIVATE_ASSET</title></svg>\n",
            "alpha/large.svg":"<svg/>\n",
            "alpha/invalid.svg":"<svg/>\n",
            "alpha/main.css":".a{background:image-set('small.svg' 1x type('image/svg+xml'),url('large.svg') 2x)}\n",
            "alpha/page.html":page,
            "alpha/preload.html":"<link rel='preload' as='image' imagesrcset='small.svg 1x, large.svg 2x'>\n",
            "alpha/blocked.html":"<img srcset='large.svg 2x'><base href='/deployed/'><style>.a{background:image-set('large.svg' 2x)}</style>\n",
            "alpha/invalid.html":"<img srcset='data:image/svg+xml,large.svg 1x, invalid.svg -1x'>\n",
            "beta/foreign.html":"<img srcset='../alpha/large.svg 2x'>\n",
            "alpha/image/svg+xml":"not an image\n",
        }
        for name,body in fixtures.items():
            path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ("alpha","beta"):
                sync=mcp.call("sync_sources",dict(path=str(source/project),wing=project));assert not sync.get("errors"),sync
            mcp.call("ingest_file",dict(path=str(source/"alpha/image/svg+xml"),wing="alpha"))
            mcp.call("index_code",{})
            def impact(name,**kw):
                return mcp.call("impact",dict(impact_file=str(source/"alpha"/name),impact_project="alpha",**kw))
            result=impact("large.svg")
            expected={str(source/"alpha/main.css"),str(source/"alpha/page.html"),str(source/"alpha/preload.html")}
            assert {r["path"] for r in result["items"]}==expected,result
            css=next(r for r in result["items"] if r["path"]==str(source/"alpha/main.css"))
            html=next(r for r in result["items"] if r["path"]==str(source/"alpha/page.html"))
            assert css["via"]["reference_kind"]=="css:image-set" and css["via"]["choice"]["descriptors"]==["2x"],css
            assert html["via"]["reference_kind"]=="html:source:srcset" and html["via"]["choice"]["descriptors"]==["800w"],html
            assert all(not r["via"]["choice"]["selection_proven"] for r in result["items"]),result
            assert impact("large.svg",impact_relations="calls")["items"]==[]
            assert impact("image/svg+xml")["target_found"] and impact("image/svg+xml")["items"]==[]
            assert impact("invalid.svg")["items"]==[]
            assert "PRIVATE_ASSET" not in json.dumps(result)
            dead=mcp.call("dead_code",dict(dead_code_view="files",dead_code_project="alpha"))
            protected={r["file"]["path"] for r in dead["protected"]}
            assert {str(source/"alpha/small.svg"),str(source/"alpha/large.svg")}<=protected,dead
            path=source/"alpha/page.html";path.write_text(page.replace("large.svg 2x","large.svg 3x"))
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            changed=impact("large.svg");diff_args=dict(before=result["index_rev"],after=changed["index_rev"],project="alpha")
            frozen=mcp.call("diff",diff_args)
            assert frozen["responsive_resource_relations_available"] and frozen["css_url_classification_comparable"],frozen
            for collection,descriptor in (("relations_removed","2x"),("relations_added","3x")):
                assert any(r.get("reference_kind")=="html:img:srcset" and r["choice"]["descriptors"]==[descriptor] for r in frozen[collection]),frozen
            path.write_text("\n\n"+path.read_text())
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            moved=impact("large.svg")
            diff=mcp.call("diff",dict(before=changed["index_rev"],after=moved["index_rev"],project="alpha"))
            assert diff["relations_added_total"]==0 and diff["relations_removed_total"]==0,diff
            assert mcp.call("diff",diff_args)==frozen
            large=source/"alpha/large.svg";large.write_text("<svg><title>changed</title></svg>\n")
            mcp.call("ingest_file",dict(path=str(large),wing="alpha"))
            stale=impact("large.svg");assert not stale["target_available"] and stale["items"]==[],stale
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            assert {r["path"] for r in impact("large.svg")["items"]}==expected
            report=dict(status="passed",versions=mcp.call("index_status",dict(project="alpha"))["versions"],checks=["picture_srcset","preload_imagesrcset","css_image_set_string_and_url","source_choice_evidence","data_url_commas","invalid_descriptors","mime_not_file","base_guard","project_guard","call_mode_separation","dead_file_guard","descriptor_diff","line_move_no_semantic_churn","frozen_diff","stale_exclusion_and_sync","no_source_bodies"],production_database_touched=False,source_applications_executed=False,browser_selection_proven=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+"\n")
            print(json.dumps(report))


if __name__=="__main__":main()
