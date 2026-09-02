use std::process::Command;

use chrono::Utc;
use rag_mcp::db::recovery::{
    BundleDocument, RecoveryBundle, BUNDLE_VERSION, PORTABLE_RECOVERY_MAX_BYTES,
    PORTABLE_RECOVERY_MAX_DOCUMENTS,
};
use rag_mcp::models::Document;
use rag_mcp::Store;

#[test]
fn recovery_cli_smoke_backup_verify_inventory_and_drill() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("live.duckdb");
    let backup = root.path().join("backup.duckdb");
    let drill = root.path().join("drill.duckdb");
    drop(Store::open(&db).unwrap());
    let binary = env!("CARGO_BIN_EXE_recovery");

    for args in [
        vec![
            "backup",
            "--db",
            db.to_str().unwrap(),
            "--out",
            backup.to_str().unwrap(),
        ],
        vec!["verify", "--backup", backup.to_str().unwrap()],
        vec!["inventory", "--dir", root.path().to_str().unwrap()],
        vec![
            "restore-drill",
            "--db",
            db.to_str().unwrap(),
            "--out",
            drill.to_str().unwrap(),
        ],
    ] {
        let output = Command::new(binary).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .trim_start()
                .starts_with('{')
                || String::from_utf8_lossy(&output.stdout)
                    .trim_start()
                    .starts_with('[')
        );
    }
}

#[test]
fn recovery_cli_export_bundle_is_no_clobber_and_refuses_the_database_target() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("live.duckdb");
    let bundle = root.path().join("bundle.json");
    drop(Store::open(&db).unwrap());
    let binary = env!("CARGO_BIN_EXE_recovery");

    let first = Command::new(binary)
        .args([
            "export-bundle",
            "--db",
            db.to_str().unwrap(),
            "--out",
            bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let original = std::fs::read(&bundle).unwrap();
    assert!(!original.is_empty());

    let no_clobber = Command::new(binary)
        .args([
            "export-bundle",
            "--db",
            db.to_str().unwrap(),
            "--out",
            bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!no_clobber.status.success());
    assert_eq!(std::fs::read(&bundle).unwrap(), original);

    let live_target = Command::new(binary)
        .args([
            "export-bundle",
            "--db",
            db.to_str().unwrap(),
            "--out",
            db.to_str().unwrap(),
            "--overwrite",
        ])
        .output()
        .unwrap();
    assert!(!live_target.status.success());
    assert!(String::from_utf8_lossy(&live_target.stderr).contains("must not be the live DuckDB"));
    assert_eq!(Store::open(&db).unwrap().stats().unwrap(), (0, 0, 0, 0));
}

#[test]
fn recovery_cli_import_returns_nonzero_when_report_success_is_false() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("target.duckdb");
    let input = root.path().join("conflict.json");
    drop(Store::open(&db).unwrap());
    let document = |uri: &str| Document {
        id: "duplicate-id".into(),
        uri: uri.into(),
        title: uri.into(),
        content: uri.into(),
        ..Document::default()
    };
    let bundle = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: None,
        documents: vec![
            BundleDocument {
                document: document("recovery://first"),
                chunks: Vec::new(),
            },
            BundleDocument {
                document: document("recovery://second"),
                chunks: Vec::new(),
            },
        ],
    };
    std::fs::write(&input, serde_json::to_vec_pretty(&bundle).unwrap()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_recovery"))
        .args([
            "import-bundle",
            "--db",
            db.to_str().unwrap(),
            "--in",
            input.to_str().unwrap(),
            "--apply",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["success"], false);
    assert_eq!(report["durable_mutation_committed"], false);
    assert!(String::from_utf8_lossy(&output.stderr).contains("did not commit"));
    assert_eq!(Store::open(&db).unwrap().stats().unwrap(), (0, 0, 0, 0));
}

#[test]
fn recovery_cli_rejects_oversized_input_before_opening_target_database() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("must-not-be-created.duckdb");
    let input = root.path().join("oversized.json");
    let file = std::fs::File::create(&input).unwrap();
    file.set_len(PORTABLE_RECOVERY_MAX_BYTES + 1).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_recovery"))
        .args([
            "import-bundle",
            "--db",
            db.to_str().unwrap(),
            "--in",
            input.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("verified DuckDB backup"));
    assert!(!db.exists(), "input preflight must run before Store::open");
}

#[test]
fn recovery_cli_export_preflight_leaves_no_artifact() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("large.duckdb");
    let output_path = root.path().join("must-not-exist.json");
    drop(Store::open(&db).unwrap());
    let conn = duckdb::Connection::open(&db).unwrap();
    conn.execute(
        r#"
        INSERT INTO documents (
            id, uri, title, content, metadata_json, created_at, updated_at
        )
        SELECT
            'portable-limit-' || CAST(i AS VARCHAR),
            'recovery://portable-limit/' || CAST(i AS VARCHAR),
            'limit', '', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
        FROM range(0, ?) AS generated(i)
        "#,
        [i64::try_from(PORTABLE_RECOVERY_MAX_DOCUMENTS + 1).unwrap()],
    )
    .unwrap();
    drop(conn);

    let output = Command::new(env!("CARGO_BIN_EXE_recovery"))
        .args([
            "export-bundle",
            "--db",
            db.to_str().unwrap(),
            "--out",
            output_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("contains 10001 documents"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("verified DuckDB backup"));
    assert!(!output_path.exists());
}
