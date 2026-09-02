use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, HeaderValue, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use super::{
    error::{api_err, api_ok},
    HttpState,
};
use crate::db::DocumentCatalogFilter;
use crate::error::AppError;
use crate::retrieval::{self, SearchCommand, SimilarDocumentsQuery};
use crate::revisions::{RestoreRevisionCommand, RevisionService};
use crate::util::check_path_allowlist;

pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/search", post(search_http))
        .route("/v1/multi-get", post(multi_get))
        .route("/v1/expand-chunks", get(expand_chunks))
        .route("/v1/find-similar", get(find_similar))
        .route("/v1/documents", get(documents))
        .route("/v1/source-file", get(source_file))
        .route("/v1/revisions", get(revisions))
        .route("/v1/revisions/snapshot", get(revision_snapshot))
        .route("/v1/revisions/diff", get(revision_diff))
        .route("/v1/revisions/restore", post(restore_revision))
}

const MAX_SOURCE_FILE_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Deserialize)]
struct SourceFileQuery {
    document_id: String,
}

async fn source_file(
    State(state): State<HttpState>,
    Query(query): Query<SourceFileQuery>,
) -> axum::response::Response {
    let document = match state.store.get_document(&query.document_id) {
        Ok(Some(document)) => document,
        Ok(None) => return api_err(AppError::not_found("document not found")),
        Err(error) => return api_err(error),
    };
    let Some(source_file) = document.source_file.as_deref() else {
        return api_err(AppError::not_found("document has no original source file"));
    };
    let path = std::path::Path::new(source_file);
    if let Err(error) = check_path_allowlist(path, &state.config.ingest_roots) {
        return api_err(AppError::forbidden(error.to_string()));
    }
    let metadata = match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return api_err(AppError::not_found("source path is not a regular file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return api_err(AppError::not_found("original source file no longer exists"));
        }
        Err(error) => return api_err(error.into()),
    };
    if metadata.len() > MAX_SOURCE_FILE_BYTES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({"ok": false, "code": "SOURCE_FILE_TOO_LARGE", "max_bytes": MAX_SOURCE_FILE_BYTES})),
        )
            .into_response();
    }
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(error) => return api_err(error.into()),
    };
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("source-file");
    let safe_filename: String = filename
        .chars()
        .map(|ch| {
            if ch == '\r' || ch == '\n' || ch == '"' {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let mut response = Body::from(bytes).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(source_content_type(path)),
    );
    if let Ok(value) = HeaderValue::from_str(&format!("attachment; filename=\"{safe_filename}\"")) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, value);
    }
    super::record_rag_action(source_download_audit_action());
    response
}

fn source_download_audit_action() -> &'static str {
    // The activity feed is shared across clients, so resource identifiers stay private.
    "download_source_file"
}

fn source_content_type(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("pdf") => "application/pdf",
        Some("json") => "application/json",
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("md" | "txt" | "rs" | "py" | "js" | "ts" | "css" | "csv") => {
            "text/plain; charset=utf-8"
        }
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("xlsx") => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
}

#[derive(Deserialize)]
struct RevisionsQuery {
    document_id: String,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    cursor: Option<String>,
}

