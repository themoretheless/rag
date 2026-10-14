//! Smoke tests for the offline maintenance binaries (argument handling plus one
//! happy path each against a temporary database, mock embeddings, no network).

use std::path::Path;
use std::process::{Command, Output};

use chrono::Utc;
use rag_mcp::models::Document;
use rag_mcp::Store;

fn run(binary: &str, cwd: &Path, args: &[&str]) -> Output {
    Command::new(binary)
        .args(args)
        .current_dir(cwd)
        .env("RAG_EMBEDDING_PROVIDER", "mock")
        .env("RAG_EMBEDDING_DIMS", "32")
        .env("RAG_LLM_ENABLED", "false")
        .env("RAG_INGEST_ROOTS", cwd)
        .output()
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_help(binary: &str, cwd: &Path) {
    let output = run(binary, cwd, &["--help"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).contains("Usage"), "{}", stderr(&output));
}

fn assert_fails_with(binary: &str, cwd: &Path, args: &[&str], needle: &str) {
    let output = run(binary, cwd, args);
    assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
    assert!(
        stderr(&output).contains(needle),
        "{args:?}: {}",
        stderr(&output)
    );
}

fn document(id: &str, wing: &str, room: &str, source_file: &str) -> Document {
    let now = Utc::now();
    Document {
        id: id.into(),
        uri: format!("text://{id}"),
        title: id.into(),
        content: format!("{id} body"),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        wing: Some(wing.into()),
        room: (!room.is_empty()).then(|| room.into()),
        source_file: (!source_file.is_empty()).then(|| source_file.into()),
        ..Document::default()
    }
}

#[test]
fn db_repair_cli_validates_arguments() {
    let root = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_db_repair");
    assert_help(binary, root.path());
    assert_fails_with(binary, root.path(), &[], "required: --db");
    assert_fails_with(
        binary,
        root.path(),
        &["--db", "x.duckdb"],
        "required: --out",
    );
    assert_fails_with(binary, root.path(), &["--bogus"], "unknown arg: --bogus");
    assert_fails_with(binary, root.path(), &["--db"], "--db needs path");
}

#[test]
fn db_repair_cli_rebuilds_into_a_new_file_and_refuses_to_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("live.duckdb");
    let out = root.path().join("rebuilt.duckdb");
    {
        let store = Store::open(&db).unwrap();
        store
            .upsert_document(&document("keep-me", "w", "r", ""))
            .unwrap();
    }
    let binary = env!("CARGO_BIN_EXE_db_repair");
    let args = ["--db", db.to_str().unwrap(), "--out", out.to_str().unwrap()];
    let output = run(binary, root.path(), &args);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("verified: documents=1"),
        "{}",
        stderr(&output)
    );

    let rebuilt = Store::open(&out).unwrap();
    assert_eq!(
        rebuilt.get_document("keep-me").unwrap().unwrap().content,
        "keep-me body"
    );
    drop(rebuilt);

    assert_fails_with(binary, root.path(), &args, "--out already exists");
}

#[test]
fn ingest_file_cli_validates_arguments() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("note.md");
    std::fs::write(&file, "# Note\nbody").unwrap();
    let db = root.path().join("rag.duckdb");
    let binary = env!("CARGO_BIN_EXE_ingest_file");
    assert_help(binary, root.path());
    assert_fails_with(binary, root.path(), &[], "required: --path");
    assert_fails_with(
        binary,
        root.path(),
        &["--path", file.to_str().unwrap()],
        "required: --db",
    );
    assert_fails_with(
        binary,
        root.path(),
        &[
            "--path",
            file.to_str().unwrap(),
            "--db",
            db.to_str().unwrap(),
        ],
        "without --offline",
    );
    assert_fails_with(
        binary,
        root.path(),
        &[
            "--offline",
            "--db",
            db.to_str().unwrap(),
            "--path",
            root.path().to_str().unwrap(),
        ],
        "--path is not a file",
    );
    assert_fails_with(binary, root.path(), &["--nope"], "unknown arg: --nope");
    assert!(!db.exists(), "argument errors must not create the database");
}

