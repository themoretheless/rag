//! Portable bundle codec: size preflight, JSON/JSONL encode/decode and shape validation.

use super::*;

pub(super) fn portable_recovery_limit_error(reason: impl std::fmt::Display) -> AppError {
    AppError::config(format!(
        "portable recovery bundle {reason}; limit is {} bytes, {} documents, and {} chunks; {PORTABLE_RECOVERY_LIMIT_GUIDANCE}",
        PORTABLE_RECOVERY_MAX_BYTES,
        PORTABLE_RECOVERY_MAX_DOCUMENTS,
        PORTABLE_RECOVERY_MAX_CHUNKS,
    ))
}

pub(super) fn ensure_portable_preflight(
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

pub(super) fn ensure_portable_encoded_bytes(bytes: u64) -> Result<()> {
    if bytes > PORTABLE_RECOVERY_MAX_BYTES {
        return Err(portable_recovery_limit_error(format!(
            "serialized to {bytes} bytes"
        )));
    }
    Ok(())
}

pub(super) fn saturating_len(value: &str) -> u64 {
    u64::try_from(value.len()).unwrap_or(u64::MAX)
}

pub(super) fn add_json_string_estimate(total: &mut u64, value: &str) {
    *total =
        total.saturating_add(saturating_len(value).saturating_mul(JSON_STRING_WORST_CASE_FACTOR));
}

pub(super) fn portable_preflight_from_bundle(
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

pub(super) struct BoundedRecoveryWriter {
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
pub(super) struct JsonlManifestRecord<'a> {
    record_type: &'static str,
    format: &'a str,
    version: u32,
    exported_at: &'a DateTime<Utc>,
    embedding_manifest: &'a Option<EmbeddingManifest>,
}

#[derive(Serialize)]
pub(super) struct JsonlDocumentRecord<'a> {
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

pub(super) fn validate_embedding_manifest_json_shape(value: &serde_json::Value, context: &str) -> Result<()> {
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

pub(super) fn validate_bundle_document_json_shape(
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

pub(super) fn validate_recovery_bundle_json_shape(value: &serde_json::Value) -> Result<()> {
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

pub(super) fn ensure_only_fields(
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

pub(super) fn nonnegative_metric(value: i64, name: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| {
        AppError::db(format!(
            "portable recovery preflight returned negative {name}"
        ))
    })
}

pub(super) fn portable_store_preflight_locked(conn: &duckdb::Connection) -> Result<PortableBundlePreflight> {
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
