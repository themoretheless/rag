"""Small real-stdio MCP client for isolated qualification runners."""
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time


class McpClient:
    def __init__(self, binary, root, source):
        self.root = Path(root).resolve()
        self.sequence = 0
        self.incoming = queue.Queue()
        env = {k: v for k, v in os.environ.items() if not k.startswith("RAG_")}
        env.update(RAG_DB_PATH=str(self.root / "test.duckdb"), RAG_INGEST_ROOTS=str(Path(source).resolve()),
                   RAG_EMBEDDING_PROVIDER="mock", RAG_EMBEDDING_DIMS="16",
                   RAG_LLM_ENABLED="false", RAG_TOOLS="full")
        self.stderr = (self.root / "stderr.log").open("w")
        self.process = subprocess.Popen([str(Path(binary).resolve())], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=self.stderr, text=True, env=env)
        def receive():
            try:
                for line in self.process.stdout:
                    self.incoming.put(json.loads(line))
            except Exception as error:
                self.incoming.put(error)
            finally:
                self.incoming.put(None)
        threading.Thread(target=receive, daemon=True).start()

    def request(self, method, params, timeout=60, expect_error=False):
        self.sequence += 1
        self.process.stdin.write(json.dumps(dict(jsonrpc="2.0", id=self.sequence, method=method, params=params)) + "\n")
        self.process.stdin.flush()
        deadline = time.monotonic() + timeout
        while True:
            result = self.incoming.get(timeout=max(.01, deadline - time.monotonic()))
            if isinstance(result, Exception):
                raise result
            assert result is not None, (self.root / "stderr.log").read_text()[-4000:]
            if result.get("id") == self.sequence:
                if expect_error:
                    assert "error" in result, result
                    return result
                assert "error" not in result, result
                return result["result"]

    def call(self, name, arguments):
        result = self.request("tools/call", dict(name=name, arguments=arguments))
        assert not result.get("isError"), result
        return json.loads(next(c["text"] for c in result["content"] if c["type"] == "text"))

    def __enter__(self):
        try:
            self.request("initialize", dict(protocolVersion="2024-11-05", capabilities={},
                                            clientInfo=dict(name="code-intelligence-qualification", version="1")))
            self.process.stdin.write(json.dumps(dict(jsonrpc="2.0", method="notifications/initialized")) + "\n")
            self.process.stdin.flush()
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def __exit__(self, *unused):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=10)
        self.stderr.close()