#[test]
fn ingest_file_cli_ingests_one_file_offline() {
    let root = tempfile::tempdir().unwrap();
    let root_path = root.path().canonicalize().unwrap();
    let file = root_path.join("note.md");
    std::fs::write(&file, "# Note\nThe offline ingest body.").unwrap();
    let db = root_path.join("rag.duckdb");
    let binary = env!("CARGO_BIN_EXE_ingest_file");

    let output = run(
        binary,
        &root_path,
        &[
            "--offline",
            "--db",
            db.to_str().unwrap(),
            "--path",
            file.to_str().unwrap(),
            "--wing",
            "proj",
            "--room",
            "notes",
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let result: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    assert!(result["chunk_count"].as_u64().unwrap() >= 1, "{result}");
    let id = result["document_id"].as_str().unwrap().to_string();

    let store = Store::open(&db).unwrap();
    let doc = store.get_document(&id).unwrap().unwrap();
    assert!(doc.content.contains("offline ingest body"));
    assert_eq!(doc.wing.as_deref(), Some("proj"));
    assert_eq!(doc.room.as_deref(), Some("notes"));
    assert!(doc.uri.starts_with("file://"), "{}", doc.uri);
}

#[test]
fn ingest_project_cli_validates_arguments_and_dry_run_skips_the_database() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(project.join("a.md"), "# A\nalpha").unwrap();
    std::fs::write(project.join("b.rs"), "fn b() {}").unwrap();
    let db = root.path().join("rag.duckdb");
    let binary = env!("CARGO_BIN_EXE_ingest_project");
    assert_help(binary, root.path());
    assert_fails_with(binary, root.path(), &[], "required: --root");
    assert_fails_with(
        binary,
        root.path(),
        &["--root", project.to_str().unwrap()],
        "required: --db",
    );
    assert_fails_with(
        binary,
        root.path(),
        &[
            "--root",
            project.to_str().unwrap(),
            "--db",
            db.to_str().unwrap(),
        ],
        "without --offline",
    );
    assert_fails_with(
        binary,
        root.path(),
        &[
            "--offline",
            "--db",
            db.to_str().unwrap(),
            "--root",
            "/definitely/not/here",
        ],
        "--root is not a directory",
    );
    assert_fails_with(binary, root.path(), &["--max-bytes", "lots"], "max-bytes");

    let output = run(
        binary,
        root.path(),
        &[
            "--offline",
            "--db",
            db.to_str().unwrap(),
            "--root",
            project.to_str().unwrap(),
            "--room",
            "demo",
            "--ext",
            "md",
            "--dry-run",
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let log = stderr(&output);
    assert!(log.contains("files to ingest: 1"), "{log}");
    assert!(
        log.contains("[dry-run] project://projects/demo/a.md"),
        "{log}"
    );
    assert!(!log.contains("b.rs"), "{log}");
    assert!(!db.exists(), "dry run must not open the database");
}

#[test]
fn ingest_project_cli_ingests_a_tree_offline() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("proj");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("README.md"), "# Readme\nproject overview").unwrap();
    std::fs::write(
        project.join("src").join("lib.rs"),
        "pub fn answer() -> u8 { 42 }",
    )
    .unwrap();
    let db = root.path().join("rag.duckdb");
    let binary = env!("CARGO_BIN_EXE_ingest_project");

    let output = run(
        binary,
        root.path(),
        &[
            "--offline",
            "--db",
            db.to_str().unwrap(),
            "--root",
            project.to_str().unwrap(),
            "--wing",
            "code",
            "--room",
            "demo",
        ],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("done: ok=2 skipped=0 failed=0 wing=code room=demo"),
        "{}",
        stderr(&output)
    );

    let store = Store::open(&db).unwrap();
    let mut uris: Vec<String> = store
        .list_documents()
        .unwrap()
        .into_iter()
        .filter(|d| d.wing.as_deref() == Some("code") && d.room.as_deref() == Some("demo"))
        .map(|d| d.uri)
        .collect();
    uris.sort();
    assert_eq!(
        uris,
        vec![
            "project://code/demo/README.md".to_string(),
            "project://code/demo/src/lib.rs".to_string(),
        ]
    );
}

#[test]
fn promote_dodo_projects_cli_requires_a_database_argument() {
    let root = tempfile::tempdir().unwrap();
    assert_fails_with(
        env!("CARGO_BIN_EXE_promote_dodo_projects"),
        root.path(),
        &[],
        "usage: promote_dodo_projects",
    );
}

#[test]
fn promote_dodo_projects_cli_moves_dodo_rooms_to_project_wings() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("rag.duckdb");
    {
        let store = Store::open(&db).unwrap();
        for doc in [
            document("nested", "Dodo", "alpha", "/repos/Dodo/alpha/api/file.md"),
            document("overview", "Dodo", "overview", ""),
            document("flat", "Dodo", "beta", "/elsewhere/file.md"),
            document("untouched", "Other", "alpha", "/repos/Dodo/alpha/api/x.md"),
        ] {
            store.upsert_document(&doc).unwrap();
        }
    }
    let output = run(
        env!("CARGO_BIN_EXE_promote_dodo_projects"),
        root.path(),
        &[db.to_str().unwrap()],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("migrated 3 Dodo documents"), "{stdout}");
    assert!(stdout.contains("remaining Dodo wing: 0"), "{stdout}");

    let store = Store::open(&db).unwrap();
    let placement = |id: &str| {
        let doc = store.get_document(id).unwrap().unwrap();
        let tags = doc.metadata().unwrap()["tags"].clone();
        (
            doc.wing.unwrap_or_default(),
            doc.room.unwrap_or_default(),
            tags,
        )
    };
    let dodo = serde_json::json!(["Dodo"]);
    assert_eq!(
        placement("nested"),
        ("alpha".into(), "api".into(), dodo.clone())
    );
    assert_eq!(
        placement("overview"),
        ("Dodo-overview".into(), "wiki".into(), dodo.clone())
    );
    assert_eq!(placement("flat"), ("beta".into(), "root".into(), dodo));
    assert_eq!(
        placement("untouched"),
        ("Other".into(), "alpha".into(), serde_json::Value::Null)
    );
}
