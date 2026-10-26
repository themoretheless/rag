//! Preserve original markup without polluting readable document bodies or metadata payloads.
use super::*;
use crate::models::Document;
use duckdb::Connection;
use std::borrow::Cow;

pub(crate) fn persist<'a>(conn: &Connection, doc: &'a Document) -> Result<Cow<'a, Document>> {
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
    conn.execute("INSERT INTO code_index_source_blobs VALUES (?,?) ON CONFLICT(content_hash) DO UPDATE SET content=excluded.content",params![hash,source])?;
    let object = metadata
        .as_object_mut()
        .ok_or_else(|| AppError::config("source metadata must be an object"))?;
    object.remove("code_source_text");
    object.insert("code_source_hash".into(), serde_json::json!(hash));
    let mut normalized = doc.clone();
    normalized.metadata_json = serde_json::to_string(&metadata)?;
    Ok(Cow::Owned(normalized))
}
pub(crate) fn read<'a>(
    conn: &Connection,
    content: &'a str,
    metadata: &str,
) -> Result<Cow<'a, str>> {
    if !metadata.contains("code_source_hash") {
        return Ok(Cow::Borrowed(content));
    }
    let meta: serde_json::Value = serde_json::from_str(metadata)?;
    let Some(hash) = meta.get("code_source_hash").and_then(|v| v.as_str()) else {
        return Ok(Cow::Borrowed(content));
    };
    let source: Option<String> = conn
        .query_row(
            "SELECT content FROM code_index_source_blobs WHERE content_hash=?",
            [hash],
            |r| r.get(0),
        )
        .optional()?;
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
pub(crate) fn is_preserved_html(
    conn: &Connection,
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
            && read(conn, "", m).is_ok()
    })
}
pub(crate) fn same_attachment(
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
