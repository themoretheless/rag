//! Consistent DuckDB backup and portable document bundle recovery.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::fs::OpenOptions;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};
use std::time::SystemTime;

use chrono::{DateTime, NaiveDateTime, Utc};
use duckdb::params;
use serde::{Deserialize, Serialize};

use super::store::{embedding_identity_fingerprint, set_embedding_manifest_locked};
use super::Store;
use crate::error::{AppError, Result};
use crate::models::{Chunk, Document, EmbeddingManifest};
use crate::util::backup_artifact_paths;

pub const BUNDLE_VERSION: u32 = 2;

/// Portable JSON/JSONL recovery is intentionally bounded because both serde
/// decoding and the in-memory [`RecoveryBundle`] representation amplify the
/// on-disk payload. Larger corpora must use the verified DuckDB backup path.
pub const PORTABLE_RECOVERY_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const PORTABLE_RECOVERY_MAX_DOCUMENTS: u64 = 10_000;
pub const PORTABLE_RECOVERY_MAX_CHUNKS: u64 = 50_000;
pub const PORTABLE_RECOVERY_LIMIT_GUIDANCE: &str =
    "use backup_db (or `recovery backup`) and its verification sidecars to create a verified DuckDB backup for this large corpus";

const PORTABLE_DOCUMENT_OVERHEAD_BYTES: u64 = 1_024;
const PORTABLE_CHUNK_OVERHEAD_BYTES: u64 = 512;
const PORTABLE_MANIFEST_OVERHEAD_BYTES: u64 = 1_024;
const JSON_STRING_WORST_CASE_FACTOR: u64 = 6;
const JSON_FLOAT_WORST_CASE_BYTES: u64 = 64;

