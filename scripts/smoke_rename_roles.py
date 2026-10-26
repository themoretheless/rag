#!/usr/bin/env python3
"""Verify rename discovery and authorization over real authenticated HTTP MCP."""
import hashlib
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request


def main():
    binary = Path(sys.argv[1]).resolve()
    report = Path(sys.argv[2])
    with tempfile.TemporaryDirectory(prefix="rag-rename-roles-") as temporary:
        root = Path(temporary).resolve()
        source = root / "sources"
        source.mkdir()
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        tokens = {role: secrets.token_hex(32) for role in ("read", "write", "admin")}
        env = {k: v for k, v in os.environ.items() if not k.startswith("RAG_")}
        env.update(RAG_DB_PATH=str(root / "test.duckdb"), RAG_INGEST_ROOTS=str(source), RAG_TOOLS="full",
                   RAG_EMBEDDING_PROVIDER="mock", RAG_EMBEDDING_DIMS="16", RAG_LLM_ENABLED="false",
                   RAG_HTTP_BIND=f"127.0.0.1:{port}", **{f"RAG_HTTP_{role.upper()}_TOKEN": token for role, token in tokens.items()})
        results = {}
        with (root / "stderr.log").open("w") as stderr:
            process = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=stderr, env=env)
            try:
                def request(method, params, role):
                    data = json.dumps(dict(jsonrpc="2.0", id=1, method=method, params=params)).encode()
                    req = urllib.request.Request(f"http://127.0.0.1:{port}/mcp", data=data,
                                                 headers={"Content-Type": "application/json", "Accept": "application/json, text/event-stream", "Authorization": f"Bearer {tokens[role]}"})
                    with urllib.request.urlopen(req, timeout=30) as response:
                        if "application/json" in response.headers.get("Content-Type", ""):
                            return json.load(response)
                        for line in response:
                            if line.startswith(b"data:"):
                                value = json.loads(line[5:])
                                if value.get("id") == 1:
                                    return value
                    raise AssertionError("HTTP MCP response missing")
                deadline = time.monotonic() + 45
                while True:
                    try:
                        request("tools/list", {}, "read")
                        break
                    except (OSError, urllib.error.URLError):
                        assert process.poll() is None, (root / "stderr.log").read_text()
                        assert time.monotonic() < deadline, "HTTP MCP startup timeout"
                        time.sleep(.1)
                query = dict(symbol="absent", new_name="changed", project="fixture")
                for role in tokens:
                    discovered = request("tools/list", {}, role)
                    names = {tool["name"] for tool in discovered["result"]["tools"]}
                    assert "rename" in names
                    assert ("apply_rename" in names) == (role == "admin"), role
                    preview = request("tools/call", dict(name="rename", arguments=query), role)
                    assert "error" not in preview, preview
                    applied = request("tools/call", dict(name="apply_rename", arguments=dict(query=query, plan_token="unreviewed")), role)
                    assert "error" in applied, applied
                    if role != "admin":
                        assert applied["error"].get("data", {}).get("code") == "FORBIDDEN", applied
                    else:
                        assert "fresh preview" in applied["error"]["message"], applied
                    results[role] = dict(preview_discovered=True, apply_discovered="apply_rename" in names,
                                         preview_allowed=True, apply_error=applied["error"])
            finally:
                process.stdin.close()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.terminate()
                    process.wait(timeout=10)
        evidence = dict(status="passed", transport="authenticated HTTP MCP", binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                        roles=results, production_database_touched=False, source_mutations=0)
        report.parent.mkdir(parents=True, exist_ok=True)
        report.write_text(json.dumps(evidence, indent=2) + "\n")
        print(json.dumps(dict(status="passed", roles=list(results), report=str(report))))


if __name__ == "__main__":
    main()
