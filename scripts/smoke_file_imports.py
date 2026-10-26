#!/usr/bin/env python3
"""Real MCP qualification; isolated DB, static fixtures and mock embeddings."""
import json
from pathlib import Path
import sys
import tempfile
from mcp_smoke_client import McpClient


class StartupClient(McpClient):
    def request(self, method, params, timeout=60):
        # Cold dyld startup under concurrent local builds can exceed one minute.
        return super().request(method, params, timeout=180 if method == "initialize" else timeout)


def main():
    binary = sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp"
    with tempfile.TemporaryDirectory(prefix="rag-file-imports-") as temp:
        root = Path(temp).resolve()
        source = root / "sources"
        source.mkdir()
        fixtures = {"api.py": "VALUE = 1\n", "client.py": "from api import missing\n",
                    "effect.js": "console.log('PRIVATE_BODY');\n",
                    "entry.ts": "import './effect.js'; export {missing} from './effect.js';\n"}
        for name, body in fixtures.items():
            (source / name).write_text(body)
        with StartupClient(binary, root, source) as mcp:
            sync = mcp.call("sync_sources", dict(path=str(source), wing="alpha"))
            assert not sync.get("errors"), sync
            def impact(name, **kw):
                return mcp.call("impact", dict(impact_file=str(source/name), impact_project="alpha", **kw))
            result = impact("api.py")
            assert [r["path"] for r in result["items"]] == [str(source/"client.py")], result
            edge = result["items"][0]["via"]
            assert result["items"][0]["via_kind"] == "file_import_candidate", result
            assert edge["content_hash"] and edge["revision"] and not edge["runtime_import_proven"], edge
            assert impact("api.py", impact_relations="calls")["items"] == []
            js = impact("effect.js")
            assert [r["path"] for r in js["items"]] == [str(source/"entry.ts")], js
            assert "PRIVATE_BODY" not in json.dumps(js)
            dead = mcp.call("dead_code", dict(dead_code_view="files", dead_code_project="alpha"))
            assert str(source/"api.py") not in {r["file"]["path"] for r in dead["items"]}, dead
            (source/"client.py").write_text("VALUE = 2\n")
            mcp.call("ingest_file", dict(path=str(source/"client.py"), wing="alpha"))
            stale = impact("api.py")
            assert stale["items"] == [] and stale["stale_documents"], stale
            sync = mcp.call("sync_sources", dict(path=str(source), wing="alpha"))
            assert not sync.get("errors"), sync
            assert impact("api.py")["items"] == []
            dead = mcp.call("dead_code", dict(dead_code_view="files", dead_code_project="alpha"))
            assert str(source/"api.py") in {r["file"]["path"] for r in dead["items"]}, dead
            status = mcp.call("index_status", {})
            report = dict(status="passed", checks=["missing_export_file_edge", "side_effect_import", "reexport", "no_fabricated_calls", "dead_file_guard", "stale_exclusion", "sync_removes_edge", "dead_file_candidate_restored", "no_source_bodies"],
                          index_status=status, production_database_touched=False, source_applications_executed=False)
            if len(sys.argv)>2:
                Path(sys.argv[2]).write_text(json.dumps(report, indent=2)+"\n")
            print(json.dumps(report))


if __name__ == "__main__":
    main()
