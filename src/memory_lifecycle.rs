//! Deterministic lifecycle operations for durable memory documents.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::db::Store;
use crate::models::{Document, DocumentFilter, DocumentMetaUpdate, OpsLogEntry};
use crate::error::{AppError, Result};

pub const STATUS_ACTIVE: &str = "active";
pub const STATUS_CONSOLIDATED: &str = "consolidated";
pub const STATUS_ARCHIVED: &str = "archived";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleCandidate {
    pub id: String,
    pub uri: String,
    pub title: String,
    pub layer: String,
    pub kind: String,
    pub status: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleMutationResult {
    pub operation: String,
    pub requested: usize,
    pub changed_ids: Vec<String>,
    pub skipped_ids: Vec<String>,
    pub missing_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_document_id: Option<String>,
}

pub fn list_candidates(store: &Store, status: Option<&str>, layer: Option<&str>, kind: Option<&str>, limit: usize) -> Result<Vec<LifecycleCandidate>> {
    let docs = store.list_documents_filtered(&DocumentFilter {
        status: Some(status.unwrap_or(STATUS_ACTIVE).to_string()),
        layer: layer.map(str::to_string), kind: kind.map(str::to_string),
        include_archived: Some(true), limit: Some(limit.clamp(1, 500)),
        ..DocumentFilter::default()
    })?;
    Ok(docs.into_iter().map(|doc| LifecycleCandidate {
        id: doc.id, uri: doc.uri, title: doc.title, layer: doc.layer,
        kind: doc.kind, status: doc.status, updated_at: doc.updated_at,
    }).collect())
}

pub fn consolidate_selected(store: &Store, document_ids: &[String], output_document_id: &str, agent: Option<&str>) -> Result<LifecycleMutationResult> {
    let output_id = output_document_id.trim();
    if output_id.is_empty() { return Err(AppError::config("output_document_id must be non-empty")); }
    let output = store.get_document(output_id)?.ok_or_else(|| AppError::not_found(format!("output document not found: {output_id}")))?;
    let (sources, missing_ids) = resolve_unique(store, document_ids, Some(output_id))?;
    let source_ids: Vec<String> = sources.iter().map(|d| d.id.clone()).collect();
    let source_uris: Vec<String> = sources.iter().map(|d| d.uri.clone()).collect();

    let mut output_meta = object_metadata(&output)?;
    let original_output_meta = output_meta.clone();
    let lifecycle = object_field(&mut output_meta, "memory_lifecycle");
    merge_string_array(lifecycle, "consolidated_from", &source_ids);
    merge_string_array(lifecycle, "source_uris", &source_uris);
    lifecycle.insert("operation".into(), Value::String("consolidate".into()));
    if output_meta != original_output_meta {
        store.update_document_meta(output_id, &DocumentMetaUpdate {
            metadata_json: Some(Value::Object(output_meta).to_string()), ..DocumentMetaUpdate::default()
        })?;
    }

    let mut changed_ids = Vec::new();
    let mut skipped_ids = Vec::new();
    for source in sources {
        let mut meta = object_metadata(&source)?;
        let lifecycle = object_field(&mut meta, "memory_lifecycle");
        let already_linked = lifecycle.get("consolidated_into").and_then(Value::as_str) == Some(output_id);
        let already_done = source.status == STATUS_CONSOLIDATED && already_linked;
        if already_done { skipped_ids.push(source.id); continue; }
        lifecycle.insert("consolidated_into".into(), Value::String(output_id.to_string()));
        lifecycle.insert("consolidated_at".into(), Value::String(Utc::now().to_rfc3339()));
        store.update_document_meta(&source.id, &DocumentMetaUpdate {
            status: Some(STATUS_CONSOLIDATED.into()), metadata_json: Some(Value::Object(meta).to_string()),
            ..DocumentMetaUpdate::default()
        })?;
        changed_ids.push(source.id);
    }
    let result = LifecycleMutationResult { operation: "consolidate".into(), requested: document_ids.len(), changed_ids, skipped_ids, missing_ids, output_document_id: Some(output_id.into()) };
    log_mutation(store, &result, agent)?;
    Ok(result)
}

pub fn archive_selected(store: &Store, document_ids: &[String], agent: Option<&str>) -> Result<LifecycleMutationResult> {
    let (documents, missing_ids) = resolve_unique(store, document_ids, None)?;
    let mut changed_ids = Vec::new();
    let mut skipped_ids = Vec::new();
    for document in documents {
        if document.status == STATUS_ARCHIVED { skipped_ids.push(document.id); continue; }
        store.update_document_meta(&document.id, &DocumentMetaUpdate { status: Some(STATUS_ARCHIVED.into()), ..DocumentMetaUpdate::default() })?;
        changed_ids.push(document.id);
    }
    let result = LifecycleMutationResult { operation: "archive".into(), requested: document_ids.len(), changed_ids, skipped_ids, missing_ids, output_document_id: None };
    log_mutation(store, &result, agent)?;
    Ok(result)
}

fn resolve_unique(store: &Store, ids: &[String], exclude: Option<&str>) -> Result<(Vec<Document>, Vec<String>)> {
    let mut seen = std::collections::HashSet::new();
    let mut documents = Vec::new();
    let mut missing = Vec::new();
    for raw in ids {
        let id = raw.trim();
        if id.is_empty() || !seen.insert(id.to_string()) || exclude == Some(id) { continue; }
        match store.get_document(id)? { Some(doc) => documents.push(doc), None => missing.push(id.to_string()) }
    }
    Ok((documents, missing))
}

fn object_metadata(doc: &Document) -> Result<serde_json::Map<String, Value>> {
    match serde_json::from_str::<Value>(&doc.metadata_json)? {
        Value::Object(map) => Ok(map),
        _ => Err(AppError::config(format!("document {} metadata_json must be an object", doc.id))),
    }
}

fn object_field<'a>(root: &'a mut serde_json::Map<String, Value>, key: &str) -> &'a mut serde_json::Map<String, Value> {
    if !root.get(key).is_some_and(Value::is_object) { root.insert(key.into(), json!({})); }
    root.get_mut(key).and_then(Value::as_object_mut).expect("object inserted")
}

fn merge_string_array(object: &mut serde_json::Map<String, Value>, key: &str, values: &[String]) {
    let mut merged: Vec<String> = object.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    for value in values { if !merged.contains(value) { merged.push(value.clone()); } }
    object.insert(key.into(), json!(merged));
}

fn log_mutation(store: &Store, result: &LifecycleMutationResult, agent: Option<&str>) -> Result<()> {
    store.append_ops_log(&OpsLogEntry {
        id: Uuid::new_v4().to_string(), seq: 0, ts: Utc::now(), op: format!("memory_lifecycle_{}", result.operation),
        prefix: Some("MEMORY".into()), message: format!("memory lifecycle {} changed {} item(s)", result.operation, result.changed_ids.len()),
        entity_id: result.output_document_id.clone(), entity_kind: Some("memory_lifecycle".into()), payload_json: serde_json::to_string(result)?, agent_name: agent.map(str::to_string),
    })?;
    Ok(())
}
