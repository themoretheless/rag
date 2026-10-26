#!/usr/bin/env python3
"""Real MCP Ruby syntax/owner/import qualification. No Ruby VM execution."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-ruby-") as temporary:
        root=Path(temporary).resolve();source=root/"sources"
        fixtures={
            "alpha/base.rb":"# café\nclass Base\n def work; 'PRIVATE_BODY'; end\n def self.api; 'API'; end\nend\n",
            "alpha/client.rb":"require_relative 'base'\nclass Child < Base\n def initialize; @value=1; end\n def run; work(); end\nend\n",
            "alpha/main.rb":"require_relative 'client'\ndef main\n child=Child.new\n child.run()\n Child.api()\nend\nmain()\n",
            "alpha/guards.rb":"require_relative 'client'\ndef parameter(child); child.run(); end\ndef reassigned\n child=Child.new\n child=other\n child.run()\nend\ndef item; 1; end\ndef tuple_values\n item, rest=[1,2]\n item\n item()\nend\ndef loop_values\n for item in [1]\n  item\n end\nend\ndef rescue_values\n begin\n  raise 'error'\n rescue StandardError => item\n  item\n end\nend\nclass Dynamic < Child\n include Mixin\n def run; work(); end\nend\n",
            "beta/base.rb":"class Base\n def work; 'FOREIGN'; end\nend\n",
            "beta/main.rb":"require_relative 'base'\ndef main; Base.new.work(); end\nmain()\n",
        }
        for name,body in fixtures.items():
            path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ("alpha","beta"):
                sync=mcp.call("sync_sources",dict(path=str(source/project),wing=project));assert not sync.get("errors"),sync
            def symbols(name):return mcp.call("symbol",dict(path=str(source/"alpha"/name)))
            base=symbols("base.rb");assert not base["parse_errors"],base
            work=next(s for s in base["items"] if s["qualified_name"]=="Base#work")
            base_class=next(s for s in base["items"] if s["kind"]=="class")
            client=symbols("client.rb");run=next(s for s in client["items"] if s["qualified_name"]=="Child#run");child=next(s for s in client["items"] if s["kind"]=="class")
            main=next(s for s in symbols("main.rb")["items"] if s["qualified_name"]=="main")
            callees=mcp.call("callees",dict(symbol=run["id"]));call=next(e for e in callees["items"] if e["reference"]["name"]=="work")
            assert call["candidates"]==[work["id"]] and call["resolution"]=="resolved_static",callees
            callers=mcp.call("callers",dict(symbol=work["id"]))
            assert callers["total"]==1 and callers["items"][0]["reference"]["caller"]==run["id"],callers
            main_calls=mcp.call("callees",dict(symbol=main["id"]))
            assert next(e for e in main_calls["items"] if e["reference"]["expression"]=="child.run")["candidates"]==[run["id"]],main_calls
            entries=mcp.call("entrypoints",dict(path=str(source/"alpha/client.rb")))
            assert any(e["kind"]=="ruby_constructor_hook_candidate" for e in entries["items"]),entries
            inheritance=mcp.call("inheritance",dict(symbol=child["id"]))
            assert inheritance["items"][0]["candidates"]==[base_class["id"]],inheritance
            assert inheritance["items"][0]["hierarchy_status"]!="complete_static",inheritance
            for symbol in symbols("guards.rb")["items"]:
                if symbol["kind"] in ("method","singleton_method"):
                    calls=mcp.call("callees",dict(symbol=symbol["id"]))
                    assert all(not e["candidates"] for e in calls["items"] if e["reference"]["name"] in ("run","work")),calls
                    item_calls=[e for e in calls["items"] if e["reference"]["name"]=="item"]
                    if symbol["name"]=="tuple_values": assert len(item_calls)==1 and item_calls[0]["candidates"],calls
                    elif symbol["name"] in ("loop_values","rescue_values"): assert item_calls==[],calls
            body=mcp.call("code",dict(symbol=work["id"]))
            expected=fixtures["alpha/base.rb"].encode()[work["start_byte"]:work["end_byte"]].decode()
            assert body["found"] and body["code"]==expected,body
            assert "PRIVATE_BODY" not in json.dumps([base,callers,callees,inheritance])
            structural=mcp.call("slice",dict(symbol=main["id"],direction="forward"))
            assert structural["analysis_incomplete"] and structural["detail"]["structural_only"],structural
            impact=mcp.call("impact",dict(impact_file=str(source/"alpha/base.rb"),impact_project="alpha"))
            assert {r["path"] for r in impact["items"]}=={str(source/"alpha/client.rb"),str(source/"alpha/main.rb"),str(source/"alpha/guards.rb")},impact
            status=mcp.call("index_status",dict(project="alpha"))
            assert any(l["language"]=="ruby" and l["available"] and not l["compiler_semantics_verified"] for l in status["capabilities"]["languages"]),status
            full=mcp.call("index_code",dict(force_full_resolution=True))
            assert not full["revision_created"] and full["resolution"]["equivalent_to_existing"] is True,full
            path=source/"alpha/client.rb";path.write_text("class Child\n def run; 0; end\nend\n")
            mcp.call("ingest_file",dict(path=str(path),wing="alpha"))
            assert mcp.call("callers",dict(symbol=work["id"]))["items"]==[]
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            refreshed=mcp.call("impact",dict(impact_file=str(source/"alpha/base.rb"),impact_project="alpha"))
            assert refreshed["items"]==[],refreshed
            diff=mcp.call("diff",dict(before=impact["index_rev"],after=refreshed["index_rev"],project="alpha"))
            assert diff["ruby_relations_available"] and any(r.get("import_kind")=="ruby_require_relative_candidate" for r in diff["relations_removed"]),diff
            report=dict(status="passed",versions=status["versions"],checks=["ruby_capability","module_class_instance_singleton_definitions","owner_local_constructor_calls","required_superclass_methods","project_isolation","parameter_reassignment_mixin_guards","parallel_loop_rescue_local_bindings","constructor_hook_and_instance_state","exact_unicode_byte_body","metadata_body_separation","partial_slice","require_relative_file_impact","full_cache_equivalence","stale_exclusion","sync_rebind","frozen_import_diff"],production_database_touched=False,ruby_code_executed=False,runtime_dispatch_proven=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+"\n")
            print(json.dumps(report))


if __name__=="__main__":main()