const DATABASE_ARTIFACT: usize = 0;
const SHA256_ARTIFACT: usize = 1;
const METADATA_ARTIFACT: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackupPublishStage {
    SnapshotConnectionReady,
    SidecarPublished,
    MainPublishedAndVerified,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryBundle {
    pub format: String,
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    /// Corpus-wide identity for every serialized chunk embedding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_manifest: Option<EmbeddingManifest>,
    pub documents: Vec<BundleDocument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleDocument {
    pub document: Document,
    #[serde(default)]
    pub chunks: Vec<Chunk>,
}

/// Allocation-safe preflight summary for portable recovery materialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PortableBundlePreflight {
    pub documents: u64,
    pub chunks: u64,
    pub embedding_values: u64,
    /// Conservative upper bound used before rows or JSON are materialized.
    pub estimated_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    Error,
    Skip,
    Overwrite,
}

impl ConflictPolicy {
    pub fn parse(value: Option<&str>) -> Result<Self> {
        match value
            .unwrap_or("error")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "error" => Ok(Self::Error),
            "skip" => Ok(Self::Skip),
            "overwrite" => Ok(Self::Overwrite),
            other => Err(AppError::config(format!(
                "invalid conflict_policy '{other}': expected error, skip, or overwrite"
            ))),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct BackupReport {
    pub success: bool,
    pub dry_run: bool,
    pub source: String,
    pub path: String,
    pub overwritten: bool,
    pub bytes: Option<u64>,
    pub errors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<BackupVerification>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupVerification {
    pub ok: bool,
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub schema_version: i32,
    pub documents: u64,
    pub chunks: u64,
    pub nodes: u64,
    pub edges: u64,
    pub orphan_chunks: u64,
    pub orphan_document_nodes: u64,
    pub orphan_edges: u64,
    /// Every stored vector is covered by one canonical, complete manifest and
    /// has the manifest's declared dimensionality.
    pub embedding_contract_ok: bool,
    pub embedding_manifest: Option<crate::models::EmbeddingManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupSidecar {
    pub format: String,
    pub created_at: DateTime<Utc>,
    pub source: String,
    pub required_free_bytes: u64,
    pub verification: BackupVerification,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupInventoryItem {
    pub path: String,
    pub bytes: u64,
    pub modified_at: DateTime<Utc>,
    pub protected: bool,
    pub newest: bool,
}

#[derive(Debug, Serialize)]
pub struct BundleExportReport {
    pub success: bool,
    pub dry_run: bool,
    pub path: String,
    pub format: String,
    pub overwritten: bool,
    pub documents: u64,
    pub chunks: u64,
    pub bytes: Option<u64>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct BundleImportReport {
    pub success: bool,
    pub dry_run: bool,
    /// True only when this call committed document/chunk mutations.
    pub durable_mutation_committed: bool,
    /// Original legacy format version when the gateway upgraded it in memory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy_bundle_version: Option<u32>,
    /// Whether the caller explicitly authorized replacing legacy vectors.
    pub legacy_reembed_requested: bool,
    /// Legacy chunk vectors that would be replaced on apply.
    pub embeddings_reembed_planned: u64,
    /// Legacy chunk vectors actually replaced before this import attempt.
    pub embeddings_reembedded: u64,
    pub path: String,
    pub format: String,
    pub conflict_policy: String,
    pub documents_read: u64,
    pub documents_inserted: u64,
    pub documents_overwritten: u64,
    pub documents_skipped: u64,
    pub chunks_inserted: u64,
    pub conflicts: u64,
    pub errors: Vec<String>,
}

fn portable_recovery_limit_error(reason: impl std::fmt::Display) -> AppError {
    AppError::config(format!(
        "portable recovery bundle {reason}; limit is {} bytes, {} documents, and {} chunks; {PORTABLE_RECOVERY_LIMIT_GUIDANCE}",
        PORTABLE_RECOVERY_MAX_BYTES,
        PORTABLE_RECOVERY_MAX_DOCUMENTS,
        PORTABLE_RECOVERY_MAX_CHUNKS,
    ))
}

fn ensure_portable_preflight(
    preflight: PortableBundlePreflight,
) -> Result<PortableBundlePreflight> {
    if preflight.documents > PORTABLE_RECOVERY_MAX_DOCUMENTS {
        return Err(portable_recovery_limit_error(format!(
            "contains {} documents",
            preflight.documents
        )));
    }
    if preflight.chunks > PORTABLE_RECOVERY_MAX_CHUNKS {
        return Err(portable_recovery_limit_error(format!(
            "contains {} chunks",
            preflight.chunks
        )));
    }
    if preflight.estimated_bytes > PORTABLE_RECOVERY_MAX_BYTES {
        return Err(portable_recovery_limit_error(format!(
            "has a conservative materialized-size estimate of {} bytes",
            preflight.estimated_bytes
        )));
    }
    Ok(preflight)
}

fn ensure_portable_encoded_bytes(bytes: u64) -> Result<()> {
    if bytes > PORTABLE_RECOVERY_MAX_BYTES {
        return Err(portable_recovery_limit_error(format!(
            "serialized to {bytes} bytes"
        )));
    }
    Ok(())
}

fn saturating_len(value: &str) -> u64 {
    u64::try_from(value.len()).unwrap_or(u64::MAX)
}

fn add_json_string_estimate(total: &mut u64, value: &str) {
    *total =
        total.saturating_add(saturating_len(value).saturating_mul(JSON_STRING_WORST_CASE_FACTOR));
}

fn portable_preflight_from_bundle(
    bundle: &RecoveryBundle,
    reembed_dimensions: Option<usize>,
) -> Result<PortableBundlePreflight> {
    let documents = u64::try_from(bundle.documents.len()).unwrap_or(u64::MAX);
    let mut chunks = 0u64;
    let mut embedding_values = 0u64;
    let mut string_bytes = 0u64;

    add_json_string_estimate(&mut string_bytes, &bundle.format);
    if let Some(manifest) = &bundle.embedding_manifest {
        for value in [
            Some(manifest.id.as_str()),
            Some(manifest.provider.as_str()),
            Some(manifest.model.as_str()),
            manifest.base_url.as_deref(),
            manifest.content_fingerprint.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            add_json_string_estimate(&mut string_bytes, value);
        }
    }

    for item in &bundle.documents {
        chunks = chunks.saturating_add(u64::try_from(item.chunks.len()).unwrap_or(u64::MAX));
        let document = &item.document;
        for value in [
            Some(document.id.as_str()),
            Some(document.uri.as_str()),
            Some(document.title.as_str()),
            Some(document.content.as_str()),
            Some(document.metadata_json.as_str()),
            document.content_hash.as_deref(),
            document.wing.as_deref(),
            document.room.as_deref(),
            document.source_file.as_deref(),
            Some(document.layer.as_str()),
            Some(document.kind.as_str()),
            Some(document.status.as_str()),
        ]
        .into_iter()
        .flatten()
        {
            add_json_string_estimate(&mut string_bytes, value);
        }
        for chunk in &item.chunks {
            for value in [
                chunk.id.as_str(),
                chunk.document_id.as_str(),
                chunk.content.as_str(),
                chunk.metadata_json.as_str(),
            ] {
                add_json_string_estimate(&mut string_bytes, value);
            }
            let dimensions = reembed_dimensions.unwrap_or(chunk.embedding.len());
            embedding_values =
                embedding_values.saturating_add(u64::try_from(dimensions).unwrap_or(u64::MAX));
        }
    }

    let estimated_bytes = PORTABLE_MANIFEST_OVERHEAD_BYTES
        .saturating_add(documents.saturating_mul(PORTABLE_DOCUMENT_OVERHEAD_BYTES))
        .saturating_add(chunks.saturating_mul(PORTABLE_CHUNK_OVERHEAD_BYTES))
        .saturating_add(string_bytes)
        .saturating_add(embedding_values.saturating_mul(JSON_FLOAT_WORST_CASE_BYTES));
    ensure_portable_preflight(PortableBundlePreflight {
        documents,
        chunks,
        embedding_values,
        estimated_bytes,
    })
}

/// Validate a materialized portable bundle before encoding or importing it.
pub fn preflight_recovery_bundle(bundle: &RecoveryBundle) -> Result<PortableBundlePreflight> {
    portable_preflight_from_bundle(bundle, None)
}

/// Validate the prospective allocation before replacing every chunk vector.
pub fn preflight_recovery_bundle_reembed(
    bundle: &RecoveryBundle,
    dimensions: usize,
) -> Result<PortableBundlePreflight> {
    portable_preflight_from_bundle(bundle, Some(dimensions))
}

/// Read a portable recovery file without permitting an unbounded allocation.
///
/// Metadata is checked first for the common path, then a `MAX + 1` reader closes
/// the replacement/growth race before an exact post-read check.
pub fn read_recovery_bundle_file(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(AppError::config(format!(
            "recovery bundle '{}' is not a regular file",
            path.display()
        )));
    }
    ensure_portable_encoded_bytes(metadata.len())?;
    let capacity = usize::try_from(metadata.len()).map_err(|_| {
        portable_recovery_limit_error(format!(
            "file '{}' is too large for this platform",
            path.display()
        ))
    })?;
    let mut input = BoundedRecoveryWriter::with_capacity(capacity)?;
    let mut reader = fs::File::open(path)?.take(PORTABLE_RECOVERY_MAX_BYTES.saturating_add(1));
    if let Err(error) = std::io::copy(&mut reader, &mut input) {
        if input.limit_exceeded {
            return Err(portable_recovery_limit_error(
                "exceeds the input byte limit",
            ));
        }
        return Err(error.into());
    }
    let bytes = input.finish()?;
    String::from_utf8(bytes).map_err(|error| {
        AppError::config(format!(
            "recovery bundle '{}' must contain UTF-8 JSON: {}",
            path.display(),
            error.utf8_error()
        ))
    })
}

struct BoundedRecoveryWriter {
    bytes: Vec<u8>,
    limit_exceeded: bool,
}

impl BoundedRecoveryWriter {
    fn new() -> Self {
        Self {
            bytes: Vec::new(),
            limit_exceeded: false,
        }
    }

    fn with_capacity(capacity: usize) -> Result<Self> {
        let mut writer = Self::new();
        writer.bytes.try_reserve_exact(capacity).map_err(|_| {
            portable_recovery_limit_error("cannot be allocated safely on this host")
        })?;
        Ok(writer)
    }

    fn finish(self) -> Result<Vec<u8>> {
        ensure_portable_encoded_bytes(u64::try_from(self.bytes.len()).unwrap_or(u64::MAX))?;
        Ok(self.bytes)
    }
}

impl Write for BoundedRecoveryWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let next_len = self.bytes.len().checked_add(buffer.len()).ok_or_else(|| {
            self.limit_exceeded = true;
            std::io::Error::other("portable recovery bundle byte count overflow")
        })?;
        if u64::try_from(next_len).unwrap_or(u64::MAX) > PORTABLE_RECOVERY_MAX_BYTES {
            self.limit_exceeded = true;
            return Err(std::io::Error::other(format!(
                "portable recovery bundle exceeds {} bytes; {PORTABLE_RECOVERY_LIMIT_GUIDANCE}",
                PORTABLE_RECOVERY_MAX_BYTES
            )));
        }
        let additional = next_len.saturating_sub(self.bytes.len());
        self.bytes
            .try_reserve_exact(additional)
            .map_err(|_| std::io::Error::other("portable recovery bundle allocation failed"))?;
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Serialize)]
struct JsonlManifestRecord<'a> {
    record_type: &'static str,
    format: &'a str,
    version: u32,
    exported_at: &'a DateTime<Utc>,
    embedding_manifest: &'a Option<EmbeddingManifest>,
}

#[derive(Serialize)]
struct JsonlDocumentRecord<'a> {
    record_type: &'static str,
    value: &'a BundleDocument,
}

/// Encode JSON/JSONL directly into a hard-bounded buffer.
pub fn encode_recovery_bundle(bundle: &RecoveryBundle, format: &str) -> Result<Vec<u8>> {
    preflight_recovery_bundle(bundle)?;
    let mut writer = BoundedRecoveryWriter::new();
    let encoded = match format {
        "json" => serde_json::to_writer_pretty(&mut writer, bundle),
        "jsonl" | "ndjson" => {
            let manifest = JsonlManifestRecord {
                record_type: "manifest",
                format: &bundle.format,
                version: bundle.version,
                exported_at: &bundle.exported_at,
                embedding_manifest: &bundle.embedding_manifest,
            };
            serde_json::to_writer(&mut writer, &manifest)
                .and_then(|()| writer.write_all(b"\n").map_err(serde_json::Error::io))
                .and_then(|()| {
                    for item in &bundle.documents {
                        serde_json::to_writer(
                            &mut writer,
                            &JsonlDocumentRecord {
                                record_type: "document",
                                value: item,
                            },
                        )?;
                        writer.write_all(b"\n").map_err(serde_json::Error::io)?;
                    }
                    Ok(())
                })
        }
        _ => {
            return Err(AppError::config(
                "recovery bundle format must be json or jsonl",
            ))
        }
    };
    if let Err(error) = encoded {
        if writer.limit_exceeded {
            return Err(portable_recovery_limit_error(
                "exceeds the serialized byte limit",
            ));
        }
        return Err(error.into());
    }
    writer.finish()
}

/// Decode either recovery JSON or the line-oriented recovery transport.
///
/// Recovery files are durable interchange artifacts, so the decoder is
/// intentionally strict: unknown fields are rejected at every nesting level,
/// and v2 document records must explicitly carry their `chunks` array. Legacy
/// v1 metadata-only records may omit `chunks` and deserialize it as empty.
pub fn decode_recovery_bundle(input: &str, format: &str) -> Result<RecoveryBundle> {
    ensure_portable_encoded_bytes(u64::try_from(input.len()).unwrap_or(u64::MAX))?;
    if format == "json" {
        let value: serde_json::Value = serde_json::from_str(input)?;
        validate_recovery_bundle_json_shape(&value)?;
        let bundle = serde_json::from_value(value)?;
        preflight_recovery_bundle(&bundle)?;
        return Ok(bundle);
    }
    if format != "jsonl" && format != "ndjson" {
        return Err(AppError::config(
            "recovery bundle format must be json or jsonl",
        ));
    }

    let mut manifest = None;
    let mut documents: Vec<BundleDocument> = Vec::new();
    let mut chunks = 0u64;
    for (line_index, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let line_no = line_index + 1;
        let value: serde_json::Value = serde_json::from_str(line)
            .map_err(|error| AppError::config(format!("invalid JSONL line {line_no}: {error}")))?;
        let object = value.as_object().ok_or_else(|| {
            AppError::config(format!("JSONL record at line {line_no} must be an object"))
        })?;
        match object
            .get("record_type")
            .and_then(serde_json::Value::as_str)
        {
            Some("manifest") => {
                ensure_only_fields(
                    object,
                    &[
                        "record_type",
                        "format",
                        "version",
                        "exported_at",
                        "embedding_manifest",
                    ],
                    &format!("JSONL manifest at line {line_no}"),
                )?;
                if manifest.is_some() {
                    return Err(AppError::config(format!(
                        "duplicate JSONL manifest at line {line_no}"
                    )));
                }
                if !documents.is_empty() {
                    return Err(AppError::config(format!(
                        "JSONL manifest must precede documents (line {line_no})"
                    )));
                }
                let bundle_format = object
                    .get("format")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        AppError::config(format!(
                            "JSONL manifest at line {line_no} requires string field 'format'"
                        ))
                    })?
                    .to_owned();
                let raw_version = object
                    .get("version")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| {
                        AppError::config(format!(
                            "JSONL manifest at line {line_no} requires unsigned integer field 'version'"
                        ))
                    })?;
                let version = u32::try_from(raw_version).map_err(|_| {
                    AppError::config(format!(
                        "JSONL manifest version {raw_version} at line {line_no} exceeds u32"
                    ))
                })?;
                if version == 0 {
                    return Err(AppError::config(format!(
                        "JSONL manifest version must be greater than zero at line {line_no}"
                    )));
                }
                let exported_at = object.get("exported_at").cloned().ok_or_else(|| {
                    AppError::config(format!(
                        "JSONL manifest at line {line_no} requires 'exported_at'"
                    ))
                })?;
                let exported_at =
                    serde_json::from_value::<DateTime<Utc>>(exported_at).map_err(|error| {
                        AppError::config(format!(
                            "invalid JSONL manifest exported_at at line {line_no}: {error}"
                        ))
                    })?;
                let embedding_manifest = object
                    .get("embedding_manifest")
                    .filter(|value| !value.is_null())
                    .map(|value| {
                        validate_embedding_manifest_json_shape(
                            value,
                            &format!("JSONL manifest embedding_manifest at line {line_no}"),
                        )?;
                        serde_json::from_value(value.clone()).map_err(AppError::from)
                    })
                    .transpose()?;
                manifest = Some((bundle_format, version, exported_at, embedding_manifest));
            }
            Some("document") => {
                ensure_only_fields(
                    object,
                    &["record_type", "value"],
                    &format!("JSONL document at line {line_no}"),
                )?;
                if manifest.is_none() {
                    return Err(AppError::config(format!(
                        "JSONL document precedes manifest (appears before the manifest) at line {line_no}"
                    )));
                }
                let document_value = object.get("value").ok_or_else(|| {
                    AppError::config(format!(
                        "JSONL document at line {line_no} requires field 'value'"
                    ))
                })?;
                let version = manifest
                    .as_ref()
                    .map(|(_, version, _, _)| *version)
                    .expect("manifest presence checked above");
                validate_bundle_document_json_shape(
                    document_value,
                    u64::from(version),
                    &format!("JSONL document value at line {line_no}"),
                )?;
                let document: BundleDocument = serde_json::from_value(document_value.clone())?;
                let next_documents = u64::try_from(documents.len())
                    .unwrap_or(u64::MAX)
                    .saturating_add(1);
                chunks =
                    chunks.saturating_add(u64::try_from(document.chunks.len()).unwrap_or(u64::MAX));
                ensure_portable_preflight(PortableBundlePreflight {
                    documents: next_documents,
                    chunks,
                    embedding_values: 0,
                    estimated_bytes: 0,
                })?;
                documents.push(document);
            }
            other => {
                return Err(AppError::config(format!(
                    "unknown JSONL record_type {other:?} at line {line_no}"
                )))
            }
        }
    }
    let (bundle_format, version, exported_at, embedding_manifest) = manifest.ok_or_else(|| {
        AppError::config(
            "JSONL recovery bundle is missing its manifest; requires exactly one manifest record",
        )
    })?;
    let bundle = RecoveryBundle {
        format: bundle_format,
        version,
        exported_at,
        embedding_manifest,
        documents,
    };
    preflight_recovery_bundle(&bundle)?;
    Ok(bundle)
}

