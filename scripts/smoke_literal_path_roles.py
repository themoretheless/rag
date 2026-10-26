#!/usr/bin/env python3
"""Actual MCP literal path role evidence. Temporary sources/DB and mock embeddings."""
import json
from pathlib import Path
import sys
import tempfile
from smoke_file_imports import StartupClient


def main():
    binary=sys.argv[1] if len(sys.argv)>1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-literal-roles-") as temporary:
        root=Path(temporary).resolve();source=root/"sources"
        fixtures={
            "alpha/php/Files.php":"<?php function work(){return 'needle';}\n",
            "alpha/php/Files2.php":"<?php function work(){return 'needle';}\n",
            "alpha/app.js":"// café\nconst ENDPOINT='php/Files2.php';\n",
            "beta/app.js":"const ENDPOINT='../alpha/php/Files.php';\n",
        }
        for name,body in fixtures.items():
            path=source/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_text(body)
        with StartupClient(binary,root,source) as mcp:
            for project in ("alpha","beta"):
                result=mcp.call("sync_sources",dict(path=str(source/project),wing=project));assert not result.get("errors"),result
            scope=dict(project="alpha",path_prefix=str(source/"alpha/php"))
            def search(options=None):
                args=dict(**scope,queries=["needle"])
                if options is not None:args["literal_mentions"]=options
                return mcp.call("search_multi",args)
            def duplicates():return mcp.call("duplicates",dict(**scope,min_similarity=1.0,max_members=1))
            before=search();member=before["families"][0]["members"][0]
            assert member["document"]["path"].endswith("Files2.php") and member["usage_score"]==2,member
            evidence=next(e for e in member["role_evidence"] if e["kind"]=="literal_text_path_mention_candidate")
            assert evidence["path"]==str(source/"alpha/app.js") and evidence["line"]==2 and not evidence["binding_proven"] and not evidence["runtime_usage_proven"],evidence
            start,end=evidence["byte_start"],evidence["byte_end"]
            assert fixtures["alpha/app.js"].encode()[start:end].decode()==evidence["needle"]=="php/Files2.php",evidence
            assert evidence["content_hash"] and evidence["target_content_hash"] and evidence["revision"]>0,evidence
            dup=duplicates();assert dup["groups"][0]["members"][0]["document"]["path"].endswith("Files2.php") and dup["groups"][0]["usage_score"]==2 and not dup["groups"][0]["deletion_safe"],dup
            assert before==search()
            disabled=search(dict(enabled=False));assert disabled["literal_path_analysis"]["status"]=="disabled" and disabled["families"][0]["usage_score"]==0,disabled
            capped=search(dict(max_bytes=1));assert not capped["literal_path_analysis"]["complete"] and not capped["literal_path_analysis"]["absence_proven"] and capped["families"][0]["usage_score"]==0,capped
            path=source/"alpha/app.js";path.write_text("const ENDPOINT='removed';\n")
            mcp.call("ingest_file",dict(path=str(path),wing="alpha"))
            stale=search();assert stale["families"][0]["usage_score"]==0 and stale["literal_path_analysis"]["sources_stale_unindexed_or_unqualified"]>0,stale
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"));assert search()["families"][0]["usage_score"]==0
            # A basename duplicated outside the selected target prefix is not a binding.
            other=source/"alpha/other/Files2.php";other.parent.mkdir();other.write_text("<?php echo 'needle';\n")
            path.write_text("const ENDPOINT='Files2.php';\n")
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            ambiguous=search();assert ambiguous["families"][0]["usage_score"]==0 and ambiguous["literal_path_analysis"]["ambiguous_basename_targets"]==1,ambiguous
            path.write_text("const ENDPOINT='php/Files2.php';\n")
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"));assert search()["families"][0]["usage_score"]==2
            for i in range(25):(source/f"alpha/ref{i:02}.js").write_text("const ENDPOINT='php/Files2.php'; // php/Files2.php\n")
            mcp.call("sync_sources",dict(path=str(source/"alpha"),wing="alpha"))
            counted=duplicates();member=counted["groups"][0]["members"][0]
            assert member["evidence_total"]==26 and len(member["evidence"])==20 and member["evidence_truncated"],member
            assert counted["literal_path_analysis"]["evidence_total"]==26 and counted["literal_path_analysis"]["evidence_retained"]==20,counted
            status=mcp.call("index_status",dict(project="alpha"))
            report=dict(status="passed",versions=status["versions"],checks=["literal_endpoint_constant","weaker_unbound_text_role","exact_unicode_byte_anchor","cross_project_exclusion","source_outside_target_prefix","search_and_duplicate_primary","deterministic_evidence","disabled_mode","decoded_byte_budget","no_absence_claim","stale_source_exclusion_before_sync","sync_replacement","project_wide_basename_ambiguity","qualified_path_disambiguation","unique_source_target_counts","20_of_26_literal_retention"],source_code_executed=False,production_database_touched=False,runtime_binding_or_deletion_safety_proven=False)
            if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+"\n")
            print(json.dumps(report))


if __name__=="__main__":main()
