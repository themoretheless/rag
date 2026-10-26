#!/usr/bin/env python3
"""Preview/apply Rust rename against disposable sources through actual MCP."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from mcp_smoke_client import McpClient


def main():
    binary = Path(sys.argv[1]).resolve()
    report = Path(sys.argv[2])
    with tempfile.TemporaryDirectory(prefix="rag-rename-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        (source / "src").mkdir(parents=True)
        files = {
            "Cargo.toml": '[package]\nname="rename-fixture"\nversion="0.1.0"\nedition="2021"\n',
            "src/lib.rs": 'pub mod api; pub mod client;\n// value remains in documentation\npub const MESSAGE: &str = "value";\n#[cfg(test)] mod tests { #[test] fn works(){let got=crate::api::value();assert_eq!(got, 7);} }\n',
            "src/api.rs": '// Unicode before identifier: привет\npub fn value()->i32{7}\n',
            "src/client.rs": 'pub fn run()->i32{crate::api::value()}\npub fn callback()->fn()->i32{crate::api::value}\n',
            "README.md": "value and compute are documentation, not Rust bindings.\n",
            "compose.yaml": "services: {value: {image: demo/compute:v1}}\n",
            "Dockerfile": "FROM scratch\n# value and compute are configuration text\n",
        }
        for path, text in files.items():
            (source / path).write_text(text)
        compiler = []
        def compile_fixture(label):
            run = subprocess.run(["cargo", "test", "--manifest-path", str(source / "Cargo.toml"), "--offline", "--target-dir", str(root / "cargo-target")], capture_output=True, text=True)
            assert run.returncode == 0, run.stderr + run.stdout
            assert "1 passed" in run.stdout, run.stdout
            compiler.append(dict(case=label, exit_code=run.returncode, stdout=run.stdout, stderr=run.stderr))
        compile_fixture("before")
        with McpClient(binary, root, source) as client:
            call = client.call
            synced = call("sync_sources", dict(path=str(source), wing="rename"))
            assert not synced.get("errors"), synced
            query = dict(symbol="value", new_name="compute", project="rename", path=str(source / "src/api.rs"))
            before = {p: (source / p).read_bytes() for p in files}
            preview = call("rename", query)
            assert preview["status"] == "preview", preview
            assert preview == call("rename", query), "preview must be deterministic"
            assert preview["edits_total"] == 4, preview
            assert len(preview["guards"]) == len(files), preview
            assert {p: (source / p).read_bytes() for p in files} == before, "preview mutated sources"
            for change in preview["changes"]:
                raw = Path(change["path"]).read_bytes()
                for edit in change["edits"]:
                    assert raw[edit["start_byte"]:edit["end_byte"]].decode() == edit["old_text"] == "value"
            wrong = client.request("tools/call", dict(name="apply_rename", arguments=dict(query=query, plan_token="wrong")), expect_error=True)
            assert wrong["error"]["code"] == -32602, wrong
            lib = source / "src/lib.rs"
            lib.write_bytes(before["src/lib.rs"] + b"// concurrent unrelated edit\n")
            refused = client.request("tools/call", dict(name="apply_rename", arguments=dict(query=query, plan_token=preview["plan_token"])), expect_error=True)
            assert refused["error"]["code"] == -32602, refused
            assert (source / "src/api.rs").read_bytes() == before["src/api.rs"]
            lib.write_bytes(before["src/lib.rs"])
            applied = call("apply_rename", dict(query=query, plan_token=preview["plan_token"]))
            assert applied["status"] == "applied" and applied["index_refresh_required"], applied
            assert '"value"' in lib.read_text() and "// value" in lib.read_text()
            assert "api::compute()" in lib.read_text()
            compile_fixture("after")
            journal = json.loads(Path(applied["journal"]).read_text())
            assert journal["status"] == "applied" and len(journal["files"]) == 3, journal
            for record in journal["files"]:
                old = Path(record["backup"]).read_bytes()
                relative = str(Path(record["path"]).relative_to(source))
                assert old == before[relative]
            stale = call("rename", query)
            assert stale["status"] == "blocked" and stale["plan_token"] is None, stale
            for path in files:
                call("ingest_file", dict(path=str(source / path), wing="rename"))
            call("index_code", {})
            refreshed = call("callees", dict(symbol="run", path=str(source / "src/client.rs")))
            assert len(refreshed["items"]) == 1 and len(refreshed["items"][0]["candidates"]) == 1, refreshed
            symbol = call("symbol", dict(symbol="compute", path=str(source / "src/api.rs")))
            invalid = client.request("tools/call", dict(name="rename", arguments=dict(query, dry_run=False)), expect_error=True)
            assert invalid["error"]["code"] == -32602
            evidence = dict(status="passed", transport="stdio MCP", binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                            index_format=call("index_status", dict(project="rename"))["versions"]["index_format"],
                            preview=preview, applied=applied, journal=journal, compiler=compiler, refreshed_calls=refreshed, refreshed_symbol=symbol,
                            fixture_sha256_before={p: hashlib.sha256(raw).hexdigest() for p, raw in before.items()},
                            fixture_sha256_after={p: hashlib.sha256((source / p).read_bytes()).hexdigest() for p in files},
                            production_database_touched=False,
                            checks=["deterministic_nonmutating_preview", "four_exact_declaration_call_test_spans", "unicode_byte_offsets", "wrong_token_refusal", "unedited_provider_guard_refusal", "reviewed_apply", "original_backups", "comments_strings_preserved", "before_after_compiler_test_execution", "stale_after_apply", "reingest_binding_refresh", "preview_rejects_apply_parameter"],
                            limitations=["bounded captured Rust graph; not compiler rename completeness", "no external/generated/dynamic consumer proof", "multi-file journal is not crash-atomic", "stdio uses local authority; HTTP role matrix is tested separately"])
            report.parent.mkdir(parents=True, exist_ok=True)
            report.write_text(json.dumps(evidence, indent=2) + "\n")
            print(json.dumps(dict(status="passed", report=str(report), edits=preview["edits_total"])))


if __name__ == "__main__":
    main()