fn validate_embedding_manifest_json_shape(value: &serde_json::Value, context: &str) -> Result<()> {
    let manifest = value
        .as_object()
        .ok_or_else(|| AppError::config(format!("{context} must be an object")))?;
    ensure_only_fields(
        manifest,
        &[
            "id",
            "provider",
            "model",
            "dims",
            "base_url",
            "content_fingerprint",
            "updated_at",
        ],
        context,
    )
}

fn validate_bundle_document_json_shape(
    item: &serde_json::Value,
    version: u64,
    context: &str,
) -> Result<()> {
    let item = item
        .as_object()
        .ok_or_else(|| AppError::config(format!("{context} must be an object")))?;
    ensure_only_fields(item, &["document", "chunks"], context)?;
    if version >= u64::from(BUNDLE_VERSION) && !item.contains_key("chunks") {
        return Err(AppError::config(format!(
            "{context} requires explicit field 'chunks' in recovery bundle v{version}"
        )));
    }
    let document = item
        .get("document")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| AppError::config(format!("{context}.document must be an object")))?;
    ensure_only_fields(
        document,
        &[
            "id",
            "uri",
            "title",
            "content",
            "metadata_json",
            "created_at",
            "updated_at",
            "wing",
            "room",
            "source_file",
            "layer",
            "kind",
            "content_hash",
            "status",
            "pinned",
            "boost",
            "revision",
        ],
        &format!("{context}.document"),
    )?;
    let chunks = match item.get("chunks") {
        Some(chunks) => chunks
            .as_array()
            .ok_or_else(|| AppError::config(format!("{context}.chunks must be an array")))?,
        None => return Ok(()),
    };
    for (chunk_index, chunk) in chunks.iter().enumerate() {
        let chunk_context = format!("{context}.chunks[{chunk_index}]");
        let chunk = chunk
            .as_object()
            .ok_or_else(|| AppError::config(format!("{chunk_context} must be an object")))?;
        ensure_only_fields(
            chunk,
            &[
                "id",
                "document_id",
                "chunk_index",
                "content",
                "embedding",
                "char_start",
                "char_end",
                "metadata_json",
            ],
            &chunk_context,
        )?;
    }
    Ok(())
}

fn validate_recovery_bundle_json_shape(value: &serde_json::Value) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| AppError::config("recovery bundle root must be a JSON object"))?;
    ensure_only_fields(
        object,
        &[
            "format",
            "version",
            "exported_at",
            "embedding_manifest",
            "documents",
        ],
        "recovery bundle",
    )?;
    let version = object
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| AppError::config("recovery bundle requires unsigned integer 'version'"))?;
    let documents = object
        .get("documents")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| AppError::config("recovery bundle requires array 'documents'"))?;
    if let Some(manifest) = object
        .get("embedding_manifest")
        .filter(|manifest| !manifest.is_null())
    {
        validate_embedding_manifest_json_shape(manifest, "recovery bundle embedding_manifest")?;
    }
    for (document_index, item) in documents.iter().enumerate() {
        let context = format!("recovery bundle documents[{document_index}]");
        validate_bundle_document_json_shape(item, version, &context)?;
    }
    Ok(())
}

fn ensure_only_fields(
    object: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
    context: &str,
) -> Result<()> {
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return Err(AppError::config(format!(
            "{context} contains unknown field '{field}'"
        )));
    }
    Ok(())
}

fn nonnegative_metric(value: i64, name: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| {
        AppError::db(format!(
            "portable recovery preflight returned negative {name}"
        ))
    })
}

