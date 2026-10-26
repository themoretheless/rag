#!/usr/bin/env python3
"""Real MCP resource dependency checks in a temporary mock-embedding corpus."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary = sys.argv[1] if len(sys.argv)>1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-file-resources-") as temp:
        root=Path(temp).resolve()
        source=root/"sources"
        fixtures={
            "alpha/base.css":".base{color:red}\n",
            "alpha/main.css":"@import './base.css' screen;\n",
            "alpha/page.html":"<link href='./main.css?build=1&amp;mode=dev'><script src='./app.js'></script>\n",
            "alpha/app.js":"console.log('PRIVATE_BODY');\n",
            "alpha/blocked.html":"<link href='./base.css'><base href='/deployed/'><style>@import './base.css';</style>\n",
            "beta/foreign.html":"<link href='../alpha/base.css'>\n",
        }
        for name,body in fixtures.items():
            path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ("alpha","beta"):
                result=mcp.call("sync_sources",dict(path=str(source/project),wing=project))
                assert not result.get("errors"),result
            def impact(name,**kw):
                return mcp.call("impact",dict(impact_file=str(source/"alpha"/name),impact_project="alpha",**kw))
            result=impact("base.css")
            assert [r["path"] for r in result["items"]]==[str(source/"alpha/main.css"),str(source/"alpha/page.html")],result
            assert result["items"][1]["depth"]==2,result
            assert all(r["via_kind"]=="file_resource_candidate" for r in result["items"]),result
            assert all(r["via"]["content_hash"] and r["via"]["revision"] and not r["via"]["runtime_resource_proven"] for r in result["items"]),result
            assert result["items"][1]["via"]["target"]=="./main.css?build=1&mode=dev",result
            assert result["static_file_resource_candidates_included"],result
            assert impact("base.css",impact_relations="calls")["items"]==[]
            app=impact("app.js")
            assert [r["path"] for r in app["items"]]==[str(source/"alpha/page.html")],app
            assert "PRIVATE_BODY" not in json.dumps(app)
            shallow=impact("base.css",depth=1)
            assert shallow["depth_limited"] and len(shallow["items"])==1,shallow
            dead=mcp.call("dead_code",dict(dead_code_view="files",dead_code_project="alpha"))
            protected={r["file"]["path"]:r for r in dead["protected"]}
            for name in ("base.css","main.css","app.js"):
                assert "known_incoming_file_relation" in protected[str(source/"alpha"/name)]["reasons"],dead
            main=source/"alpha/main.css";main.write_text(".changed{color:blue}\n")
            mcp.call("ingest_file",dict(path=str(main),wing="alpha"))
            stale=impact("base.css")
            assert stale["items"]==[] and stale["stale_documents"],stale
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            refreshed=impact("base.css")
            assert refreshed["items"]==[] and refreshed["index_rev"]!=result["index_rev"],refreshed
            dead=mcp.call("dead_code",dict(dead_code_view="files",dead_code_project="alpha"))
            assert str(source/"alpha/base.css") not in {r["file"]["path"] for r in dead["protected"]},dead
            report=dict(status="passed",checks=["css_import","html_stylesheet","html_script","entity_query_fragment","transitive_impact","base_url_guard","project_guard","call_mode_separation","depth_limit","dead_file_protection","stale_bridge_exclusion","sync_replaces_resources","no_source_bodies"],
                        versions=mcp.call("index_status",{})["versions"],production_database_touched=False,source_applications_executed=False,runtime_resource_proven=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+"\n")
            print(json.dumps(report))


if __name__=="__main__":main()
