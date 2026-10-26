#!/usr/bin/env python3
"""Actual stdio MCP C# qualification with temporary sources/DB and mock embeddings."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-csharp-") as temporary:
        root=Path(temporary).resolve();source=root/"sources"
        fixtures={
            "alpha/base.cs":'// café\nnamespace Lib; public class Base { public void Work(){ string q="PRIVATE_BODY"; } public static void Api(){} }\n',
            "alpha/client.cs":'using Lib; namespace App; public class Child : Base { public Child(){} public void Run(){ Work(); } }\n',
            "alpha/main.cs":'using App; using Alias=Lib.Base; namespace App; class Entry { static void Main(){ Child child=new Child(); child.Run(); Alias.Api(); } }\n',
            "alpha/guards.cs":'using App; class Guard { void Unknown(dynamic child){child.Run();} void Mutated(Child child){child=other;child.Run();} void Shadow(System.Action Api){Api();} }\n',
            "beta/base.cs":'namespace Lib; class Base { public void Work(){} public static void Api(){} }\n',
            "beta/main.cs":'namespace Lib; class Entry { static void Main(){ Base.Api(); } }\n',
        }
        for name,body in fixtures.items():
            path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ("alpha","beta"):
                sync=mcp.call("sync_sources",dict(path=str(source/project),wing=project));assert not sync.get("errors"),sync
            def symbols(name):return mcp.call("symbol",dict(path=str(source/"alpha"/name)))
            base=symbols("base.cs");assert not base["parse_errors"],base
            work=next(s for s in base["items"] if s["qualified_name"]=="Lib.Base.Work")
            base_class=next(s for s in base["items"] if s["qualified_name"]=="Lib.Base")
            client=symbols("client.cs");child=next(s for s in client["items"] if s["qualified_name"]=="App.Child");run=next(s for s in client["items"] if s["qualified_name"]=="App.Child.Run")
            main=next(s for s in symbols("main.cs")["items"] if s["qualified_name"]=="App.Entry.Main")
            callees=mcp.call("callees",dict(symbol=run["id"]));assert next(e for e in callees["items"] if e["reference"]["name"]=="Work")["candidates"]==[work["id"]],callees
            callers=mcp.call("callers",dict(symbol=work["id"]));assert callers["total"]==1 and callers["items"][0]["reference"]["caller"]==run["id"],callers
            calls=mcp.call("callees",dict(symbol=main["id"]));assert next(e for e in calls["items"] if e["reference"]["expression"]=="child.Run")["candidates"]==[run["id"]],calls
            assert next(e for e in calls["items"] if e["reference"]["expression"]=="Alias.Api")["candidates"],calls
            for symbol in symbols("guards.cs")["items"]:
                if symbol["kind"]=="method_declaration":
                    guards=mcp.call("callees",dict(symbol=symbol["id"]));assert all(not e["candidates"] for e in guards["items"]),guards
            entries=mcp.call("entrypoints",dict(path=str(source/"alpha/main.cs")));
            assert any(e["kind"]=="csharp_conventional_main_candidate" for e in entries["items"]),entries
            inheritance=mcp.call("inheritance",dict(symbol=child["id"]));assert inheritance["items"][0]["candidates"]==[base_class["id"]],inheritance
            assert inheritance["items"][0]["hierarchy_status"]!="complete_static",inheritance
            body=mcp.call("code",dict(symbol=work["id"]));expected=fixtures["alpha/base.cs"].encode()[work["start_byte"]:work["end_byte"]].decode();assert body["found"] and body["code"]==expected,body
            assert "PRIVATE_BODY" not in json.dumps([base,callees,callers,inheritance])
            impact=mcp.call("impact",dict(symbol=work["id"],impact_project="alpha"));assert {r["symbol_id"] for r in impact["items"]}=={run["id"],main["id"]},impact
            structural=mcp.call("slice",dict(symbol=main["id"],direction="forward"));assert structural["analysis_incomplete"],structural
            status=mcp.call("index_status",dict(project="alpha"));assert any(l["language"]=="csharp" and l["available"] and not l["compiler_semantics_verified"] for l in status["capabilities"]["languages"]),status
            full=mcp.call("index_code",dict(force_full_resolution=True));assert not full["revision_created"] and full["resolution"]["equivalent_to_existing"] is True,full
            path=source/"alpha/client.cs";path.write_text('namespace App; public class Child { public Child(){} public void Run(){} }\n')
            mcp.call("ingest_file",dict(path=str(path),wing="alpha"));assert mcp.call("callers",dict(symbol=work["id"]))["items"]==[]
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            after=mcp.call("impact",dict(symbol=work["id"],impact_project="alpha"));assert after["items"]==[],after
            diff=mcp.call("diff",dict(before=impact["index_rev"],after=after["index_rev"],project="alpha"));assert diff["csharp_relations_available"] and any(e.get("relation")=="csharp_base_type_candidate" for e in diff["relations_removed"]),diff
            report=dict(status="passed",versions=status["versions"],checks=["csharp_capability","namespaces_owners_symbols","constructor_and_typed_receivers","inherited_calls","alias_static_calls","project_isolation","dynamic_mutation_delegate_guards","main_candidate","exact_unicode_byte_body","metadata_body_separation","transitive_impact","partial_slice","cache_equivalence","stale_exclusion","source_sync_rebind","frozen_inheritance_diff"],source_code_executed=False,compiler_or_runtime_dispatch_proven=False,production_database_touched=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+"\n")
            print(json.dumps(report))


if __name__=="__main__":main()
