//! Offline recovery CLI. Stop the live single-writer service before opening its DB.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rag_mcp::db::recovery::{
    backup_inventory, decode_recovery_bundle, encode_recovery_bundle, publish_recovery_artifact,
    read_recovery_bundle_file, retention_preview, verify_backup, ConflictPolicy, RecoveryBundle,
    BUNDLE_VERSION,
};
use rag_mcp::util::refuse_live_database_target;
use rag_mcp::Store;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".into());
    let rest: Vec<String> = args.collect();
    match command.as_str() {
        "backup" => backup(&rest, false),
        "restore-drill" => backup(&rest, true),
        "verify" => verify(&rest),
        "inventory" => inventory(&rest),
        "retention" => retention(&rest),
        "export-vault" => export_vault(&rest),
        "export-bundle" => export_bundle(&rest),
        "import-bundle" => import_bundle(&rest),
        "help" | "-h" | "--help" => {
            usage();
            Ok(())
        }
        other => bail!("unknown recovery command '{other}' (try --help)"),
    }
}

fn backup(args: &[String], drill: bool) -> Result<()> {
    let db = required_path(args, "--db")?;
    let out = required_path(args, "--out")?;
    let overwrite = flag(args, "--overwrite");
    let store = Store::open(&db).with_context(|| format!("open source {}", db.display()))?;
    let source = store.stats()?;
    let source_schema = store.schema_version()?.unwrap_or(0);
    let source_manifest = store.get_embedding_manifest()?;
    let report = store.backup_database(&out, false, overwrite)?;
    let verification = report
        .verification
        .as_ref()
        .context("backup verification missing")?;
    if drill
        && source
            != (
                verification.documents,
                verification.chunks,
                verification.nodes,
                verification.edges,
            )
    {
        bail!(
            "restore drill count mismatch: source={source:?} restored=({}, {}, {}, {})",
            verification.documents,
            verification.chunks,
            verification.nodes,
            verification.edges
        );
    }
    if drill && source_schema != verification.schema_version {
        bail!(
            "restore drill schema mismatch: source={source_schema} restored={}",
            verification.schema_version
        );
    }
    if drill
        && serde_json::to_value(&source_manifest)?
            != serde_json::to_value(&verification.embedding_manifest)?
    {
        bail!("restore drill embedding manifest mismatch");
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "command": if drill { "restore-drill" } else { "backup" }, "report": report,
            "source_counts": {"documents": source.0, "chunks": source.1, "nodes": source.2, "edges": source.3},
            "source_schema_version": source_schema, "source_embedding_manifest": source_manifest
        }))?
    );
    Ok(())
}

fn verify(args: &[String]) -> Result<()> {
    let path = required_path(args, "--backup")?;
    let report = verify_backup(&path)?;
    if !report.ok {
        bail!("backup relational verification failed");
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn inventory(args: &[String]) -> Result<()> {
    let dir = required_path(args, "--dir")?;
    println!(
        "{}",
        serde_json::to_string_pretty(&backup_inventory(&dir)?)?
    );
    Ok(())
}

fn retention(args: &[String]) -> Result<()> {
    let dir = required_path(args, "--dir")?;
    let keep = value(args, "--keep").unwrap_or("7").parse::<usize>()?;
    let candidates = retention_preview(&dir, keep)?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"dry_run": true, "keep": keep, "would_delete": candidates})
        )?
    );
    Ok(())
}

fn export_vault(args: &[String]) -> Result<()> {
    let db = required_path(args, "--db")?;
    let out = required_path(args, "--out")?;
    let store = Store::open(&db)?;
    let dry_run = !flag(args, "--apply");
    let report = store.export_vault(&out, dry_run, flag(args, "--overwrite"))?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn export_bundle(args: &[String]) -> Result<()> {
    let db = required_path(args, "--db")?;
    let out = required_path(args, "--out")?;
    let overwrite = flag(args, "--overwrite");
    let dry_run = flag(args, "--dry-run");
    let store = Store::open(&db)?;
    refuse_live_database_target(&out, store.path())?;
    if out.exists() && !overwrite {
        bail!("output exists: {}", out.display());
    }
    store.portable_recovery_preflight()?;
    let bundle = store.recovery_bundle()?;
    let format = bundle_format(args, &out)?;
    let encoded = encode_recovery_bundle(&bundle, format)?;
    if !dry_run {
        publish_recovery_artifact(&out, &encoded, overwrite)?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({"dry_run": dry_run,
        "path": out, "format": format, "documents": bundle.documents.len(), "bytes": encoded.len()}))?
    );
    Ok(())
}

fn import_bundle(args: &[String]) -> Result<()> {
    let db = required_path(args, "--db")?;
    let input = required_path(args, "--in")?;
    let format = bundle_format(args, &input)?;
    let raw = read_recovery_bundle_file(&input)?;
    let bundle = prepare_offline_import_bundle(decode_bundle(&raw, format)?)?;
    let store = Store::open(&db)?;
    let policy = ConflictPolicy::parse(value(args, "--conflict-policy"))?;
    let dry_run = !flag(args, "--apply");
    let report = store.import_recovery_bundle(&bundle, policy, dry_run, &input, format)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !report.success {
        bail!("recovery bundle import did not commit; inspect the structured report above");
    }
    Ok(())
}

fn prepare_offline_import_bundle(mut bundle: RecoveryBundle) -> Result<RecoveryBundle> {
    if bundle.version != 1 {
        return Ok(bundle);
    }
    let chunks = bundle
        .documents
        .iter()
        .map(|document| document.chunks.len())
        .sum::<usize>();
    if chunks > 0 {
        bail!(
            "legacy recovery bundle v1 contains {chunks} chunks with unverifiable vectors; import it through the running gateway with reembed_legacy=true"
        );
    }
    // Metadata-only v1 bundles contain no vector identity to trust or migrate.
    bundle.embedding_manifest = None;
    bundle.version = BUNDLE_VERSION;
    Ok(bundle)
}

