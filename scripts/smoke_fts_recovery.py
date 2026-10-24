#!/usr/bin/env python3
"""Real-process FTS/WAL and shutdown qualification in disposable mock databases.

python3 scripts/smoke_fts_recovery.py target/debug/rag-mcp report.json
Only child processes/databases created here are signalled or removed.
"""
import hashlib
import json
import os
from pathlib import Path
import queue
import re
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request


class Server:
    def __init__(self, binary, root, mode):
        self.mode, self.root = mode, root
        self.opener=urllib.request.build_opener(urllib.request.ProxyHandler({}))
        self.http = mode in ("http_only", "dual_uninitialized")
        self.ids = 0
        self.messages = queue.Queue()
        self.log_path = root / f"{mode}-{time.time_ns()}.log"
        self.log = self.log_path.open("w")
        self.env = {key: value for key, value in os.environ.items() if not key.startswith("RAG_")}
        self.env.update(RAG_DB_PATH=str(root / "test.duckdb"), RAG_INGEST_ROOTS=str(root / "project"),
                        RAG_EMBEDDING_PROVIDER="mock", RAG_EMBEDDING_DIMS="16", RAG_LLM_ENABLED="false",
                        RAG_TOOLS="full", RAG_WORKER_THREADS="2", RAG_FTS_STALE_GENERATION_THRESHOLD="0", RUST_LOG="info")
        self.port = None
        if mode != "stdio":
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                self.port = listener.getsockname()[1]
            self.env.update(RAG_HTTP_BIND=f"127.0.0.1:{self.port}", RAG_HTTP_ADMIN_TOKEN="a" * 48)
        if mode == "http_only":
            self.env["RAG_HTTP_ONLY"] = "true"
        self.process = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=self.log, text=True, env=self.env)
        def receive():
            for line in self.process.stdout:
                self.messages.put(json.loads(line))
            self.messages.put(None)
        threading.Thread(target=receive, daemon=True).start()
        try:
            if not self.http:
                self.rpc("initialize", dict(protocolVersion="2024-11-05", capabilities={},
                                            clientInfo=dict(name="fts-recovery-fixture", version="1")))
                self.process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
                self.process.stdin.flush()
            else:
                # The dual_uninitialized case deliberately leaves stdin open with no
                # initialize request, reproducing headless/launch-agent startup.
                deadline = time.monotonic() + 60
                while True:
                    if self.process.poll() is not None:
                        raise AssertionError(self.log_path.read_text())
                    try:
                        self.rpc("tools/list", {}, timeout=1)
                        break
                    except (urllib.error.URLError, TimeoutError, ConnectionError):
                        if time.monotonic() >= deadline:
                            raise
                        time.sleep(0.05)
        except BaseException:
            self.cleanup()
            raise

    def rpc(self, method, params, timeout=30):
        self.ids += 1
        request_id = self.ids
        payload = dict(jsonrpc="2.0", id=request_id, method=method, params=params)
        if self.http:
            req = urllib.request.Request(f"http://127.0.0.1:{self.port}/mcp", data=json.dumps(payload).encode(), headers={
                "Content-Type": "application/json", "Accept": "application/json, text/event-stream", "Authorization": "Bearer " + "a" * 48})
            with self.opener.open(req, timeout=timeout) as response:
                if "application/json" in response.headers.get("Content-Type", ""):
                    reply = json.load(response)
                else:
                    reply = None
                    for line in response:
                        if line.startswith(b"data:"):
                            value = json.loads(line[5:].strip())
                            if value.get("id") == request_id:
                                reply = value
                                break
                    assert reply is not None, "missing HTTP MCP response"
        else:
            self.process.stdin.write(json.dumps(payload) + "\n")
            self.process.stdin.flush()
            deadline = time.monotonic() + timeout
            while True:
                reply = self.messages.get(timeout=max(0.01, deadline - time.monotonic()))
                assert reply is not None, self.log_path.read_text()
                if reply.get("id") == request_id:
                    break
        assert "error" not in reply, reply
        return reply["result"]

    def call(self, name, arguments=None):
        result = self.rpc("tools/call", dict(name=name, arguments=arguments or {}, _meta={"rag/task_session": "recovery"}))
        assert not result.get("isError"), result
        return json.loads(next(c["text"] for c in result["content"] if c["type"] == "text"))

    def stop(self, signum):
        assert self.process.poll() is None, self.log_path.read_text()
        before = time.monotonic()
        self.process.send_signal(signum)
        try:
            code = self.process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            self.cleanup()
            raise AssertionError(f"shutdown exceeded 20s: {self.log_path.read_text()}")
        elapsed = time.monotonic() - before
        self.log.close()
        if signum != signal.SIGKILL:
            assert code == 0, (code, self.log_path.read_text())
            text=re.sub(r"\x1b\[[0-9;]*m", "", self.log_path.read_text())
            assert "shutdown checkpoint failed" not in text.lower(),text
            assert "rag-mcp shutdown complete" in text,text
            assert "checkpointed=true" in text,text
        else:
            assert code < 0, code
        return code, round(elapsed, 3)

    def cleanup(self):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=10)
        if not self.process.stdin.closed:
            self.process.stdin.close()
        self.log.close()


def manifest(server):
    result = server.call("catalog", dict(limit=100))
    return result["index_rev"], {i["document_id"]: (i["path"], i["document_revision"], i["content_hash"]) for i in result["items"]}


