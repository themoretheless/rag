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
