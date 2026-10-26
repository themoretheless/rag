#!/usr/bin/env python3
"""Actual MCP PowerShell candidates using temporary roots/DBs and mock embeddings."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-powershell-") as temporary:
        root=Path(temporary).resolve();source=root/"sources"
        fixtures={
            "alpha/base.psm1":"# café\nclass Base { [int] Work(){ # PRIVATE_BODY\n return 1 } static [int] Read(){return 2} }\n",
            "alpha/child.psm1":"using module './base.psm1'\nclass Child : Base { [int] Run(){return $this.Work()} static [int] Call(){return [Base]::Read()} }\n",
            "alpha/functions.ps1":"function Get-Work {1}\n",
            "alpha/main.ps1":'using module "./child.psm1"\n. "$PSScriptRoot/functions.ps1"\nfunction Invoke-Main {GET-WORK; [Child]::Call(); & $unknown}\nInvoke-Main\n',
            "alpha/guard.ps1":"function Get-Work {1}\nSet-Alias Get-Work Other\nfunction Guard {Get-Work; & $unknown; $object.Work()}\n",
            "beta/base.psm1":"class Base { [int] Work(){return 3} static [int] Read(){return 4} }\n",
            "beta/main.ps1":"using module './base.psm1'\nfunction Invoke-Main {[Base]::Read()}\n",
        }
        for name,body in fixtures.items():
            path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ("alpha","beta"):
                sync=mcp.call("sync_sources",dict(path=str(source/project),wing=project));assert not sync.get("errors"),sync
            def symbols(name):return mcp.call("symbol",dict(path=str(source/"alpha"/name)))
            base=symbols("base.psm1");assert not base["parse_errors"],base
            work=next(s for s in base["items"] if s["qualified_name"]=="base::Work");read=next(s for s in base["items"] if s["qualified_name"]=="base::Read");base_type=next(s for s in base["items"] if s["name"]=="Base")
            child_symbols=symbols("child.psm1");child=next(s for s in child_symbols["items"] if s["name"]=="Child");run=next(s for s in child_symbols["items"] if s["qualified_name"]=="child::Run");call=next(s for s in child_symbols["items"] if s["qualified_name"]=="child::Call")
            main=next(s for s in symbols("main.ps1")["items"] if s["name"]=="Invoke-Main")
            helper=next(s for s in symbols("functions.ps1")["items"] if s["name"]=="Get-Work")
            calls=mcp.call("callees",dict(symbol=run["id"]));assert next(e for e in calls["items"] if e["reference"]["name"]=="work")["candidates"]==[work["id"]],calls
            callers=mcp.call("callers",dict(symbol=read["id"]));assert callers["total"]==1 and callers["items"][0]["reference"]["caller"]==call["id"],callers
            main_calls=mcp.call("callees",dict(symbol=main["id"]));assert next(e for e in main_calls["items"] if e["reference"]["expression"]=="get-work")["candidates"]==[helper["id"]],main_calls
            assert next(e for e in main_calls["items"] if e["reference"]["expression"]=="child::call")["candidates"]==[call["id"]],main_calls
            assert all(not e["candidates"] for e in main_calls["items"] if e["reference"]["name"]=="<dynamic-command>"),main_calls
            guards=symbols("guard.ps1")
            for symbol in guards["items"]:
                if symbol["kind"]=="function_statement":
                    result=mcp.call("callees",dict(symbol=symbol["id"]));assert all(not e["candidates"] for e in result["items"]),result
            inherited=mcp.call("inheritance",dict(symbol=child["id"]));assert inherited["items"][0]["candidates"]==[base_type["id"]],inherited
            assert inherited["items"][0]["hierarchy_status"]!="complete_static",inherited
            body=mcp.call("code",dict(symbol=work["id"]));expected=fixtures["alpha/base.psm1"].encode()[work["start_byte"]:work["end_byte"]].decode();assert body["found"] and body["code"]==expected,body
            assert "PRIVATE_BODY" not in json.dumps([base,child_symbols,main_calls,callers,inherited])
            impact=mcp.call("impact",dict(symbol=read["id"],impact_project="alpha"));assert {e["symbol_id"] for e in impact["items"]}=={call["id"],main["id"]},impact
            files=mcp.call("impact",dict(impact_file=str(source/"alpha/base.psm1"),impact_project="alpha"));assert {e["path"] for e in files["items"]}=={str(source/"alpha/child.psm1"),str(source/"alpha/main.ps1")},files
            structural=mcp.call("slice",dict(symbol=main["id"],direction="forward"));assert structural["analysis_incomplete"],structural
            entries=mcp.call("entrypoints",dict(path=str(source/"alpha/main.ps1")));assert any(e["kind"]=="powershell_conventional_main_candidate" for e in entries["items"]),entries
            status=mcp.call("index_status",dict(project="alpha"));assert any(l["language"]=="powershell" and l["available"] and not l["compiler_semantics_verified"] for l in status["capabilities"]["languages"]),status
            full=mcp.call("index_code",dict(force_full_resolution=True));assert not full["revision_created"] and full["resolution"]["equivalent_to_existing"] is True,full
            path=source/"alpha/child.psm1";path.write_text('class Child {[int] Run(){return 1} static [int] Call(){return 2}}\n')
            mcp.call("ingest_file",dict(path=str(path),wing="alpha"));assert mcp.call("callers",dict(symbol=read["id"]))["items"]==[]
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            after=mcp.call("impact",dict(impact_file=str(source/"alpha/base.psm1"),impact_project="alpha"));assert after["items"]==[],after
            diff=mcp.call("diff",dict(before=files["index_rev"],after=after["index_rev"],project="alpha"));assert diff["powershell_relations_available"] and any(e.get("relation")=="powershell_base_type_candidate" for e in diff["relations_removed"]),diff
            report=dict(status="passed",versions=status["versions"],checks=["powershell_capability","functions_classes_methods","case_insensitive_commands","literal_dot_source_functions","using_module_type_scope","instance_and_static_inherited_calls","project_isolation","alias_dynamic_guards","exact_unicode_body","body_comment_metadata_separation","transitive_symbol_and_file_impact","partial_slice","entrypoint_candidates","full_resolution_equivalence","stale_exclusion","sync_rebind","frozen_base_and_load_diff"],powershell_code_executed=False,runspace_or_runtime_dispatch_proven=False,production_database_touched=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+"\n")
            print(json.dumps(report))


if __name__=="__main__":main()
