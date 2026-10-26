#!/usr/bin/env python3
"""Actual MCP Perl index qualification. No Perl interpreter or production DB."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-perl-") as temporary:
        root=Path(temporary).resolve();source=root/"sources"
        fixtures={
            "alpha/renamed.pm":"# café\npackage Base;our %counts=(key=>1);sub work {my $q='SELECT * FROM users';$counts{key};'PRIVATE_BODY';}\n",
            "alpha/child.pm":"package Child;use parent 'Base';sub run {Child->work();}\n",
            "alpha/main.pl":"use Child ();sub main {Child->run();Base::work();}\nmain();\n",
            "alpha/guard.pm":"package Guard;use External;sub work {1}sub run {work();Guard->work();$object->work();}\n",
            "beta/base.pm":"package Base;sub work {'FOREIGN';}\n",
            "beta/main.pl":"sub main {Base::work();}\nmain();\n",
        }
        for name,body in fixtures.items():
            path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ("alpha","beta"):
                sync=mcp.call("sync_sources",dict(path=str(source/project),wing=project));assert not sync.get("errors"),sync
            def symbols(name):return mcp.call("symbol",dict(path=str(source/"alpha"/name)))
            base=symbols("renamed.pm");assert not base["parse_errors"],base
            work=next(s for s in base["items"] if s["qualified_name"]=="Base::work");base_package=next(s for s in base["items"] if s["qualified_name"]=="Base")
            variable=next(s for s in base["items"] if s["name"]=="%counts")
            client=symbols("child.pm");run=next(s for s in client["items"] if s["qualified_name"]=="Child::run");child=next(s for s in client["items"] if s["qualified_name"]=="Child")
            main=next(s for s in symbols("main.pl")["items"] if s["qualified_name"]=="main::main")
            callees=mcp.call("callees",dict(symbol=run["id"]));assert next(e for e in callees["items"] if e["reference"]["name"]=="work")["candidates"]==[work["id"]],callees
            callers=mcp.call("callers",dict(symbol=work["id"]));assert callers["total"]==2 and {e["reference"]["caller"] for e in callers["items"]}=={run["id"],main["id"]},callers
            references=mcp.call("references",dict(symbol=variable["id"]));assert references["total"]==1 and references["items"][0]["name"]=="%counts" and references["items"][0]["expression"]=="$counts",references
            for symbol in symbols("guard.pm")["items"]:
                if symbol["kind"]=="subroutine_declaration_statement":
                    guards=mcp.call("callees",dict(symbol=symbol["id"]));assert all(not e["candidates"] for e in guards["items"]),guards
            body=mcp.call("code",dict(symbol=work["id"]));expected=fixtures["alpha/renamed.pm"].encode()[work["start_byte"]:work["end_byte"]].decode();assert body["found"] and body["code"]==expected,body
            assert "PRIVATE_BODY" not in json.dumps([base,callees,callers,references])
            inherited=mcp.call("inheritance",dict(symbol=child["id"]));assert inherited["items"][0]["candidates"]==[base_package["id"]],inherited
            assert inherited["items"][0]["hierarchy_status"]!="complete_static",inherited
            file_impact=mcp.call("impact",dict(impact_file=str(source/"alpha/renamed.pm"),impact_project="alpha"));assert {e["path"] for e in file_impact["items"]}=={str(source/"alpha/child.pm"),str(source/"alpha/main.pl")},file_impact
            table=mcp.call("impact",dict(impact_table="users",impact_project="alpha"));assert table["direct_owner_symbols"]==1 and {e["symbol_id"] for e in table["items"]}=={run["id"],main["id"]},table
            structural=mcp.call("slice",dict(symbol=main["id"],direction="forward"));assert structural["analysis_incomplete"],structural
            status=mcp.call("index_status",dict(project="alpha"));assert any(l["language"]=="perl" and l["available"] and not l["compiler_semantics_verified"] for l in status["capabilities"]["languages"]),status
            full=mcp.call("index_code",dict(force_full_resolution=True));assert not full["revision_created"] and full["resolution"]["equivalent_to_existing"] is True,full
            path=source/"alpha/child.pm";path.write_text('package Child;sub run {1;}\n')
            mcp.call("ingest_file",dict(path=str(path),wing="alpha"));assert mcp.call("callers",dict(symbol=work["id"]))["total"]==1
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            after=mcp.call("impact",dict(impact_file=str(source/"alpha/renamed.pm"),impact_project="alpha"));assert {e["path"] for e in after["items"]}=={str(source/"alpha/main.pl")},after
            diff=mcp.call("diff",dict(before=file_impact["index_rev"],after=after["index_rev"],project="alpha"));assert diff["perl_relations_available"] and any(e.get("relation")=="perl_parent_package_candidate" for e in diff["relations_removed"]),diff
            report=dict(status="passed",versions=status["versions"],checks=["perl_capability","packages_subroutines_variables","qualified_and_parent_method_calls","project_isolation","normalized_hash_variable_reference","dynamic_import_object_guards","exact_unicode_body","metadata_body_separation","inheritance_candidates","package_identity_file_impact","static_sql_owner_table_impact","partial_slice","full_resolution_equivalence","stale_exclusion","source_sync","frozen_parent_diff"],perl_code_executed=False,INC_or_runtime_dispatch_proven=False,production_database_touched=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+"\n")
            print(json.dumps(report))


if __name__=="__main__":main()
