//! Offline recovery CLI. Stop the live single-writer service before opening its DB.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::Utc;
use rag_mcp::db::recovery::{
    backup_inventory, publish_recovery_artifact, retention_preview, verify_backup, BundleDocument,
    ConflictPolicy, RecoveryBundle, BUNDLE_VERSION,
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
    let bundle = store.recovery_bundle()?;
    let format = bundle_format(args, &out)?;
    let encoded = encode_bundle(&bundle, format)?;
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
    let raw = fs::read_to_string(&input)?;
    let bundle = decode_bundle(&raw, format)?;
    let store = Store::open(&db)?;
    let policy = ConflictPolicy::parse(value(args, "--conflict-policy"))?;
    let dry_run = !flag(args, "--apply");
    let report = store.import_recovery_bundle(&bundle, policy, dry_run, &input, format)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
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

fn encode_bundle(bundle: &RecoveryBundle, format: &str) -> Result<Vec<u8>> {
    if format == "json" {
        return Ok(serde_json::to_vec_pretty(bundle)?);
    }
    let mut lines = vec![serde_json::to_string(
        &serde_json::json!({"record_type":"manifest",
        "format": bundle.format, "version": bundle.version, "exported_at": bundle.exported_at}),
    )?];
    for item in &bundle.documents {
        lines.push(serde_json::to_string(
            &serde_json::json!({"record_type":"document","value":item}),
        )?);
    }
    Ok((lines.join("\n") + "\n").into_bytes())
}

fn decode_bundle(input: &str, format: &str) -> Result<RecoveryBundle> {
    if format == "json" {
        return Ok(serde_json::from_str(input)?);
    }
    let mut bundle = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        documents: Vec::new(),
    };
    for (line_no, line) in input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
    {
        let value: serde_json::Value =
            serde_json::from_str(line).with_context(|| format!("JSONL line {}", line_no + 1))?;
        match value.get("record_type").and_then(|v| v.as_str()) {
            Some("manifest") => {
                bundle.format = value["format"].as_str().unwrap_or_default().into();
                bundle.version = value["version"].as_u64().unwrap_or(BUNDLE_VERSION as u64) as u32;
                bundle.exported_at = serde_json::from_value(value["exported_at"].clone())?;
            }
            Some("document") => bundle
                .documents
                .push(serde_json::from_value::<BundleDocument>(
                    value["value"].clone(),
                )?),
            other => bail!(
                "unknown JSONL record_type {other:?} at line {}",
                line_no + 1
            ),
        }
    }
    Ok(bundle)
}

fn usage() {
    eprintln!(
        "recovery backup|restore-drill --db DB --out FILE [--overwrite]\n\
      recovery verify --backup FILE\nrecovery inventory|retention --dir DIR [--keep N]\n\
      recovery export-vault --db DB --out DIR [--apply] [--overwrite]\n\
      recovery export-bundle --db DB --out FILE [--format json|jsonl] [--dry-run]\n\
      recovery import-bundle --db DB --in FILE [--apply] [--conflict-policy error|skip|overwrite]"
    );
}
