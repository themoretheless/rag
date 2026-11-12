//! Preserve original markup without polluting readable document bodies or metadata payloads.
use super::*;
use crate::db::lin_engine::{LinReader, RecBuilder, cell_str};
use crate::db::{DocumentWriteHook, HookWriter, register_document_write_hook};
use crate::models::Document;
use std::borrow::Cow;

/// Store hook that moves `code_source_text` into `code_index_source_blobs`.
struct SourceBlobHook;

impl DocumentWriteHook for SourceBlobHook {
    fn name(&self) -> &'static str {
        "code_index.source_blob"
    }

    fn before_document_write<'a>(
        &self,
        writer: &mut dyn HookWriter,
        doc: &'a Document,
    ) -> Result<Cow<'a, Document>> {
        persist(writer, doc)
    }
}

/// Register the source-blob hook with the store (idempotent).
pub fn install_document_write_hook() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        register_document_write_hook(std::sync::Arc::new(SourceBlobHook));
    });
}

pub(crate) fn persist<'a>(
    w: &mut (impl HookWriter + ?Sized),
    doc: &'a Document,
) -> Result<Cow<'a, Document>> {
    if !doc.metadata_json.contains("code_source_text") {
        return Ok(Cow::Borrowed(doc));
    }
    let mut metadata: serde_json::Value = serde_json::from_str(&doc.metadata_json)?;
    let Some(source) = metadata
        .get("code_source_text")
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return Ok(Cow::Borrowed(doc));
    };
    let hash = content_hash(&source);
    if metadata
        .get("code_source_hash")
        .and_then(|v| v.as_str())
        .is_some_and(|h| h != hash)
    {
        return Err(AppError::config("original code source checksum mismatch"));
    }
    // A matching hash identifies the bytes; this also repairs a corrupted blob from verified input.
    w.upsert_record(
        "code_index_source_blobs",
        &hash,
        RecBuilder::new()
            .id(&hash)
            .str("content_hash", &hash)
            .str("content", &source)
            .build(),
    )?;
    let object = metadata
        .as_object_mut()
        .ok_or_else(|| AppError::config("source metadata must be an object"))?;
    object.remove("code_source_text");
    object.insert("code_source_hash".into(), serde_json::json!(hash));
    let mut normalized = doc.clone();
    normalized.metadata_json = serde_json::to_string(&metadata)?;
    Ok(Cow::Owned(normalized))
}
pub fn read<'a>(r: &impl LinReader, content: &'a str, metadata: &str) -> Result<Cow<'a, str>> {
    if !metadata.contains("code_source_hash") {
        return Ok(Cow::Borrowed(content));
    }
    let meta: serde_json::Value = serde_json::from_str(metadata)?;
    let Some(hash) = meta.get("code_source_hash").and_then(|v| v.as_str()) else {
        return Ok(Cow::Borrowed(content));
    };
    let source = r
        .get_by_id("code_index_source_blobs", hash)?
        .and_then(|row| cell_str(&row, "content").map(str::to_string));
    let source = source.ok_or_else(|| {
        AppError::config("original code source blob missing; resync the source file")
    })?;
    if content_hash(&source) != hash {
        return Err(AppError::config(
            "original code source blob corrupt; resync the source file",
        ));
    }
    Ok(Cow::Owned(source))
}
pub fn is_preserved_html(
    r: &impl LinReader,
    path: &std::path::Path,
    metadata: Option<&str>,
) -> bool {
    if !path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("html") || s.eq_ignore_ascii_case("htm"))
    {
        return true;
    }
    metadata.is_some_and(|m| {
        let Ok(meta) = serde_json::from_str::<serde_json::Value>(m) else {
            return false;
        };
        meta.get("code_source_hash")
            .and_then(|v| v.as_str())
            .is_some()
            && read(r, "", m).is_ok()
    })
}
pub fn same_attachment(
    old: Option<&str>,
    new: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    let old = old.and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok());
    [
        "code_source_hash",
        "source_bytes_hash",
        "source_text_hash",
        "source_encoding",
        "source_encoding_basis",
        "source_decoding_version",
        "source_capture_version",
        "source_bytes_hash_algorithm",
        "source_bytes_count",
    ]
    .iter()
    .all(|key| old.as_ref().and_then(|m| m.get(*key)) == new.get(*key))
}