fn portable_store_preflight_locked(conn: &duckdb::Connection) -> Result<PortableBundlePreflight> {
    let (documents_raw, document_string_bytes_raw): (i64, i64) = conn.query_row(
        r#"
        SELECT
            COUNT(*),
            CAST(COALESCE(SUM(
                strlen(id) + strlen(uri) + strlen(title) + strlen(content) +
                strlen(metadata_json) + strlen(COALESCE(content_hash, '')) +
                strlen(COALESCE(wing, '')) + strlen(COALESCE(room, '')) +
                strlen(COALESCE(source_file, '')) + strlen(COALESCE(layer, 'raw')) +
                strlen(COALESCE(kind, 'document')) + strlen(COALESCE(status, 'active'))
            ), 0) AS BIGINT)
        FROM documents
        "#,
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let (chunks_raw, chunk_string_bytes_raw, embedding_json_bytes_raw, embedding_values_raw): (
        i64,
        i64,
        i64,
        i64,
    ) = conn.query_row(
        r#"
        SELECT
            COUNT(*),
            CAST(COALESCE(SUM(
                strlen(id) + strlen(document_id) + strlen(content) +
                strlen(COALESCE(metadata_json, '{}'))
            ), 0) AS BIGINT),
            CAST(COALESCE(SUM(strlen(embedding_json)), 0) AS BIGINT),
            CAST(COALESCE(SUM(
                CASE
                    WHEN trim(embedding_json) = '[]' THEN 0
                    ELSE 1 + strlen(embedding_json) - strlen(replace(embedding_json, ',', ''))
                END
            ), 0) AS BIGINT)
        FROM chunks
        "#,
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let manifest_string_bytes_raw: i64 = conn.query_row(
        r#"
        SELECT CAST(COALESCE(SUM(
            strlen(id) + strlen(provider) + strlen(model) +
            strlen(COALESCE(base_url, '')) + strlen(COALESCE(content_fingerprint, ''))
        ), 0) AS BIGINT)
        FROM embedding_manifest
        WHERE id = 'default'
        "#,
        [],
        |row| row.get(0),
    )?;

    let documents = nonnegative_metric(documents_raw, "document count")?;
    let chunks = nonnegative_metric(chunks_raw, "chunk count")?;
    let document_string_bytes =
        nonnegative_metric(document_string_bytes_raw, "document byte count")?;
    let chunk_string_bytes = nonnegative_metric(chunk_string_bytes_raw, "chunk byte count")?;
    let embedding_json_bytes =
        nonnegative_metric(embedding_json_bytes_raw, "embedding JSON byte count")?;
    let embedding_values = nonnegative_metric(embedding_values_raw, "embedding value count")?;
    let manifest_string_bytes =
        nonnegative_metric(manifest_string_bytes_raw, "manifest byte count")?;
    let string_bytes = document_string_bytes
        .saturating_add(chunk_string_bytes)
        .saturating_add(manifest_string_bytes);
    let estimated_bytes = PORTABLE_MANIFEST_OVERHEAD_BYTES
        .saturating_add(documents.saturating_mul(PORTABLE_DOCUMENT_OVERHEAD_BYTES))
        .saturating_add(chunks.saturating_mul(PORTABLE_CHUNK_OVERHEAD_BYTES))
        .saturating_add(string_bytes.saturating_mul(JSON_STRING_WORST_CASE_FACTOR))
        .saturating_add(embedding_json_bytes)
        .saturating_add(embedding_values.saturating_mul(JSON_FLOAT_WORST_CASE_BYTES));
    ensure_portable_preflight(PortableBundlePreflight {
        documents,
        chunks,
        embedding_values,
        estimated_bytes,
    })
}

impl Store {
    /// Refuse a portable export before loading all document and chunk rows.
    pub fn portable_recovery_preflight(&self) -> Result<PortableBundlePreflight> {
        let conn = self.lock()?;
        portable_store_preflight_locked(&conn)
    }

    pub fn backup_database(
        &self,
        path: &Path,
        dry_run: bool,
        overwrite: bool,
    ) -> Result<BackupReport> {
        self.backup_database_with_publish_hook(path, dry_run, overwrite, |_| Ok(()))
    }

    fn backup_database_with_publish_hook<F>(
        &self,
        path: &Path,
        dry_run: bool,
        overwrite: bool,
        mut publish_hook: F,
    ) -> Result<BackupReport>
    where
        F: FnMut(BackupPublishStage) -> Result<()>,
    {
        // A backup is a three-file publication. Keep every generation for the
        // same destination serialized from preflight through final verification
        // so concurrent overwrite requests cannot interleave their sidecars.
        let destination_lock = backup_publication_lock(path);
        let _destination_guard = destination_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let artifacts = backup_artifact_paths(path);
        let mut artifact_exists = [false; 3];
        for (index, artifact) in artifacts.iter().enumerate() {
            artifact_exists[index] = preflight_backup_target(
                self.path(),
                artifact,
                overwrite,
                if index == 0 {
                    "backup"
                } else {
                    "backup sidecar"
                },
            )?;
        }
        let exists = artifact_exists[0];
        let source = self.path().display().to_string();
        if dry_run {
            return Ok(BackupReport {
                success: true,
                dry_run: true,
                source,
                path: path.display().to_string(),
                overwritten: exists,
                bytes: self.db_file_size_bytes(),
                errors: Vec::new(),
                sha256: None,
                verification: None,
            });
        }

        let snapshot_conn = {
            let conn = self.lock()?;
            conn.execute_batch("CHECKPOINT")?;
            conn.try_clone()?
        };
        let mut publication = BackupGroupPublication::new(artifacts, overwrite);
        let (database_stage, bytes) =
            stage_database_snapshot(snapshot_conn, self.path(), path, || {
                publish_hook(BackupPublishStage::SnapshotConnectionReady)
            })?;
        publication.set_staged(DATABASE_ARTIFACT, database_stage);

        let mut verification = verify_backup(publication.staged_path(DATABASE_ARTIFACT)?)?;
        if !verification.ok {
            return Err(AppError::db(format!(
                "staged backup '{}' failed relational or embedding-contract verification",
                path.display()
            )));
        }
        sync_file(publication.staged_path(DATABASE_ARTIFACT)?)?;
        // Sidecars describe the durable destination, not the private staging name.
        verification.path = path.display().to_string();
        let sidecar_bytes = backup_sidecar_bytes(path, self.path(), &verification)?;
        publication.set_staged(
            SHA256_ARTIFACT,
            stage_bytes(
                &publication.artifacts[SHA256_ARTIFACT],
                &sidecar_bytes.sha256,
            )?,
        );
        publication.set_staged(
            METADATA_ARTIFACT,
            stage_bytes(
                &publication.artifacts[METADATA_ARTIFACT],
                &sidecar_bytes.metadata,
            )?,
        );

        // Persist all three staged files and their directory entries before any
        // public name changes. The database destination is deliberately the
        // final publish: for a new (overwrite=false) group, its presence is the
        // commit marker and both sidecars are already durable at that point.
        sync_parent_directory(path)?;
        publication.preserve_overwritten_artifacts()?;
        sync_parent_directory(path)?;
        publication.publish(SHA256_ARTIFACT)?;
        sync_parent_directory(path)?;
        publish_hook(BackupPublishStage::SidecarPublished)?;
        publication.publish(METADATA_ARTIFACT)?;
        sync_parent_directory(path)?;

        publication.publish(DATABASE_ARTIFACT)?;
        sync_parent_directory(path)?;
        let final_verification = verify_published_backup_group(path, self.path())?;
        if !backup_verifications_match(&final_verification, &verification)? {
            return Err(AppError::db(format!(
                "published backup group '{}' does not match its verified staging generation",
                path.display()
            )));
        }
        publish_hook(BackupPublishStage::MainPublishedAndVerified)?;
        publication.commit();

        Ok(BackupReport {
            success: true,
            dry_run: false,
            source,
            path: path.display().to_string(),
            overwritten: exists,
            bytes: Some(bytes),
            errors: Vec::new(),
            sha256: Some(final_verification.sha256.clone()),
            verification: Some(final_verification),
        })
    }

    pub fn recovery_bundle(&self) -> Result<RecoveryBundle> {
        let conn = self.lock()?;
        portable_store_preflight_locked(&conn)?;
        let mut stmt = conn.prepare(
            "SELECT id, uri, title, content, metadata_json, content_hash, wing, room, source_file, layer, kind, CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR), COALESCE(status, 'active'), COALESCE(pinned, false), COALESCE(boost, 1.0), COALESCE(revision, 1) FROM documents ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Document {
                id: row.get(0)?,
                uri: row.get(1)?,
                title: row.get(2)?,
                content: row.get(3)?,
                metadata_json: row.get(4)?,
                content_hash: row.get(5)?,
                wing: row.get(6)?,
                room: row.get(7)?,
                source_file: row.get(8)?,
                layer: row.get(9)?,
                kind: row.get(10)?,
                created_at: parse_ts(row.get::<_, String>(11)?),
                updated_at: parse_ts(row.get::<_, String>(12)?),
                status: row.get(13)?,
                pinned: row.get(14)?,
                boost: row.get(15)?,
                revision: row.get(16)?,
            })
        })?;
        let documents = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);

        let mut bundled = Vec::with_capacity(documents.len());
        for document in documents {
            let mut chunks_stmt = conn.prepare(
                "SELECT id, document_id, chunk_index, content, embedding_json, char_start, char_end, COALESCE(metadata_json, '{}') FROM chunks WHERE document_id = ? ORDER BY chunk_index",
            )?;
            let mut rows = chunks_stmt.query(params![document.id])?;
            let mut chunks = Vec::new();
            while let Some(row) = rows.next()? {
                let raw: String = row.get(4)?;
                let embedding = serde_json::from_str(&raw).map_err(|error| {
                    AppError::db(format!(
                        "invalid embedding JSON for chunk {} in recovery export: {error}",
                        row.get::<_, String>(0).unwrap_or_else(|_| "unknown".into())
                    ))
                })?;
                chunks.push(Chunk {
                    id: row.get(0)?,
                    document_id: row.get(1)?,
                    chunk_index: row.get(2)?,
                    content: row.get(3)?,
                    embedding,
                    char_start: row.get(5)?,
                    char_end: row.get(6)?,
                    metadata_json: row.get(7)?,
                });
            }
            bundled.push(BundleDocument { document, chunks });
        }
        let embedding_manifest = {
            let mut manifest_stmt = conn.prepare(
                r#"
                SELECT id, provider, model, dims, base_url, content_fingerprint,
                       CAST(updated_at AS VARCHAR)
                FROM embedding_manifest
                WHERE id = 'default'
                "#,
            )?;
            let mut rows = manifest_stmt.query([])?;
            match rows.next()? {
                Some(row) => Some(super::rows::embedding_manifest(row)?),
                None => None,
            }
        };
        let bundle = RecoveryBundle {
            format: "rag-recovery-bundle".into(),
            version: BUNDLE_VERSION,
            exported_at: Utc::now(),
            embedding_manifest,
            documents: bundled,
        };
        validate_bundle_structure(&bundle)?;
        validate_bundle_embeddings(&bundle)?;
        preflight_recovery_bundle(&bundle)?;
        Ok(bundle)
    }

    pub fn import_recovery_bundle(
        &self,
        bundle: &RecoveryBundle,
        policy: ConflictPolicy,
        dry_run: bool,
        path: &Path,
        format: &str,
    ) -> Result<BundleImportReport> {
        preflight_recovery_bundle(bundle)?;
        if bundle.format != "rag-recovery-bundle" || bundle.version != BUNDLE_VERSION {
            return Err(AppError::config(format!(
                "unsupported recovery bundle format/version: {}/{}",
                bundle.format, bundle.version
            )));
        }
        validate_bundle_structure(bundle)?;
        let mut report = BundleImportReport {
            success: true,
            dry_run,
            durable_mutation_committed: false,
            legacy_bundle_version: None,
            legacy_reembed_requested: false,
            embeddings_reembed_planned: 0,
            embeddings_reembedded: 0,
            path: path.display().to_string(),
            format: format.into(),
            conflict_policy: match policy {
                ConflictPolicy::Error => "error",
                ConflictPolicy::Skip => "skip",
                ConflictPolicy::Overwrite => "overwrite",
            }
            .into(),
            documents_read: bundle.documents.len() as u64,
            documents_inserted: 0,
            documents_overwritten: 0,
            documents_skipped: 0,
            chunks_inserted: 0,
            conflicts: 0,
            errors: Vec::new(),
        };
        let bundle_manifest = validate_bundle_embeddings(bundle)?;
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        if let Some(bundle_manifest) = bundle_manifest {
            let target_chunk_count: i64 =
                tx.query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
            let target_manifest = {
                let mut stmt = tx.prepare(
                    r#"
                    SELECT id, provider, model, dims, base_url, content_fingerprint,
                           CAST(updated_at AS VARCHAR)
                    FROM embedding_manifest
                    WHERE id = 'default'
                    "#,
                )?;
                let mut rows = stmt.query([])?;
                match rows.next()? {
                    Some(row) => Some(super::rows::embedding_manifest(row)?),
                    None => None,
                }
            };
            match target_manifest.as_ref() {
                Some(target) => {
                    validate_embedding_manifest(target, "target corpus")?;
                    if !embedding_manifests_match(target, bundle_manifest) {
                        return Err(AppError::embeddings(format!(
                            "recovery bundle embedding identity does not match target corpus (bundle provider='{}', model='{}', dims={}; target provider='{}', model='{}', dims={})",
                            bundle_manifest.provider,
                            bundle_manifest.model,
                            bundle_manifest.dims,
                            target.provider,
                            target.model,
                            target.dims,
                        )));
                    }
                }
                None if target_chunk_count > 0 => {
                    return Err(AppError::embeddings(format!(
                        "target corpus has {target_chunk_count} chunks but no embedding_manifest; run a complete uncapped reembed_all before importing vectors"
                    )));
                }
                None => set_embedding_manifest_locked(&tx, bundle_manifest)?,
            }
        }
        let mut fts_marked_dirty = false;
        for item in &bundle.documents {
            let matching_document_ids = {
                let mut stmt =
                    tx.prepare("SELECT id FROM documents WHERE id = ? OR uri = ? ORDER BY id ASC")?;
                let rows = stmt.query_map(params![item.document.id, item.document.uri], |row| {
                    row.get::<_, String>(0)
                })?;
                rows.collect::<std::result::Result<Vec<_>, _>>()?
            };
            if matching_document_ids.len() > 1 {
                return Err(AppError::conflict(format!(
                    "recovery bundle document {} ({}) matches multiple existing documents by id/uri: {}",
                    item.document.id,
                    item.document.uri,
                    matching_document_ids.join(", ")
                )));
            }
            let existing_document_id = matching_document_ids.first();
            let exists = existing_document_id.is_some();
            if exists {
                report.conflicts += 1;
                match policy {
                    ConflictPolicy::Error => {
                        report.success = false;
                        report.errors.push(format!(
                            "document conflict: {} ({})",
                            item.document.id, item.document.uri
                        ));
                        continue;
                    }
                    ConflictPolicy::Skip => {
                        report.documents_skipped += 1;
                        continue;
                    }
                    ConflictPolicy::Overwrite => {
                        report.documents_overwritten += 1;
                    }
                }
            } else {
                report.documents_inserted += 1;
            }
            report.chunks_inserted += item.chunks.len() as u64;
            if let Some(existing_document_id) = existing_document_id {
                super::store::delete_document_locked(&tx, existing_document_id)?;
                fts_marked_dirty = true;
            } else if !fts_marked_dirty && !item.chunks.is_empty() {
                super::fts::mark_fts_dirty(&tx)?;
                fts_marked_dirty = true;
            }
            let d = &item.document;
            tx.execute(
                "INSERT INTO documents (id, uri, title, content, metadata_json, content_hash, wing, room, source_file, layer, kind, status, pinned, boost, revision, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
                params![d.id, d.uri, d.title, d.content, d.metadata_json, d.content_hash, d.wing, d.room, d.source_file, d.layer, d.kind, d.status, d.pinned, d.boost, d.revision, d.created_at.to_rfc3339(), d.updated_at.to_rfc3339()],
            )?;
            for c in &item.chunks {
                tx.execute(
                    "INSERT INTO chunks (id, document_id, chunk_index, content, embedding_json, char_start, char_end, metadata_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)",
                    params![c.id, d.id, c.chunk_index, c.content, serde_json::to_string(&c.embedding)?, c.char_start, c.char_end, c.metadata_json],
                )?;
            }
        }
        if report.success && !dry_run {
            tx.commit()?;
            report.durable_mutation_committed =
                report.documents_inserted > 0 || report.documents_overwritten > 0;
        }
        Ok(report)
    }
}

