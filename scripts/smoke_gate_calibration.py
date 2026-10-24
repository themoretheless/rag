#!/usr/bin/env python3
"""Exercise the index-usage gate, hook adapter and threshold calibration through the real stdio MCP binary.

Usage: python3 scripts/smoke_gate_calibration.py [path/to/rag-mcp] [report.json]
Never uses the user's database, watcher or external embedding/LLM service.
"""
import json
import os
from pathlib import Path
import queue
import subprocess
import socket
import urllib.request
import urllib.error
import sys
import tempfile
import threading
import time


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp").resolve()
    report = Path(sys.argv[2]) if len(sys.argv) > 2 else None
    hook_binary = binary.with_name("rag_gate")
    with tempfile.TemporaryDirectory(prefix="rag-gate-calibration-") as temporary:
        root = Path(temporary)
        source = root / "sources"
        source.mkdir()
        (source / "main.css").write_text("/* .fake {} */\n.live:hover {color:red}\n.unused {}\n")
        page = source / "page.html"
        page.write_text('<html><body><p class="live missing">Visible.</p></body></html>')
        policy = source / "policy.php"
        policy.write_text("<?php function policy($role) { if ($role === 'admin') return true; return false; } function handler() { return policy('employee'); }")
        env = {key: value for key, value in os.environ.items() if not key.startswith("RAG_")}
        env.update(RAG_DB_PATH=str(root / "test.duckdb"), RAG_INGEST_ROOTS=str(source),
                   RAG_EMBEDDING_PROVIDER="mock", RAG_EMBEDDING_DIMS="16",
                   RAG_LLM_ENABLED="false", RAG_TOOLS="full")
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        write_token, read_token = "w" * 48, "r" * 48
        env.update(RAG_HTTP_BIND=f"127.0.0.1:{port}", RAG_HTTP_WRITE_TOKEN=write_token, RAG_HTTP_READ_TOKEN=read_token)
        with (root / "stderr.log").open("w") as stderr:
            process = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                       stderr=stderr, text=True, env=env)
            incoming = queue.Queue()
            def receive():
                for line in process.stdout:
                    incoming.put(json.loads(line))
                incoming.put(None)
            threading.Thread(target=receive, daemon=True).start()
            request_id = 0
            def request(method, params):
                nonlocal request_id
                request_id += 1
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", id=request_id, method=method, params=params)) + "\n")
                process.stdin.flush()
                deadline = time.monotonic() + 60
                while True:
                    response = incoming.get(timeout=max(0.01, deadline - time.monotonic()))
                    assert response is not None, "MCP process exited"
                    if response.get("id") == request_id:
                        assert "error" not in response, response
                        return response["result"]
            def call(name, arguments):
                result = request("tools/call", dict(name=name, arguments=arguments))
                assert not result.get("isError"), result
                return json.loads(next(c["text"] for c in result["content"] if c["type"] == "text"))
            def http_rpc(name, arguments, token=write_token, scope=None):
                params = dict(name=name, arguments=arguments)
                if scope is not None:
                    params["_meta"] = {"rag/task_session": scope}
                payload = dict(jsonrpc="2.0", id=101, method="tools/call", params=params)
                req = urllib.request.Request(f"http://127.0.0.1:{port}/mcp", data=json.dumps(payload).encode(), headers={
                    "Content-Type":"application/json", "Accept":"application/json, text/event-stream", "Authorization":f"Bearer {token}"})
                with urllib.request.urlopen(req, timeout=15) as response:
                    if "application/json" in response.headers.get("Content-Type", ""):
                        return json.load(response)
                    for line in response:
                        if line.startswith(b"data:"):
                            data = json.loads(line[5:].strip())
                            if data.get("id") == 101:
                                return data
                raise AssertionError("HTTP MCP returned no result")
            def http_call(name, arguments, token=write_token, scope=None):
                result = http_rpc(name, arguments, token, scope)
                assert "error" not in result, result
                value = result["result"]
                assert not value.get("isError"), value
                return json.loads(next(c["text"] for c in value["content"] if c["type"] == "text"))
            try:
                init = request("initialize", dict(protocolVersion="2024-11-05", capabilities={}, clientInfo=dict(name="gate-calibration-smoke", version="1")))
                process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                process.stdin.flush()
                tools = request("tools/list", {})
                names = {tool["name"] for tool in tools["tools"]}
                assert {"gate", "calibrate", "calibration_reports", "task_set", "symbol"} <= names
                sync = call("sync_sources", dict(path=str(source), wing="fixture"))
                assert not sync.get("errors"), sync
                label = call("task_set", dict(name="gate audit"))
                session = label["session_id"]
                marker = root / "must-not-exist"
                payload = dict(tool_name="Bash", tool_input=dict(command=f"rg SECRET_LITERAL > '{marker}'"), cwd=str(source), session_id=session)
                warn = call("gate", dict(payload=payload))
                assert warn["verdict"] == "allow" and warn["rule"] == "index_unused", warn
                assert warn["facts"]["fresh"] is True and warn["facts"]["index_used"] is False
                fake = call("gate", dict(payload=dict(tool_name="ragraf_symbol", session_id=session)))
                assert fake["rule"] == "index_call_not_completion", fake
                first = call("gate", dict(payload=payload, mode="ask"))
                second = call("gate", dict(payload=payload, mode="ask"))
                assert first["verdict"] == "ask" and second["verdict"] == "deny"
                call("symbol", dict(symbol="policy"))
                used = call("gate", dict(payload=payload, mode="deny"))
                assert used["verdict"] == "allow" and used["rule"] == "index_used", used
                original_css = (source / "main.css").read_text()
                (source / "main.css").write_text(original_css.replace("red", "tan"))
                stale = call("gate", dict(payload=payload, mode="deny"))
                assert stale["verdict"] == "deny" and stale["rule"] == "stale_index", stale
                assert not marker.exists()
                stats = call("task_stats", {})
                assert stats["gate"]["decisions"] == 6 and stats["gate"]["counters"]["deny"] == 2, stats
                assert "SECRET_LITERAL" not in json.dumps(stats)
                call("gate",dict(payload=dict(hook_event_name="SessionStart",session_id=session)))
                reset=call("gate",dict(payload=payload,mode="ask"))
                assert reset["verdict"] == "ask" and reset["facts"]["index_used"] is False, reset

                (source / "main.css").write_text(original_css)
                forbidden = http_rpc("gate", dict(payload=payload), token=read_token)
                assert "error" in forbidden, forbidden
                forbidden = http_rpc("calibrate", dict(), token=read_token)
                assert "error" in forbidden, forbidden
                sample = dict(training=[dict(id="train-pos",score=0.9,relevant=True),dict(id="train-neg",score=0.1,relevant=False)],
                              validation=[dict(id="hold-pos",score=0.95,relevant=True),dict(id="hold-neg",score=0.05,relevant=False)])
                fitted = call("calibrate", dict(samples=sample))
                assert fitted["selected_threshold"] == 0.9 and fitted["status"] == "exploratory_small_sample", fitted
                assert fitted["applied"] is False and fitted["production_qualified"] is False
                unlabelled = call("calibrate", dict(samples=dict(sample_size=500,threshold=0.5)))
                assert unlabelled["measured_sample_size"] == 0 and unlabelled["status"] == "recorded_unvalidated"
                assert unlabelled["params"]["threshold"] == 0.5
                reports = http_call("calibration_reports", dict(limit=1), token=read_token)
                assert reports["items"][0]["id"] == unlabelled["id"] and reports["truncated"]
                older = call("calibration_reports", dict(before_seq=reports["next_before_seq"]))
                assert older["items"][0]["id"] == fitted["id"]
                overlap = dict(sample)
                overlap["validation"] = [dict(id="train-pos",score=0.7,relevant=True)]
                try:
                    call("calibrate", dict(samples=overlap))
                except AssertionError:
                    pass
                else:
                    raise AssertionError("cross-split IDs should be rejected")
                hook_payload = dict(payload, session_id="hook-session")
                hook = subprocess.run([str(hook_binary),"--mode","ask"], input=json.dumps(hook_payload), text=True, capture_output=True, env=env, timeout=30)
                assert hook.returncode == 0, hook.stderr
                output = json.loads(hook.stdout)
                assert output["hookSpecificOutput"]["permissionDecision"] == "ask", output
                hook = subprocess.run([str(hook_binary),"--mode","ask"], input=json.dumps(hook_payload), text=True, capture_output=True, env=env, timeout=30)
                assert hook.returncode == 2 and hook.stderr.strip(), (hook.returncode,hook.stdout,hook.stderr)
                inspect = subprocess.run([str(hook_binary),"--mode","deny","--json"], input=json.dumps(hook_payload), text=True, capture_output=True, env=env, timeout=30)
                inspect = json.loads(inspect.stdout)
                assert inspect["hook_backend"] == "loopback_http" and inspect["verdict"] == "deny", inspect
                assert not marker.exists()
                # Verify the same adapter can use the file directly after its writer exits.
                process.stdin.close()
                process.wait(timeout=10)
                offline = dict(env)
                offline.pop("RAG_HTTP_BIND",None)
                inspect = subprocess.run([str(hook_binary),"--mode","deny","--json"], input=json.dumps(hook_payload), text=True, capture_output=True, env=offline, timeout=30)
                inspect = json.loads(inspect.stdout)
                if inspect.get("hook_backend")!="direct_database":
                    probe = subprocess.run([str(binary)], input="", text=True, capture_output=True, env=offline, timeout=15)
                    raise AssertionError((inspect, probe.returncode, probe.stderr[-6000:]))
                assert inspect.get("hook_backend") == "direct_database" and inspect["verdict"] == "deny", inspect
                assert not marker.exists()
                evidence = dict(status="passed", transport="stdio, authenticated loopback HTTP MCP and actual hook executable", protocol=init["protocolVersion"],
                    embedding="mock", external_database=False, tools_verified=["gate", "calibrate", "calibration_reports", "task_stats"],
                    checks=["tool_discovery", "warn_default", "fake_index_call_gives_no_credit", "SessionStart_resets_usage_and_escalation", "ask_then_deny", "real_query_allows", "same_length_live_change_denies", "task_scoped_gate_counts",
                            "raw_commands_not_in_stats", "read_role_cannot_mutate_gate_or_calibration", "holdout_metrics_and_small_sample_status", "unlabelled_assertions_not_measurements", "read_role_can_read_reports", "stable_report_cursor", "overlapping_IDs_rejected",
                            "actual_hook_ask_JSON", "actual_hook_deny_exit_2", "hook_uses_live_HTTP_writer", "hook_reopens_database_after_writer_exit", "proposed_command_never_executed"],
                    observed_gate_decisions=stats["gate"]["decisions"], selected_threshold=fitted["selected_threshold"], runtime_thresholds_changed=False)
                if report:
                    report.parent.mkdir(parents=True, exist_ok=True)
                    report.write_text(json.dumps(evidence, indent=2) + "\n")
                print(json.dumps(evidence))
            finally:
                process.stdin.close()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.terminate()
                    process.wait(timeout=10)


if __name__ == "__main__":
    main()