async fn revisions(
    State(state): State<HttpState>,
    Query(query): Query<RevisionsQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = match decode_cursor(query.cursor.as_deref()) {
        Ok(value) => value,
        Err(error) => return api_err(error),
    };
    match state
        .store
        .list_document_revision_summaries(&query.document_id, limit, offset)
    {
        Ok(page) => {
            let next_cursor = (offset + page.items.len() < page.total as usize)
                .then(|| encode_cursor(offset + page.items.len()));
            api_ok(json!({
                "ok":true,
                "items":page.items,
                "page":{"limit":limit,"next_cursor":next_cursor,"total":page.total}
            }))
        }
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct RevisionSnapshotQuery {
    document_id: String,
    revision: i64,
}

async fn revision_snapshot(
    State(state): State<HttpState>,
    Query(query): Query<RevisionSnapshotQuery>,
) -> impl IntoResponse {
    match RevisionService::new(&state.store, &state.embedder, &state.config)
        .snapshot_at(&query.document_id, query.revision)
    {
        Ok(snapshot) => api_ok(json!({"ok":true, "result":snapshot})),
        Err(error) => api_err(error),
    }
}

async fn restore_revision(
    State(state): State<HttpState>,
    Json(command): Json<RestoreRevisionCommand>,
) -> impl IntoResponse {
    match RevisionService::new(&state.store, &state.embedder, &state.config)
        .restore(command)
        .await
    {
        Ok(result) => api_ok(json!({"ok": true, "result": result})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct RevisionDiffQuery {
    document_id: String,
    from_revision: i64,
    #[serde(default)]
    to_revision: Option<i64>,
}

async fn revision_diff(
    State(state): State<HttpState>,
    Query(query): Query<RevisionDiffQuery>,
) -> impl IntoResponse {
    match RevisionService::new(&state.store, &state.embedder, &state.config).diff(
        &query.document_id,
        query.from_revision,
        query.to_revision,
    ) {
        Ok(result) => api_ok(json!({"ok": true, "result": result})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct SearchBody {
    query: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    top_k: Option<usize>,
    #[serde(default)]
    wing: Option<String>,
    #[serde(default)]
    room: Option<String>,
    #[serde(default)]
    layer: Option<String>,
    #[serde(default)]
    source_file: Option<String>,
    #[serde(default)]
    include_archived: bool,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

async fn search_http(
    State(state): State<HttpState>,
    Json(body): Json<SearchBody>,
) -> impl IntoResponse {
    let command = SearchCommand {
        query: body.query,
        mode: body.mode,
        default_mode: state.config.default_search_mode,
        top_k: body.top_k,
        default_top_k: state.config.default_top_k,
        document_id: None,
        wing: clean(body.wing),
        room: clean(body.room),
        layer: clean(body.layer),
        source_file: clean(body.source_file),
        include_archived: body.include_archived,
        min_score: None,
        diversity: None,
        group_by: None,
        recency_half_life_days: None,
        max_context_tokens: None,
        max_chunks_per_document: None,
        context_expansion: None,
        neighbor_chunks: None,
        timeout_ms: body.timeout_ms.or(Some(5_000)),
        fts_stemmer: state.config.fts_stemmer.clone(),
    };
    match retrieval::execute_search(&state.store, state.embedder.as_ref(), command).await {
        Ok(hits) => api_ok(json!({"ok": true, "count": hits.len(), "items": hits})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct MultiGetBody {
    document_ids: Vec<String>,
    #[serde(default)]
    include_chunks: bool,
}

async fn multi_get(
    State(state): State<HttpState>,
    Json(body): Json<MultiGetBody>,
) -> impl IntoResponse {
    match retrieval::multi_get(&state.store, body.document_ids, body.include_chunks) {
        Ok(result) => api_ok(json!({
            "ok": true,
            "items": result.documents,
            "missing": result.missing,
        })),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct ExpandQuery {
    document_id: String,
    chunk_index: i32,
    #[serde(default)]
    radius: Option<u32>,
}

async fn expand_chunks(
    State(state): State<HttpState>,
    Query(query): Query<ExpandQuery>,
) -> impl IntoResponse {
    match retrieval::expand_chunks(
        &state.store,
        &query.document_id,
        query.chunk_index,
        query.radius.unwrap_or(1),
    ) {
        Ok(items) => api_ok(json!({"ok": true, "items": items})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct SimilarQuery {
    document_id: String,
    #[serde(default)]
    top_k: Option<usize>,
}

async fn find_similar(
    State(state): State<HttpState>,
    Query(query): Query<SimilarQuery>,
) -> impl IntoResponse {
    let query = SimilarDocumentsQuery {
        document_id: query.document_id,
        top_k: query.top_k.unwrap_or(state.config.default_top_k),
        wing: None,
        room: None,
        fts_stemmer: state.config.fts_stemmer.clone(),
    };
    match retrieval::find_similar(&state.store, query) {
        Ok(items) => api_ok(json!({"ok": true, "items": items})),
        Err(error) => api_err(error),
    }
}

#[derive(Deserialize)]
struct DocumentsQuery {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    wing: Option<String>,
    #[serde(default)]
    room: Option<String>,
    #[serde(default)]
    layer: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    include_archived: bool,
}

async fn documents(
    State(state): State<HttpState>,
    Query(query): Query<DocumentsQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = match decode_cursor(query.cursor.as_deref()) {
        Ok(value) => value,
        Err(error) => return api_err(error),
    };
    let filter = DocumentCatalogFilter {
        q: clean(query.q),
        wing: clean(query.wing),
        room: clean(query.room),
        layer: clean(query.layer),
        kind: clean(query.kind),
        status: clean(query.status),
        include_archived: query.include_archived,
        limit,
        offset,
    };
    match state.store.list_document_catalog(&filter) {
        Ok(page) => {
            let next_cursor = (offset + page.items.len() < page.total as usize)
                .then(|| encode_cursor(offset + page.items.len()));
            api_ok(json!({
                "ok": true,
                "items": page.items,
                "page": {"limit": limit, "next_cursor": next_cursor, "total": page.total},
            }))
        }
        Err(error) => api_err(error),
    }
}

pub(super) fn encode_cursor(offset: usize) -> String {
    format!("v1:{offset}")
}

pub(super) fn decode_cursor(cursor: Option<&str>) -> Result<usize, AppError> {
    match cursor {
        None | Some("") => Ok(0),
        Some(value) => value
            .strip_prefix("v1:")
            .and_then(|offset| offset.parse().ok())
            .ok_or_else(|| AppError::config("invalid cursor")),
    }
}

fn clean(value: Option<String>) -> Option<String> {
    value.filter(|item| !item.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_download_activity_does_not_expose_resource_identifiers() {
        let action = source_download_audit_action();
        assert_eq!(action, "download_source_file");
        assert!(!action.contains("document_id"));
        assert!(!action.contains("secret-file.md"));
    }
}