fn embedding_manifests_match(left: &EmbeddingManifest, right: &EmbeddingManifest) -> bool {
    if left.id != "default"
        || right.id != "default"
        || left.provider != right.provider
        || left.model != right.model
        || left.dims != right.dims
        || left.base_url != right.base_url
    {
        return false;
    }
    match (
        left.content_fingerprint.as_deref(),
        right.content_fingerprint.as_deref(),
    ) {
        (Some(left), Some(right)) => left == right,
        (None, None) => true,
        _ => false,
    }
}

/// Rebuild the canonical embedding identity fingerprint from serialized fields.
///
/// This deliberately mirrors `store::embedding_config_fingerprint`: recovery
/// manifests are untrusted input, so their persisted fingerprint must describe
/// the provider/model/dimensions/base URL carried alongside it.
fn canonical_embedding_manifest_fingerprint(manifest: &EmbeddingManifest) -> Result<String> {
    if manifest.id != "default" {
        return Err(AppError::embeddings(format!(
            "embedding manifest id must be 'default' (got '{}')",
            manifest.id
        )));
    }
    if manifest.dims <= 0 {
        return Err(AppError::embeddings(format!(
            "embedding manifest has invalid dims={}",
            manifest.dims
        )));
    }
    let base_url = manifest.base_url.as_deref().ok_or_else(|| {
        AppError::embeddings(
            "embedding manifest has no base_url; its canonical fingerprint cannot be verified",
        )
    })?;
    Ok(embedding_identity_fingerprint(
        &manifest.provider,
        &manifest.model,
        manifest.dims,
        base_url,
    ))
}

