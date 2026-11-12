//! Offline recovery CLI. Stop the live single-writer service before opening its DB.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rag_mcp::db::recovery::{
    BUNDLE_VERSION, ConflictPolicy, RecoveryBundle, backup_inventory, decode_recovery_bundle,
    encode_recovery_bundle, publish_recovery_artifact, read_recovery_bundle_file,
    retention_preview, verify_backup,
};
use rag_mcp::util::refuse_live_database_target;

/// Offline recovery CLI. Stop the live single-writer service before opening
/// its DB. Portable JSON/JSONL is bounded in memory; use `recovery backup`
/// plus `recovery verify` for a large corpus.
#[derive(clap::Parser)]
#[command(name = "recovery", arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Online-consistent backup of DB into FILE, verified after writing.
    Backup(BackupArgs),
    /// Backup plus a count/schema/manifest comparison against the source.
    RestoreDrill(BackupArgs),
    /// Relational verification of a backup file.
    Verify {
        #[arg(long, value_name = "FILE")]
        backup: PathBuf,
    },
    /// List backups in a directory.
    Inventory {
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,
        /// Accepted for symmetry with `retention`; ignored.
        #[arg(long, value_name = "N", hide = true)]
        keep: Option<usize>,
    },
    /// Preview which backups retention would delete (never deletes).
    Retention {
        #[arg(long, value_name = "DIR")]
        dir: PathBuf,
        #[arg(long, value_name = "N", default_value_t = 7)]
        keep: usize,
    },
    /// Export documents as a Markdown vault (dry run unless --apply).
    ExportVault {
        #[arg(long)]
        db: PathBuf,
        #[arg(long, value_name = "DIR")]
        out: PathBuf,
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        overwrite: bool,
    },
    /// Export a portable JSON/JSONL recovery bundle.
    ExportBundle {
        #[arg(long)]
        db: PathBuf,
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// json or jsonl (default: from the file extension, else json).
        #[arg(long)]
        format: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        overwrite: bool,
    },
    /// Import a portable recovery bundle (dry run unless --apply).
    ImportBundle {
        #[arg(long)]
        db: PathBuf,
        #[arg(long = "in", value_name = "FILE")]
        input: PathBuf,
        #[arg(long)]
        format: Option<String>,
        #[arg(long)]
        apply: bool,
        /// error | skip | overwrite.
        #[arg(long)]
        conflict_policy: Option<String>,
    },
}

#[derive(clap::Args)]
struct BackupArgs {
    #[arg(long)]
    db: PathBuf,
    #[arg(long, value_name = "FILE")]
    out: PathBuf,
    #[arg(long)]
    overwrite: bool,
}

fn main() -> Result<()> {
    match <Cli as clap::Parser>::parse().command {
        Command::Backup(args) => backup(&args, false),
        Command::RestoreDrill(args) => backup(&args, true),
        Command::Verify { backup } => verify(&backup),
        Command::Inventory { dir, keep: _ } => inventory(&dir),
        Command::Retention { dir, keep } => retention(&dir, keep),
        Command::ExportVault {
            db,
            out,
            apply,
            overwrite,
        } => export_vault(&db, &out, !apply, overwrite),
        Command::ExportBundle {
            db,
            out,
            format,
            dry_run,
            overwrite,
        } => export_bundle(&db, &out, format.as_deref(), dry_run, overwrite),
        Command::ImportBundle {
            db,
            input,
            format,
            apply,
            conflict_policy,
        } => import_bundle(
            &db,
            &input,
            format.as_deref(),
            !apply,
            conflict_policy.as_deref(),
        ),
    }
}

fn backup(args: &BackupArgs, drill: bool) -> Result<()> {
    let BackupArgs { db, out, overwrite } = args;
    let overwrite = *overwrite;
    let store = rag_mcp::storage::open_store(db)
        .with_context(|| format!("open source {}", db.display()))?;
    let source = store.maintenance().stats()?;
    let source_schema = store.maintenance().schema_version()?.unwrap_or(0);
    let source_manifest = store.embedding_manifest().get_embedding_manifest()?;
    let report = store.recovery().backup_database(out, false, overwrite)?;
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

fn verify(path: &Path) -> Result<()> {
    let report = verify_backup(path)?;
    if !report.ok {
        bail!("backup relational verification failed");
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn inventory(dir: &Path) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&backup_inventory(dir)?)?);
    Ok(())
}

fn retention(dir: &Path, keep: usize) -> Result<()> {
    let candidates = retention_preview(dir, keep)?;
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"dry_run": true, "keep": keep, "would_delete": candidates})
        )?
    );
    Ok(())
}

fn export_vault(db: &Path, out: &Path, dry_run: bool, overwrite: bool) -> Result<()> {
    let store = rag_mcp::storage::open_store(db)?;
    let report = store.recovery().export_vault(out, dry_run, overwrite)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn export_bundle(
    db: &Path,
    out: &Path,
    format: Option<&str>,
    dry_run: bool,
    overwrite: bool,
) -> Result<()> {
    let store = rag_mcp::storage::open_store(db)?;
    refuse_live_database_target(out, store.path())?;
    if out.exists() && !overwrite {
        bail!("output exists: {}", out.display());
    }
    store.recovery().portable_recovery_preflight()?;
    let bundle = store.recovery().recovery_bundle()?;
    let format = bundle_format(format, out)?;
    let encoded = encode_recovery_bundle(&bundle, format)?;
    if !dry_run {
        publish_recovery_artifact(out, &encoded, overwrite)?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({"dry_run": dry_run,
        "path": out, "format": format, "documents": bundle.documents.len(), "bytes": encoded.len()}))?
    );
    Ok(())
}

fn import_bundle(
    db: &Path,
    input: &Path,
    format: Option<&str>,
    dry_run: bool,
    conflict_policy: Option<&str>,
) -> Result<()> {
    let format = bundle_format(format, input)?;
    let raw = read_recovery_bundle_file(input)?;
    let bundle = prepare_offline_import_bundle(decode_bundle(&raw, format)?)?;
    let store = rag_mcp::storage::open_store(db)?;
    let policy = ConflictPolicy::parse(conflict_policy)?;
    let report = store
        .recovery()
        .import_recovery_bundle(&bundle, policy, dry_run, input, format)?;
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

fn bundle_format(format: Option<&str>, path: &Path) -> Result<&'static str> {
    let format =
        format.unwrap_or_else(|| path.extension().and_then(|v| v.to_str()).unwrap_or("json"));
    match format {
        "json" => Ok("json"),
        "jsonl" | "ndjson" => Ok("jsonl"),
        _ => bail!("format must be json or jsonl"),
    }
}

fn decode_bundle(input: &str, format: &str) -> Result<RecoveryBundle> {
    Ok(decode_recovery_bundle(input, format)?)
}

#[cfg(test)]
mod tests {
    use super::{decode_bundle, prepare_offline_import_bundle};
    use chrono::Utc;
    use rag_mcp::db::recovery::{BUNDLE_VERSION, BundleDocument, RecoveryBundle};
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
                chunks: vec![
                    Chunk {
                        id: "legacy-chunk".into(),
                        document_id: String::new(),
                        chunk_index: 0,
                        content: "legacy".into(),
                        embedding: vec![1.0],
                        char_start: 0,
                        char_end: 6,
                        metadata_json: "{}".into(),
                    }
                    .into(),
                ],
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