fn value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
}
fn required_path(args: &[String], name: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(
        value(args, name).with_context(|| format!("required {name} PATH"))?,
    ))
}
fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| arg == name)
}
fn bundle_format(args: &[String], path: &Path) -> Result<&'static str> {
    let format = value(args, "--format")
        .unwrap_or_else(|| path.extension().and_then(|v| v.to_str()).unwrap_or("json"));
    match format {
        "json" => Ok("json"),
        "jsonl" | "ndjson" => Ok("jsonl"),
        _ => bail!("format must be json or jsonl"),
    }
}

fn decode_bundle(input: &str, format: &str) -> Result<RecoveryBundle> {
    Ok(decode_recovery_bundle(input, format)?)
}

fn usage() {
    eprintln!(
        "recovery backup|restore-drill --db DB --out FILE [--overwrite]\n\
      recovery verify --backup FILE\nrecovery inventory|retention --dir DIR [--keep N]\n\
      recovery export-vault --db DB --out DIR [--apply] [--overwrite]\n\
      recovery export-bundle --db DB --out FILE [--format json|jsonl] [--dry-run]\n\
      recovery import-bundle --db DB --in FILE [--apply] [--conflict-policy error|skip|overwrite]\n\
      Portable JSON/JSONL is bounded in memory; use `recovery backup` plus `recovery verify` for a large corpus."
    );
}

#[cfg(test)]
mod tests {
    use super::{decode_bundle, prepare_offline_import_bundle};
    use chrono::Utc;
    use rag_mcp::db::recovery::{BundleDocument, RecoveryBundle, BUNDLE_VERSION};
    use rag_mcp::models::{Chunk, Document};

    const EXPORTED_AT: &str = "2026-09-02T12:34:56Z";

    fn manifest(extra: &str) -> String {
        format!(
            r#"{{"record_type":"manifest","format":"rag-recovery-bundle","version":1,"exported_at":"{EXPORTED_AT}"{extra}}}"#
        )
    }

    #[test]
    fn jsonl_v1_manifest_may_omit_embedding_manifest() {
        let bundle = decode_bundle(&manifest(""), "jsonl").expect("valid v1 bundle");

        assert_eq!(bundle.format, "rag-recovery-bundle");
        assert_eq!(bundle.version, 1);
        assert_eq!(bundle.exported_at.to_rfc3339(), "2026-09-02T12:34:56+00:00");
        assert!(bundle.embedding_manifest.is_none());
        assert!(bundle.documents.is_empty());
    }

    #[test]
    fn offline_v1_import_only_upgrades_metadata_without_vectors() {
        let metadata_only = RecoveryBundle {
            format: "rag-recovery-bundle".into(),
            version: 1,
            exported_at: Utc::now(),
            embedding_manifest: None,
            documents: Vec::new(),
        };
        assert_eq!(
            prepare_offline_import_bundle(metadata_only)
                .expect("metadata-only v1 is safe")
                .version,
            BUNDLE_VERSION
        );

        let with_vector = RecoveryBundle {
            format: "rag-recovery-bundle".into(),
            version: 1,
            exported_at: Utc::now(),
            embedding_manifest: None,
            documents: vec![BundleDocument {
                document: Document::default(),
                chunks: vec![Chunk {
                    id: "legacy-chunk".into(),
                    document_id: String::new(),
                    chunk_index: 0,
                    content: "legacy".into(),
                    embedding: vec![1.0],
                    char_start: 0,
                    char_end: 6,
                    metadata_json: "{}".into(),
                }],
            }],
        };
        let error = prepare_offline_import_bundle(with_vector)
            .expect_err("legacy vectors need a live embedder");
        assert!(error.to_string().contains("reembed_legacy=true"));
    }

    #[test]
    fn jsonl_requires_one_manifest_before_documents() {
        for (input, expected) in [
            ("", "requires exactly one manifest"),
            ("  \n\t", "requires exactly one manifest"),
            (
                r#"{"record_type":"document","value":{}}"#,
                "document precedes manifest",
            ),
            (
                &format!("{}\n{}", manifest(""), manifest("")),
                "duplicate JSONL manifest",
            ),
        ] {
            let error = decode_bundle(input, "jsonl").expect_err("invalid JSONL must fail");
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?} in {error:#}"
            );
        }
    }

    #[test]
    fn jsonl_manifest_requires_typed_identity_fields() {
        for (input, expected) in [
            (
                format!(
                    r#"{{"record_type":"manifest","version":1,"exported_at":"{EXPORTED_AT}"}}"#
                ),
                "field 'format'",
            ),
            (
                format!(
                    r#"{{"record_type":"manifest","format":"rag-recovery-bundle","exported_at":"{EXPORTED_AT}"}}"#
                ),
                "field 'version'",
            ),
            (
                r#"{"record_type":"manifest","format":"rag-recovery-bundle","version":1}"#
                    .to_owned(),
                "requires 'exported_at'",
            ),
        ] {
            let error = decode_bundle(&input, "jsonl").expect_err("missing field must fail");
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?} in {error:#}"
            );
        }
    }

    #[test]
    fn jsonl_version_is_checked_without_current_version_fallback() {
        for (version, expected) in [("0", "greater than zero"), ("4294967296", "exceeds u32")] {
            let input = format!(
                r#"{{"record_type":"manifest","format":"rag-recovery-bundle","version":{version},"exported_at":"{EXPORTED_AT}"}}"#
            );
            let error = decode_bundle(&input, "jsonl").expect_err("invalid version must fail");
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?} in {error:#}"
            );
        }
    }
}