fn validate_embedding_manifest(manifest: &EmbeddingManifest, context: &str) -> Result<()> {
    if manifest
        .content_fingerprint
        .as_deref()
        .is_some_and(|fingerprint| fingerprint.starts_with("migration-incomplete:"))
    {
        return Err(AppError::embeddings(format!(
            "{context} embedding manifest records an incomplete embedding migration"
        )));
    }
    let expected = canonical_embedding_manifest_fingerprint(manifest)?;
    let actual = manifest.content_fingerprint.as_deref().ok_or_else(|| {
        AppError::embeddings(format!(
            "{context} embedding manifest has no canonical content_fingerprint"
        ))
    })?;
    if actual != expected {
        return Err(AppError::embeddings(format!(
            "{context} embedding manifest content_fingerprint does not match its provider/model/dims/base_url identity"
        )));
    }
    Ok(())
}

fn validate_bundle_structure(bundle: &RecoveryBundle) -> Result<()> {
    for (document_index, item) in bundle.documents.iter().enumerate() {
        let document_id = item.document.id.trim();
        if document_id.is_empty() {
            return Err(AppError::config(format!(
                "recovery bundle documents[{document_index}] has an empty document id"
            )));
        }
        if item.document.uri.trim().is_empty() {
            return Err(AppError::config(format!(
                "recovery bundle document '{document_id}' has an empty uri"
            )));
        }
        for (chunk_index, chunk) in item.chunks.iter().enumerate() {
            if chunk.id.trim().is_empty() {
                return Err(AppError::config(format!(
                    "recovery bundle document '{document_id}' chunk[{chunk_index}] has an empty id"
                )));
            }
            if chunk.document_id != item.document.id {
                return Err(AppError::config(format!(
                    "recovery bundle chunk '{}' declares document_id '{}', expected parent document id '{}'",
                    chunk.id, chunk.document_id, item.document.id
                )));
            }
        }
    }
    Ok(())
}

fn validate_bundle_embeddings(bundle: &RecoveryBundle) -> Result<Option<&EmbeddingManifest>> {
    let bundle_chunk_count = bundle
        .documents
        .iter()
        .map(|item| item.chunks.len())
        .sum::<usize>();
    if let Some(manifest) = bundle.embedding_manifest.as_ref() {
        validate_embedding_manifest(manifest, "recovery bundle")?;
    }
    if bundle_chunk_count == 0 {
        return Ok(None);
    }
    let manifest = bundle.embedding_manifest.as_ref().ok_or_else(|| {
        AppError::embeddings(
            "recovery bundle contains chunk embeddings but has no embedding_manifest",
        )
    })?;
    let expected_dims = usize::try_from(manifest.dims)
        .ok()
        .filter(|dims| *dims > 0)
        .ok_or_else(|| {
            AppError::embeddings(format!(
                "recovery bundle embedding manifest has invalid dims={}",
                manifest.dims
            ))
        })?;
    if let Some(chunk) = bundle
        .documents
        .iter()
        .flat_map(|item| &item.chunks)
        .find(|chunk| chunk.embedding.len() != expected_dims)
    {
        return Err(AppError::embeddings(format!(
            "recovery bundle chunk {} has dims={}, expected {} from embedding_manifest",
            chunk.id,
            chunk.embedding.len(),
            expected_dims
        )));
    }
    Ok(Some(manifest))
}

/// Atomically publish one recovery artifact after fully staging and syncing it.
///
/// With `overwrite=false`, a destination that appears after preflight is left
/// untouched and reported as a conflict.
pub fn publish_recovery_artifact(
    destination: &Path,
    bytes: &[u8],
    overwrite: bool,
) -> Result<bool> {
    publish_recovery_artifact_with_hook(destination, bytes, overwrite, |_| Ok(()))
}

fn publish_recovery_artifact_with_hook<F>(
    destination: &Path,
    bytes: &[u8],
    overwrite: bool,
    before_publish: F,
) -> Result<bool>
where
    F: FnOnce(&Path) -> Result<()>,
{
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(AppError::config(format!(
            "recovery artifact parent directory '{}' does not exist",
            parent.display()
        )));
    }
    let existed = match fs::symlink_metadata(destination) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    if existed && !overwrite {
        return Err(AppError::conflict(format!(
            "recovery artifact destination '{}' already exists; set overwrite=true explicitly",
            destination.display()
        )));
    }

    let temporary = stage_bytes(destination, bytes)?;
    let result = (|| {
        before_publish(destination)?;
        publish_temporary_artifact(&temporary, destination, overwrite)?;
        if !overwrite {
            fs::remove_file(&temporary)?;
        }
        sync_parent_directory(destination)?;
        Ok(existed)
    })();
    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn verify_backup(path: &Path) -> Result<BackupVerification> {
    if !path.is_file() {
        return Err(AppError::not_found(format!(
            "backup file '{}'",
            path.display()
        )));
    }
    let bytes = fs::metadata(path)?.len();
    let sha256 = sha256_file(path)?;
    let verification = (|| -> Result<BackupVerification> {
        // `Store::open` runs migrations. Perform those checks on a disposable
        // byte-for-byte copy so verification can never upgrade or otherwise
        // mutate the artifact it was asked to inspect.
        let (temporary_path, copied_bytes) = stage_file_copy(path, path)?;
        let temporary = TemporaryArtifact::new(temporary_path);
        if copied_bytes != bytes || sha256_file(temporary.path())? != sha256 {
            return Err(AppError::db(format!(
                "backup '{}' changed while its verification copy was being made",
                path.display()
            )));
        }
        let store = Store::open(temporary.path())?;
        let schema_version = store.schema_version()?.unwrap_or(0);
        let (documents, chunks, nodes, edges) = store.stats()?;
        let (_, orphan_chunks, orphan_document_nodes, orphan_edges, _) =
            store.integrity_counts()?;
        let embedding_manifest = store.get_embedding_manifest()?;
        let embedding_contract_ok =
            verify_embedding_contract(&store, chunks, embedding_manifest.as_ref())?;
        drop(store);
        let ok = orphan_chunks == 0
            && orphan_document_nodes == 0
            && orphan_edges == 0
            && embedding_contract_ok;
        Ok(BackupVerification {
            ok,
            path: path.display().to_string(),
            bytes,
            sha256: sha256.clone(),
            schema_version,
            documents,
            chunks,
            nodes,
            edges,
            orphan_chunks,
            orphan_document_nodes,
            orphan_edges,
            embedding_contract_ok,
            embedding_manifest,
        })
    })();

    let bytes_after = fs::metadata(path)?.len();
    let sha256_after = sha256_file(path)?;
    if bytes_after != bytes || sha256_after != sha256 {
        return Err(AppError::conflict(format!(
            "backup '{}' changed during verification",
            path.display()
        )));
    }
    verification
}