def main():
    binary = Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/rag-mcp").resolve()
    report_path = Path(sys.argv[2]) if len(sys.argv) > 2 else None
    cases = []
    for mode in ("stdio", "dual", "http_only", "dual_uninitialized"):
        for signum in (signal.SIGTERM, signal.SIGINT, signal.SIGKILL):
            with tempfile.TemporaryDirectory(prefix="rag-recovery-") as temporary:
                root = Path(temporary)
                source = root / "project"
                source.mkdir()
                initial = 'pub fn preserved() -> &\'static str { "quasarfiber" }\n'
                final = initial + "fn added() {}\n"
                html = "<html><body><article><p class='kept'>Visible.</p></article><style>.kept{color:red}</style></body></html>"
                (source / "main.rs").write_text(initial)
                (source / "page.html").write_text(html)
                (source / "001.sql").write_text("CREATE TABLE audit_rows(name TEXT NOT NULL);")
                server = Server(binary, root, mode)
                restarted = None
                try:
                    server.call("task_set", dict(name="committed task", session_id="recovery"))
                    one = server.call("sync_sources", dict(path=str(source), wing="fixture"))
                    assert not one.get("errors"), one
                    (source / "main.rs").write_text(final)
                    two = server.call("sync_sources", dict(path=str(source), wing="fixture"))
                    assert not two.get("errors"), two
                    note = server.call("note", dict(text="committed note", session_id="recovery"))
                    calibration = server.call("calibrate", dict(samples=dict(threshold=0.7, sample_size=25)))
                    server.call("gate", dict(payload=dict(hook_event_name="SessionStart", session_id="recovery")))
                    docs = server.call("list_documents")
                    before_docs = {d["id"]: server.call("get_document", dict(document_id=d["id"], include_chunks=True)) for d in docs}
                    before_manifest = manifest(server)
                    before_search = server.call("search", dict(query="quasarfiber", mode="lex", top_k=5))
                    assert before_search, before_search
                    status = server.call("status")
                    assert status["fts_ready"] and not status["fts_dirty"], status
                    assert server.log_path.read_text().count("FTS index refreshed on chunks (DuckDB BM25)") >= 3, server.log_path.read_text()
                    wal = root / "test.duckdb.wal"
                    wal_bytes = wal.stat().st_size
                    assert wal_bytes > 0, "fixture must leave committed WAL to replay"
                    code, elapsed = server.stop(signum)
                    # No deleting/replacing WAL, snapshots or raw/control data between processes.
                    restarted = Server(binary, root, mode)
                    after_docs = {d["id"]: restarted.call("get_document", dict(document_id=d["id"], include_chunks=True)) for d in restarted.call("list_documents")}
                    assert after_docs == before_docs
                    assert manifest(restarted) == before_manifest
                    assert restarted.call("notes", dict(session_id="recovery"))["items"][0]["note_id"] == note["note_id"]
                    assert restarted.call("calibration_reports")["items"][0]["id"] == calibration["id"]
                    stats = restarted.call("task_stats", dict(session_id="recovery"))
                    assert stats["task"] == "committed task" and stats["gate"]["decisions"] == 1, stats
                    after_search = restarted.call("search", dict(query="quasarfiber", mode="lex", top_k=5))
                    assert [h["chunk_id"] for h in after_search] == [h["chunk_id"] for h in before_search]
                    assert all(abs(a["score"] - b["score"]) < 1e-9 for a, b in zip(after_search, before_search))
                    body = restarted.call("code", dict(symbol="preserved"))
                    assert body["found"] and "quasarfiber" in body["code"]
                    html_id = next(d["id"] for d in docs if d.get("source_file", "").endswith("page.html"))
                    assert restarted.call("live_read", dict(document_id=html_id, source="stored", max_chars=20000))["content"] == html
                    assert restarted.call("write_check", dict(table="audit_rows", columns=["name"]))["status"] == "valid"
                    assert restarted.call("status")["fts_rebuild_count"] == status["fts_rebuild_count"], "warm restart must reuse complete FTS"
                    assert "falling back to term-frequency" not in restarted.log_path.read_text().lower()
                    assert "term-frequency fallback" not in restarted.log_path.read_text().lower()

                    restarted.stop(signal.SIGTERM)
                    row = dict(mode=mode, signal=signal.Signals(signum).name, exit_code=code, shutdown_seconds=elapsed,
                               wal_bytes_before_stop=wal_bytes, documents=len(before_docs), index_rev=before_manifest[0],
                               committed_raw_chunks_history_html_notes_gate_calibration_preserved=True,
                               lexical_order_and_scores_unchanged=True, bm25_used=True, warm_fts_reused=True)
                    cases.append(row)
                    print(json.dumps(row), flush=True)
                finally:
                    server.cleanup()
                    if restarted:
                        restarted.cleanup()
    report = dict(status="passed", cases=cases, case_count=len(cases), mock_embeddings=True,
                  production_database_modified=False, wal_deleted_or_discarded=False,
                  checks=["SIGTERM_SIGINT_checkpointed_exit", "SIGKILL_WAL_replay", "four_serving_modes", "raw_and_control_record_preservation", "immutable_source_manifest", "original_HTML_blob", "lexical_BM25_equivalence", "warm_index_reuse"])
    if report_path:
        report_path.parent.mkdir(parents=True, exist_ok=True)
        report_path.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(dict(status="passed", case_count=len(cases))))


if __name__ == "__main__":
    main()
