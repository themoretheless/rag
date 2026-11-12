//! Aggregate `lin::Row` codecs for core `rag-mcp` domain models.

use lin::Row;

use crate::db::lin_engine::{
    cell_bool, cell_f64, cell_f64_opt, cell_i64, cell_nonempty_string, cell_opt_string,
    cell_opt_ts, cell_string, cell_ts, cell_vec_f32,
};
use crate::error::Result;
use crate::models::{
    Chunk, Document, EmbeddingManifest, GraphEdge, GraphNode, KgFact, OpsLogEntry, WikiIndexEntry,
};

pub(crate) fn document(row: &Row) -> Result<Document> {
    document_with_body(row, true)
}

pub(crate) fn document_without_body(row: &Row) -> Result<Document> {
    document_with_body(row, false)
}

fn document_with_body(row: &Row, include_body: bool) -> Result<Document> {
    let metadata_json = cell_nonempty_string(row, "metadata_json").unwrap_or_else(|| "{}".into());
    let layer = cell_nonempty_string(row, "layer")
        .unwrap_or_else(|| "raw".into())
        .into();
    let kind = cell_nonempty_string(row, "kind")
        .unwrap_or_else(|| "document".into())
        .into();
    let status = cell_nonempty_string(row, "status")
        .unwrap_or_else(|| "active".into())
        .into();
    let revision = cell_i64(row, "revision", 1).max(1);
    Ok(Document {
        id: cell_string(row, "id"),
        uri: cell_string(row, "uri"),
        title: cell_string(row, "title"),
        content: if include_body {
            cell_string(row, "content")
        } else {
            String::new()
        },
        metadata_json,
        content_hash: cell_opt_string(row, "content_hash"),
        wing: cell_opt_string(row, "wing"),
        room: cell_opt_string(row, "room"),
        source_file: cell_opt_string(row, "source_file"),
        layer,
        kind,
        created_at: cell_ts(row, "created_at")?,
        updated_at: cell_ts(row, "updated_at")?,
        status,
        pinned: cell_bool(row, "pinned", false),
        boost: cell_f64(row, "boost", 1.0),
        revision,
    })
}

pub(crate) fn document_revision(row: &Row) -> Result<Document> {
    let id = cell_nonempty_string(row, "document_id").unwrap_or_else(|| cell_string(row, "id"));
    let mut doc = document(row)?;
    doc.id = id;
    Ok(doc)
}

pub(crate) fn chunk(row: &Row) -> Result<Chunk> {
    let mut embedding = cell_vec_f32(row, "embedding");
    if embedding.is_empty()
        && let Some(json) = cell_nonempty_string(row, "embedding_json")
        && let Ok(parsed) = serde_json::from_str::<Vec<f32>>(&json)
    {
        embedding = parsed;
    }
    Ok(Chunk {
        id: cell_string(row, "id"),
        document_id: cell_string(row, "document_id"),
        chunk_index: cell_i64(row, "chunk_index", 0) as i32,
        content: cell_string(row, "content"),
        embedding,
        char_start: cell_i64(row, "char_start", 0) as i32,
        char_end: cell_i64(row, "char_end", 0) as i32,
        metadata_json: cell_nonempty_string(row, "metadata_json").unwrap_or_else(|| "{}".into()),
    })
}

pub(crate) fn embedding_manifest(row: &Row) -> Result<EmbeddingManifest> {
    Ok(EmbeddingManifest {
        id: cell_string(row, "id"),
        provider: cell_string(row, "provider"),
        model: cell_string(row, "model"),
        dims: cell_i64(row, "dims", 0) as i32,
        base_url: cell_opt_string(row, "base_url"),
        content_fingerprint: cell_nonempty_string(row, "content_fingerprint"),
        updated_at: cell_ts(row, "updated_at")?,
    })
}

pub(crate) fn graph_node(row: &Row) -> Result<GraphNode> {
    Ok(GraphNode {
        id: cell_string(row, "id"),
        kind: cell_string(row, "kind"),
        label: cell_string(row, "label"),
        document_id: cell_opt_string(row, "document_id"),
        uri: cell_opt_string(row, "uri"),
        resolved: cell_bool(row, "resolved", true),
        metadata_json: cell_nonempty_string(row, "metadata_json").unwrap_or_else(|| "{}".into()),
    })
}

pub(crate) fn graph_edge(row: &Row) -> Result<GraphEdge> {
    Ok(GraphEdge {
        id: cell_string(row, "id"),
        source_id: cell_string(row, "source_id"),
        target_id: cell_string(row, "target_id"),
        rel_type: cell_string(row, "rel_type"),
        weight: cell_f64(row, "weight", 1.0),
        context: cell_opt_string(row, "context"),
    })
}

pub(crate) fn kg_fact(row: &Row) -> Result<KgFact> {
    Ok(KgFact {
        id: cell_string(row, "id"),
        subject: cell_string(row, "subject"),
        predicate: cell_string(row, "predicate"),
        object: cell_string(row, "object"),
        valid_from: cell_opt_ts(row, "valid_from")?,
        valid_to: cell_opt_ts(row, "valid_to")?,
        status: cell_nonempty_string(row, "status").unwrap_or_else(|| "active".into()),
        superseded_by: cell_opt_string(row, "superseded_by"),
        source_document_id: cell_opt_string(row, "source_document_id"),
        source: cell_opt_string(row, "source"),
        confidence: cell_f64_opt(row, "confidence"),
        metadata_json: cell_nonempty_string(row, "metadata_json").unwrap_or_else(|| "{}".into()),
        created_at: cell_ts(row, "created_at")?,
        updated_at: cell_ts(row, "updated_at")?,
        invalidated_at: cell_opt_ts(row, "invalidated_at")?,
    })
}

pub(crate) fn ops_log_entry(row: &Row) -> Result<OpsLogEntry> {
    Ok(OpsLogEntry {
        id: cell_string(row, "id"),
        seq: cell_i64(row, "seq", 0),
        ts: cell_ts(row, "ts")?,
        op: cell_string(row, "op"),
        prefix: cell_opt_string(row, "prefix"),
        message: cell_string(row, "message"),
        entity_id: cell_opt_string(row, "entity_id"),
        entity_kind: cell_opt_string(row, "entity_kind"),
        payload_json: cell_nonempty_string(row, "payload_json").unwrap_or_else(|| "{}".into()),
        agent_name: cell_opt_string(row, "agent_name"),
    })
}

pub(crate) fn wiki_index_entry(row: &Row) -> Result<WikiIndexEntry> {
    let page_id =
        cell_nonempty_string(row, "page_id").or_else(|| cell_nonempty_string(row, "document_id"));
    let title = cell_nonempty_string(row, "title")
        .or_else(|| cell_nonempty_string(row, "label"))
        .unwrap_or_else(|| cell_string(row, "id"));
    let slug = cell_nonempty_string(row, "slug")
        .or_else(|| cell_nonempty_string(row, "label"))
        .unwrap_or_else(|| cell_string(row, "id"));
    Ok(WikiIndexEntry {
        id: cell_string(row, "id"),
        slug,
        title,
        kind: cell_nonempty_string(row, "kind").unwrap_or_else(|| "wiki".into()),
        category: cell_opt_string(row, "category"),
        summary: cell_opt_string(row, "summary"),
        page_id,
        updated_at: cell_ts(row, "updated_at")?,
    })
}
