//! Schema document (`schema://agents`) seeding and updates.

use super::*;

/// Ensure `schema://agents` exists; seed with [`DEFAULT_SCHEMA`] when missing.
pub fn ensure_default_schema(store: &Store) -> Result<Document> {
    if let Some(d) = store.get_schema_document()? {
        return Ok(d);
    }
    let now = Utc::now();
    let doc = Document {
        id: Uuid::new_v4().to_string(),
        uri: SCHEMA_URI.into(),
        title: SCHEMA_TITLE.into(),
        content: DEFAULT_SCHEMA.into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        layer: LAYER_SCHEMA.into(),
        kind: LAYER_SCHEMA.into(),
        content_hash: Some(content_hash(DEFAULT_SCHEMA)),
        ..Default::default()
    };
    // Document, graph node (schema can carry wikilinks) and audit row commit together.
    store.write_unchunked_document_with_audit(
        &doc,
        &OpsLogEntry {
            id: Uuid::new_v4().to_string(),
            seq: 0,
            ts: now,
            op: "schema_init".into(),
            prefix: Some("SCHEMA".into()),
            message: format!("initialized default {SCHEMA_URI}"),
            entity_id: Some(doc.id.clone()),
            entity_kind: Some("schema".into()),
            payload_json: "{}".into(),
            agent_name: None,
        },
    )?;
    Ok(doc)
}

/// Get or create schema content as string.
pub fn schema_text(store: &Store) -> Result<String> {
    Ok(ensure_default_schema(store)?.content)
}

/// MCP `get_schema`: return the wiki/schema document at [`SCHEMA_URI`].
///
/// Seeds [`DEFAULT_SCHEMA`] when no schema document exists yet.
pub fn get_schema(store: &Store) -> Result<SchemaDocumentView> {
    let doc = ensure_default_schema(store)?;
    Ok(SchemaDocumentView::from_doc(&doc, false))
}

/// MCP `update_schema`: replace content of the schema document at [`SCHEMA_URI`].
///
/// Stored as `layer=schema`, `kind=schema` (wiki compile-layer conventions file).
/// Does not embed chunks (agents read the full document via `get_schema`).
/// Re-runs graph extract for `[[wikilinks]]` / `#tags` in the schema text.
pub fn update_schema(
    store: &Store,
    content: &str,
    title: Option<&str>,
    agent: Option<&str>,
) -> Result<SchemaDocumentView> {
    if content.trim().is_empty() {
        return Err(AppError::config("update_schema: content must not be empty"));
    }

    let now = Utc::now();
    let title = title
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(SCHEMA_TITLE)
        .to_string();

    let (document_id, created_at, created) =
        if let Some(existing) = store.find_by_uri(SCHEMA_URI)? {
            assert_content_mutable(&existing)?;
            (existing.id, existing.created_at, false)
        } else if let Some(existing) = store.get_schema_document()? {
            // Fallback layer=schema row with a non-canonical uri: re-key to SCHEMA_URI.
            assert_content_mutable(&existing)?;
            (existing.id, existing.created_at, false)
        } else {
            (Uuid::new_v4().to_string(), now, true)
        };

    let hash = content_hash(content);
    let doc = Document {
        id: document_id.clone(),
        uri: SCHEMA_URI.into(),
        title: title.clone(),
        content: content.to_string(),
        metadata_json: "{}".into(),
        created_at,
        updated_at: now,
        layer: LAYER_SCHEMA.into(),
        kind: LAYER_SCHEMA.into(),
        content_hash: Some(hash.clone()),
        ..Default::default()
    };
    store.write_unchunked_document_with_audit(
        &doc,
        &OpsLogEntry {
            id: Uuid::new_v4().to_string(),
            seq: 0,
            ts: now,
            op: "schema_update".into(),
            prefix: Some("SCHEMA".into()),
            message: format!("updated {SCHEMA_URI}"),
            entity_id: Some(document_id.clone()),
            entity_kind: Some("schema".into()),
            payload_json: serde_json::json!({
                "uri": SCHEMA_URI,
                "title": title,
                "content_hash": hash,
                "created": created,
            })
            .to_string(),
            agent_name: agent.map(|s| s.to_string()),
        },
    )?;

    Ok(SchemaDocumentView::from_doc(&doc, created))
}
