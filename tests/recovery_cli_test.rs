use std::process::Command;

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