fn verify_embedding_contract(
    store: &Store,
    chunk_count: u64,
    manifest: Option<&EmbeddingManifest>,
) -> Result<bool> {
    let Some(manifest) = manifest else {
        return Ok(chunk_count == 0);
    };
    if validate_embedding_manifest(manifest, "backup").is_err() {
        return Ok(false);
    }
    if chunk_count == 0 {
        return Ok(true);
    }
    let expected_dims = match usize::try_from(manifest.dims) {
        Ok(dims) if dims > 0 => dims,
        _ => return Ok(false),
    };
    let conn = store.lock()?;
    let mut statement = conn.prepare("SELECT embedding_json FROM chunks ORDER BY id")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let raw: String = row.get(0)?;
        let embedding = match serde_json::from_str::<Vec<f32>>(&raw) {
            Ok(embedding) => embedding,
            Err(_) => return Ok(false),
        };
        if embedding.len() != expected_dims {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn backup_inventory(dir: &Path) -> Result<Vec<BackupInventoryItem>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut rows = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("duckdb") {
            continue;
        }
        let metadata = entry.metadata()?;
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let modified_at: DateTime<Utc> = modified.into();
        let protected = path
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|n| n.contains("final"));
        rows.push(BackupInventoryItem {
            path: path.display().to_string(),
            bytes: metadata.len(),
            modified_at,
            protected,
            newest: false,
        });
    }
    rows.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| b.path.cmp(&a.path))
    });
    if let Some(first) = rows.first_mut() {
        first.newest = true;
        first.protected = true;
    }
    Ok(rows)
}

