use std::process::Command;

use chrono::Utc;
use rag_mcp::Store;
use rag_mcp::db::recovery::{
    BUNDLE_VERSION, BundleDocument, PORTABLE_RECOVERY_MAX_BYTES, PORTABLE_RECOVERY_MAX_DOCUMENTS,
    RecoveryBundle,
};
use rag_mcp::models::Document;

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
    assert_eq!(
        Store::open(&db).unwrap().maintenance().stats().unwrap(),
        (0, 0, 0, 0)
    );
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
    assert_eq!(
        Store::open(&db).unwrap().maintenance().stats().unwrap(),
        (0, 0, 0, 0)
    );
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
    use rag_mcp::db::lin_engine::{LinWriter, RecBuilder};

    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("large.lin");
    let output_path = root.path().join("must-not-exist.json");
    {
        let store = Store::open(&db).unwrap();
        let mut w = store.write().unwrap();
        let now = "2026-11-11T00:00:00Z";
        let total = PORTABLE_RECOVERY_MAX_DOCUMENTS + 1;
        let mut batch = Vec::with_capacity(512);
        for i in 0..total {
            batch.push(
                RecBuilder::new()
                    .id(format!("portable-limit-{i}"))
                    .str("uri", format!("recovery://portable-limit/{i}"))
                    .str("title", "limit")
                    .str("content", "")
                    .str("metadata_json", "{}")
                    .str("created_at", now)
                    .str("updated_at", now)
                    .str("status", "active")
                    .str("layer", "raw")
                    .str("kind", "document")
                    .i64("revision", 1)
                    .bool("pinned", false)
                    .f64("boost", 0.0)
                    .build(),
            );
            if batch.len() >= 512 {
                w.insert_records("documents", std::mem::take(&mut batch))
                    .unwrap();
            }
        }
        if !batch.is_empty() {
            w.insert_records("documents", batch).unwrap();
        }
    }

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
