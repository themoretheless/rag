use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn insecure_or_invalid_remote_auth_fails_before_creating_the_database() {
    for duplicate_tokens in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let db = root.path().join("must-not-be-created").join("store.duckdb");
        let mut command = Command::new(env!("CARGO_BIN_EXE_rag-mcp"));
        command
            .current_dir(root.path())
            .env("RAG_DB_PATH", &db)
            .env("RAG_EMBEDDING_PROVIDER", "mock")
            .env("RAG_EMBEDDING_DIMS", "8")
            .env("RAG_LLM_PROVIDER", "ollama")
            .env("RAG_LLM_ENABLED", "false")
            .env("RAG_HTTP_BIND", "0.0.0.0:0")
            .env("RAG_HTTP_ALLOW_REMOTE", "true")
            .env_remove("RAG_HTTP_READ_TOKEN")
            .env_remove("RAG_HTTP_WRITE_TOKEN")
            .env_remove("RAG_HTTP_ADMIN_TOKEN");
        let secret = "duplicate-test-credential-0000000000";
        if duplicate_tokens {
            command
                .env("RAG_HTTP_READ_TOKEN", secret)
                .env("RAG_HTTP_ADMIN_TOKEN", secret);
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        while child.try_wait().unwrap().is_none() {
            if started.elapsed() > Duration::from_secs(5) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("insecure startup did not fail promptly");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success());
        assert!(error.contains("HTTP security configuration"), "{error}");
        assert!(!error.contains(secret));
        assert!(
            !db.parent().unwrap().exists(),
            "auth failure must precede DB initialization"
        );
    }
}