pub fn retention_preview(dir: &Path, keep: usize) -> Result<Vec<BackupInventoryItem>> {
    Ok(backup_inventory(dir)?
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| (index >= keep.max(1) && !item.protected).then_some(item))
        .collect())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        context.update(&buffer[..read]);
    }
    Ok(context
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn backup_publication_lock(path: &Path) -> Arc<StdMutex<()>> {
    static LOCKS: OnceLock<StdMutex<HashMap<PathBuf, Weak<StdMutex<()>>>>> = OnceLock::new();

    let key = normalized_destination_path(path);
    let mut locks = LOCKS
        .get_or_init(|| StdMutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(StdMutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

fn normalized_destination_path(path: &Path) -> PathBuf {
    if let Ok(path) = path.canonicalize() {
        return path;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    match (parent.canonicalize(), path.file_name()) {
        (Ok(parent), Some(file_name)) => parent.join(file_name),
        _ => path.to_path_buf(),
    }
}

fn backup_verifications_match(
    left: &BackupVerification,
    right: &BackupVerification,
) -> Result<bool> {
    Ok(serde_json::to_value(left)? == serde_json::to_value(right)?)
}

fn verify_published_backup_group(path: &Path, source: &Path) -> Result<BackupVerification> {
    let verification = verify_backup(path)?;
    let artifacts = backup_artifact_paths(path);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("backup.duckdb");
    let expected_checksum = format!("{}  {}\n", verification.sha256, file_name);
    let checksum = fs::read(&artifacts[SHA256_ARTIFACT])?;
    if checksum != expected_checksum.as_bytes() {
        return Err(AppError::db(format!(
            "published checksum sidecar '{}' does not describe database generation '{}'",
            artifacts[SHA256_ARTIFACT].display(),
            path.display()
        )));
    }

    let metadata: BackupSidecar = serde_json::from_slice(&fs::read(&artifacts[METADATA_ARTIFACT])?)
        .map_err(|error| {
            AppError::db(format!(
                "invalid backup metadata sidecar '{}': {error}",
                artifacts[METADATA_ARTIFACT].display()
            ))
        })?;
    if metadata.format != "rag-duckdb-backup"
        || metadata.source != source.display().to_string()
        || metadata.required_free_bytes != verification.bytes.saturating_mul(2)
        || !backup_verifications_match(&metadata.verification, &verification)?
    {
        return Err(AppError::db(format!(
            "published metadata sidecar '{}' does not describe database generation '{}'",
            artifacts[METADATA_ARTIFACT].display(),
            path.display()
        )));
    }
    Ok(verification)
}

struct BackupSidecarBytes {
    sha256: Vec<u8>,
    metadata: Vec<u8>,
}

fn backup_sidecar_bytes(
    path: &Path,
    source: &Path,
    verification: &BackupVerification,
) -> Result<BackupSidecarBytes> {
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("backup.duckdb");
    let sidecar = BackupSidecar {
        format: "rag-duckdb-backup".into(),
        created_at: Utc::now(),
        source: source.display().to_string(),
        required_free_bytes: verification.bytes.saturating_mul(2),
        verification: verification.clone(),
    };
    Ok(BackupSidecarBytes {
        sha256: format!("{}  {}\n", verification.sha256, name).into_bytes(),
        metadata: serde_json::to_vec_pretty(&sidecar)?,
    })
}

/// Owns the staged and published members of one three-file backup group.
///
/// A normal Rust error unwinds through `Drop`, which rolls back artifacts
/// published by this attempt and restores overwrite targets from hard-link
/// snapshots (subject, unavoidably, to the filesystem accepting cleanup I/O).
/// Filesystems do not provide a transaction spanning three names, though: a
/// process/power loss between renames can still leave sidecars without a new
/// database, hidden staging/rollback links, or (when overwriting) a mixed
/// generation. Publishing the database last makes a *new* database name the
/// commit marker; callers needing the strongest crash semantics should use a
/// fresh destination (`overwrite=false`) and ignore orphan sidecars.
struct BackupGroupPublication {
    artifacts: [PathBuf; 3],
    staged: [Option<PathBuf>; 3],
    overwritten: [Option<PathBuf>; 3],
    published: [bool; 3],
    overwrite: bool,
    committed: bool,
}

impl BackupGroupPublication {
    fn new(artifacts: [PathBuf; 3], overwrite: bool) -> Self {
        Self {
            artifacts,
            staged: std::array::from_fn(|_| None),
            overwritten: std::array::from_fn(|_| None),
            published: [false; 3],
            overwrite,
            committed: false,
        }
    }

    fn set_staged(&mut self, index: usize, path: PathBuf) {
        self.staged[index] = Some(path);
    }

    fn staged_path(&self, index: usize) -> Result<&Path> {
        self.staged[index]
            .as_deref()
            .ok_or_else(|| AppError::db("backup artifact was not staged"))
    }

    fn preserve_overwritten_artifacts(&mut self) -> Result<()> {
        if !self.overwrite {
            return Ok(());
        }
        for index in 0..self.artifacts.len() {
            match fs::symlink_metadata(&self.artifacts[index]) {
                Ok(_) => {
                    self.overwritten[index] = Some(create_rollback_link(&self.artifacts[index])?);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn publish(&mut self, index: usize) -> Result<()> {
        let staged = self.staged_path(index)?.to_path_buf();
        publish_temporary_artifact(&staged, &self.artifacts[index], self.overwrite)?;
        self.published[index] = true;
        if !self.overwrite {
            // `hard_link` publishes the final name without consuming staging.
            // Mark ownership first so an unlink error rolls back both names.
            fs::remove_file(&staged)?;
        }
        self.staged[index] = None;
        Ok(())
    }

    fn commit(mut self) {
        self.committed = true;
        for path in self.overwritten.iter_mut().filter_map(Option::take) {
            let _ = fs::remove_file(path);
        }
        let _ = sync_parent_directory(&self.artifacts[DATABASE_ARTIFACT]);
    }

    fn rollback(&mut self) {
        for index in (0..self.artifacts.len()).rev() {
            if self.published[index] {
                let _ = fs::remove_file(&self.artifacts[index]);
                self.published[index] = false;
                if let Some(original) = self.overwritten[index].take() {
                    if fs::rename(&original, &self.artifacts[index]).is_err() {
                        // Keep the hard-link snapshot for manual recovery rather
                        // than deleting the last known copy of the old artifact.
                        self.overwritten[index] = Some(original);
                    }
                }
            } else if let Some(original) = self.overwritten[index].take() {
                let _ = fs::remove_file(original);
            }
        }
        for staged in self.staged.iter_mut().filter_map(Option::take) {
            let _ = fs::remove_file(staged);
        }
        let _ = sync_parent_directory(&self.artifacts[DATABASE_ARTIFACT]);
    }
}

impl Drop for BackupGroupPublication {
    fn drop(&mut self) {
        if !self.committed {
            self.rollback();
        }
    }
}

struct TemporaryArtifact {
    path: PathBuf,
}

impl TemporaryArtifact {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryArtifact {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let mut wal_name: OsString = self.path.as_os_str().to_owned();
        wal_name.push(".wal");
        let _ = fs::remove_file(PathBuf::from(wal_name));
    }
}

fn preflight_backup_target(
    source: &Path,
    destination: &Path,
    overwrite: bool,
    label: &str,
) -> Result<bool> {
    refuse_same_file(source, destination)?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(AppError::config(format!(
            "{label} parent directory '{}' does not exist",
            parent.display()
        )));
    }
    let metadata = match fs::symlink_metadata(destination) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if metadata
        .as_ref()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(AppError::forbidden(format!(
            "{label} destination '{}' must not be a symbolic link",
            destination.display()
        )));
    }
    let exists = metadata.is_some();
    if exists && !overwrite {
        return Err(AppError::conflict(format!(
            "{label} destination '{}' already exists; set overwrite=true explicitly",
            destination.display()
        )));
    }
    Ok(exists)
}

fn stage_file_copy(source: &Path, destination: &Path) -> Result<(PathBuf, u64)> {
    let (temporary_path, mut temporary) = create_temporary_artifact(destination)?;
    let result = (|| -> Result<(PathBuf, u64)> {
        let mut input = fs::File::open(source)?;
        let bytes = std::io::copy(&mut input, &mut temporary)?;
        if let Ok(metadata) = fs::metadata(source) {
            fs::set_permissions(&temporary_path, metadata.permissions())?;
        }
        temporary.sync_all()?;
        drop(temporary);
        Ok((temporary_path.clone(), bytes))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn quote_duckdb_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn quote_duckdb_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Create a transactionally consistent DuckDB snapshot without retaining the
/// process-wide Store mutex during the potentially long copy.
fn stage_database_snapshot<F>(
    snapshot_conn: duckdb::Connection,
    source: &Path,
    destination: &Path,
    mut copy_hook: F,
) -> Result<(PathBuf, u64)>
where
    F: FnMut() -> Result<()>,
{
    static NEXT_CATALOG_ID: AtomicU64 = AtomicU64::new(0);

    let temporary_path = create_temporary_database_artifact(destination)?;
    let result = (|| -> Result<(PathBuf, u64)> {
        let temporary_utf8 = temporary_path.to_str().ok_or_else(|| {
            AppError::config(format!(
                "DuckDB snapshot staging path '{}' must be valid UTF-8",
                temporary_path.display()
            ))
        })?;
        let source_catalog: String =
            snapshot_conn.query_row("SELECT current_database()", [], |row| row.get(0))?;
        let catalog = format!(
            "rag_backup_snapshot_{}_{}",
            std::process::id(),
            NEXT_CATALOG_ID.fetch_add(1, Ordering::Relaxed)
        );
        let attach = format!(
            "ATTACH {} AS {}",
            quote_duckdb_string(temporary_utf8),
            quote_duckdb_identifier(&catalog)
        );
        snapshot_conn.execute_batch(&attach)?;
        snapshot_conn.execute_batch("BEGIN TRANSACTION")?;
        let source_catalog = quote_duckdb_identifier(&source_catalog);
        let catalog = quote_duckdb_identifier(&catalog);
        let copy = (|| -> Result<()> {
            // Pin the source snapshot before releasing the hook. COPY SCHEMA
            // and COPY DATA then observe one MVCC-consistent generation.
            snapshot_conn.query_row(
                &format!("SELECT COUNT(*) FROM {source_catalog}.main.schema_version"),
                [],
                |_row| Ok(()),
            )?;
            copy_hook()?;
            snapshot_conn
                .execute_batch(&format!("COPY FROM DATABASE {source_catalog} TO {catalog}"))?;
            Ok(())
        })();
        let transaction_end =
            snapshot_conn.execute_batch(if copy.is_ok() { "COMMIT" } else { "ROLLBACK" });
        let detach = snapshot_conn.execute_batch(&format!("DETACH {catalog}"));
        copy?;
        transaction_end?;
        detach?;
        sync_file(&temporary_path)?;
        if let Ok(metadata) = fs::metadata(source) {
            fs::set_permissions(&temporary_path, metadata.permissions())?;
        }
        let bytes = fs::metadata(&temporary_path)?.len();
        Ok((temporary_path.clone(), bytes))
    })();
    drop(snapshot_conn);
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
        let mut wal = temporary_path.as_os_str().to_os_string();
        wal.push(".wal");
        let _ = fs::remove_file(PathBuf::from(wal));
    }
    result
}

fn stage_bytes(destination: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let (temporary_path, mut temporary) = create_temporary_artifact(destination)?;
    let result = (|| -> Result<PathBuf> {
        temporary.write_all(bytes)?;
        temporary.sync_all()?;
        drop(temporary);
        Ok(temporary_path.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn sync_file(path: &Path) -> Result<()> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()?;
    Ok(())
}

fn create_rollback_link(destination: &Path) -> Result<PathBuf> {
    static NEXT_ROLLBACK_ID: AtomicU64 = AtomicU64::new(0);

    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or_else(|| AppError::config("backup destination must name a file"))?;
    for _ in 0..100 {
        let id = NEXT_ROLLBACK_ID.fetch_add(1, Ordering::Relaxed);
        let mut rollback_name = file_name.to_os_string();
        rollback_name.push(format!(".rag-backup-rollback-{}-{id}", std::process::id()));
        let rollback_path = parent.join(rollback_name);
        match fs::hard_link(destination, &rollback_path) {
            Ok(()) => return Ok(rollback_path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(AppError::conflict(format!(
        "could not preserve overwritten backup artifact '{}'",
        destination.display()
    )))
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> Result<()> {
    Ok(())
}

fn create_temporary_artifact(destination: &Path) -> Result<(PathBuf, fs::File)> {
    static NEXT_TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or_else(|| AppError::config("backup destination must name a file"))?;
    for _ in 0..100 {
        let id = NEXT_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = file_name.to_os_string();
        temporary_name.push(format!(".rag-backup-tmp-{}-{id}", std::process::id()));
        let temporary_path = parent.join(temporary_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => return Ok((temporary_path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(AppError::conflict(format!(
        "could not allocate a temporary backup artifact beside '{}'",
        destination.display()
    )))
}

fn create_temporary_database_artifact(destination: &Path) -> Result<PathBuf> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or_else(|| AppError::config("backup destination must name a file"))?;
    for _ in 0..100 {
        let mut temporary_name = file_name.to_os_string();
        temporary_name.push(format!(
            ".rag-backup-tmp-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let temporary_path = parent.join(temporary_name);
        let reservation = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        drop(reservation);
        fs::remove_file(&temporary_path)?;
        match duckdb::Connection::open(&temporary_path) {
            Ok(conn) => {
                let initialized = conn.execute_batch("CHECKPOINT");
                drop(conn);
                match initialized {
                    Ok(()) => return Ok(temporary_path),
                    Err(error) => {
                        let _ = fs::remove_file(&temporary_path);
                        return Err(error.into());
                    }
                }
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary_path);
                return Err(error.into());
            }
        }
    }
    Err(AppError::conflict(format!(
        "could not allocate a temporary DuckDB backup artifact beside '{}'",
        destination.display()
    )))
}

fn publish_temporary_artifact(temporary: &Path, destination: &Path, overwrite: bool) -> Result<()> {
    if overwrite {
        fs::rename(temporary, destination)?;
        return Ok(());
    }
    match fs::hard_link(temporary, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(AppError::conflict(format!(
                "backup artifact '{}' appeared while the backup was running",
                destination.display()
            )))
        }
        Err(error) => Err(error.into()),
    }
}

fn refuse_same_file(source: &Path, destination: &Path) -> Result<()> {
    if source == destination {
        return Err(AppError::forbidden(
            "backup destination is the live database",
        ));
    }
    #[cfg(unix)]
    if let (Ok(a), Ok(b)) = (fs::metadata(source), fs::metadata(destination)) {
        if a.dev() == b.dev() && a.ino() == b.ino() {
            return Err(AppError::forbidden(
                "backup destination resolves to the live database inode",
            ));
        }
    }
    Ok(())
}

fn parse_ts(value: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&value)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S%.f").map(|dt| dt.and_utc())
        })
        .unwrap_or_else(|_| Utc::now())
}

#[cfg(test)]
mod tests;
