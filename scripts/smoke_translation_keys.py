#!/usr/bin/env python3
"""Real MCP translation-key syntax, immutable history and pinned placeholder oracle."""
import ast
import json
from pathlib import Path
import re
import sys
import tempfile
from mcp_smoke_client import McpClient
from smoke_file_imports import StartupClient


def reference_placeholders(text):
    path = Path("/tmp/ragraf-compare-20261023/src/ragraf/extract/php_ast.py")
    tree = ast.parse(path.read_text())
    nodes = [n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "placeholders_from_text" or isinstance(n, ast.Assign) and any(isinstance(t, ast.Name) and t.id == "PLACEHOLDER_RE" for t in n.targets)]
    namespace = dict(re=re)
    exec(compile(ast.Module(body=nodes, type_ignores=[]), str(path), "exec"), namespace)
    return set(namespace["placeholders_from_text"](text))


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-translation-keys-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        fixtures = {
            "alpha/view.html": "Привет %{reg.months} %{reg.months}\n@{Other-Key} {{reg.months}} {{not a key}}",
            "alpha/render.php": r'''<?php
function render() {
 Translate('reg.months'); \Ns\Tr("reg.months"); gettext('Mixed Key');
 Translate($key); Translate("$key"); Translate('reg.' . 'months');
 Translate(key: 'named'); Translate('escaped\\key'); Translate(...$args);
 $obj->Translate('not_function'); other('not_translation');
}
''',
            "alpha/inline.phtml": "<p>@{reg.months}</p>",
            "alpha/cap.html": "%{cap.key}" * 4100,
            "beta/view.html": "%{reg.months}",
        }
        fixtures["alpha/render.php"] = fixtures["alpha/render.php"].replace("gettext('Mixed Key');", "gettext('Mixed Key'); Translate('" + "界" * 120 + "');")
        for name, body in fixtures.items():
            path = source / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body)
        expected_placeholders = reference_placeholders(fixtures["alpha/view.html"])
        with StartupClient(binary, root, source) as mcp:
            for project in ("alpha", "beta"):
                sync = mcp.call("sync_sources", dict(path=str(source / project), wing=project))
                assert not sync.get("errors"), sync
            query = dict(key="reg.months", project="alpha")
            before = mcp.call("references", query)
            assert before["total"] == 6 and before["counts_by_kind"] == {"uses_key": 6}, before
            calls = [i for i in before["items"] if i["usage_kind"] == "php_translation_call"]
            assert len(calls) == 2 and all(i["from"]["kind"] == "symbol" and i["from"]["name"] == "render" for i in calls), before
            for item in before["items"]:
                proof = item["evidence"]
                raw = Path(proof["path"]).read_bytes()
                snippet = raw[proof["start_byte"]:proof["end_byte"]].decode()
                assert "reg.months" in snippet and proof["revision"] and proof["content_hash"] and proof["project"] == "alpha", item
                assert proof["line"] == raw[:proof["start_byte"]].count(b"\n") + 1, item
                assert not item["callee_binding_proven"] and not item["template_evaluation_proven"], item
            oracle_observed = set()
            for key in ("reg.months", "Other-Key"):
                facts = mcp.call("references", dict(key=key, project="alpha", path=str(source / "alpha/view.html")))
                oracle_observed |= {(key, i["evidence"]["line"]) for i in facts["items"]}
            assert oracle_observed == expected_placeholders, (oracle_observed, expected_placeholders)
            assert mcp.call("references", dict(key="Mixed Key", project="alpha"))["total"] == 1
            assert mcp.call("references", dict(key="界" * 120, project="alpha"))["total"] == 1
            assert mcp.call("references", dict(key="escaped\\key", project="alpha"))["total"] == 1
            assert mcp.call("references", dict(key="Reg.Months", project="alpha"))["total"] == 0
            for key in ("named", "not_function", "not_translation", "reg."):
                assert mcp.call("references", dict(key=key, project="alpha"))["total"] == 0, key
            limited = mcp.call("references", dict(query, limit=1))
            assert limited["total"] == 6 and limited["truncated"] and len(limited["items"]) == 1, limited
            ambiguous = mcp.call("references", dict(key="reg.months", path=str(source / "alpha/view.html")))
            assert ambiguous["needs_disambiguation"] and ambiguous["target_candidates_total"] == 2, ambiguous
            cap = mcp.call("references", dict(key="cap.key", project="alpha", limit=2))
            assert cap["total"] == 4096 and cap["truncated"] and cap["key_capture_coverage"]["limited_captures"] == 1, cap
            assert before == mcp.call("references", query)
            for args in [dict(key=""), dict(key="x" * 121), dict(key="界" * 121), dict(key="a\n"), dict(key="key", table="users"), dict(key="key", route="/x"), dict(key="key", symbol="render"), dict(key="key", reference_mode="lexical")]:
                McpClient.request(mcp, "tools/call", dict(name="references", arguments=args), expect_error=True)
            # Position-only moves preserve semantic relations and captured historical views.
            for name in ("alpha/view.html", "alpha/render.php"):
                path = source / name
                body = fixtures[name]
                if name.endswith(".php"):
                    body = body.replace("Translate('reg.months')", r'Translate("reg\x2Emonths")')
                path.write_text("\n\n" + body)
            sync = mcp.call("sync_sources", dict(path=str(source / "alpha"), wing="alpha"))
            assert not sync.get("errors"), sync
            moved = mcp.call("references", query)
            args = dict(before=before["index_rev"], after=moved["index_rev"], project="alpha")
            frozen = mcp.call("diff", args)
            assert frozen["translation_key_relations_available"], frozen
            assert not any(i["kind"] == "uses_key" for i in frozen["relations_added"] + frozen["relations_removed"]), frozen
            # Stale PHP facts disappear before sync, then the changed keys are published.
            path = source / "alpha/render.php"
            path.write_text(path.read_text().replace(r"reg\x2Emonths", "reg.days").replace("reg.months", "reg.days"))
            mcp.call("ingest_file", dict(path=str(path), wing="alpha"))
            stale = mcp.call("references", query)
            assert stale["total"] == 4 and stale["stale_documents"], stale
            sync = mcp.call("sync_sources", dict(path=str(source / "alpha"), wing="alpha"))
            assert not sync.get("errors"), sync
            changed = mcp.call("references", dict(key="reg.days", project="alpha"))
            assert changed["total"] == 2, changed
            delta = mcp.call("diff", dict(before=moved["index_rev"], after=changed["index_rev"], project="alpha"))
            assert any(i["kind"] == "uses_key" and i["target_key"] == "reg.days" for i in delta["relations_added"]), delta
            assert any(i["kind"] == "uses_key" and i["target_key"] == "reg.months" for i in delta["relations_removed"]), delta
            assert not delta["translation_key_comparison"]["complete_for_supported_documents"], delta  # cap.html is deliberately incomplete.
            assert mcp.call("diff", args) == frozen
            status = mcp.call("index_status", dict(project="alpha"))
            result = dict(status="passed", versions=status["versions"], reference_commit="075caf86dc2f6274b9c48487e521915bcdfeaf2c", reference_execution="only_placeholder_regex_and_pure_extractor_on_fixed_template", checks=["template_PHP_shared_key_target", "qualified_case_insensitive_translator_name_candidates", "PHP_owner_and_PHTML_file_evidence", "literal_key_case_and_space_identity", "120_unicode_characters_not_120_bytes", "dynamic_interpolation_concat_named_member_exclusion_and_static_escape_decoding", "UTF8_byte_line_anchors", "reference_placeholder_key_line_sets", "repeated_occurrences_preserved", "project_disambiguation_before_path", "independent_result_cap", "explicit_4096_occurrence_limit", "invalid_and_mixed_target_rejection", "position_and_equivalent_literal_spelling_history_stability", "stale_exclusion", "sync_replaces_keys", "immutable_historical_diff", "incomplete_capture_diff_exclusion"], production_database_touched=False, source_code_or_SQL_executed=False, translator_or_template_runtime_binding_proven=False, representative_quality_scale_or_full_parity_proven=False)
            if len(sys.argv) > 2:
                Path(sys.argv[2]).write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result))


if __name__ == "__main__":
    main()
