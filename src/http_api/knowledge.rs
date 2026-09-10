//! Typed wiki properties and dynamic saved views over the same document authority.
use super::{
    error::{api_err, api_ok},
    HttpState,
};
use crate::{
    db::{store::DocumentDerivedWrite, Store},
    error::{AppError, Result},
};
use axum::{
    extract::{Query, State},
    response::Response,
    routing::get,
    Json, Router,
};
use duckdb::{params, params_from_iter, OptionalExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
pub(super) fn routes() -> Router<HttpState> {
    Router::new()
        .route("/v1/knowledge", get(list).put(update))
        .route("/v1/knowledge/views", get(views).post(save_view))
}
#[derive(Default, Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct Properties {
    document_type: String,
    review_status: String,
    owner: String,
    review_due: String,
}
impl Properties {
    fn validate(&self) -> Result<()> {
        if !["", "note", "decision", "guide", "service", "research"]
            .contains(&self.document_type.as_str())
            || !["", "draft", "needs_review", "verified"].contains(&self.review_status.as_str())
            || self.owner.len() > 200
        {
            return Err(AppError::config("invalid knowledge properties"));
        }
        date(&self.review_due)
    }
}
fn date(value: &str) -> Result<()> {
    if !value.is_empty()
        && (value.len() != 10 || chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_err())
    {
        return Err(AppError::config("date must be a valid YYYY-MM-DD"));
    }
    Ok(())
}
#[derive(Default, Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct Filter {
    q: String,
    wing: String,
    document_type: String,
    review_status: String,
    owner: String,
    due_before: String,
    offset: u32,
}
impl Filter {
    fn validate(&self) -> Result<()> {
        if self.q.len() > 500 || self.wing.len() > 200 {
            return Err(AppError::config("filter is too long"));
        }
        Properties {
            document_type: self.document_type.clone(),
            review_status: self.review_status.clone(),
            owner: self.owner.clone(),
            review_due: self.due_before.clone(),
        }
        .validate()
    }
}
fn query(store: &Store, f: Filter) -> Result<Value> {
    f.validate()?;
    let mut clauses = vec![
        "layer = 'wiki'".to_string(),
        "COALESCE(status,'active') NOT IN ('archived','tombstone')".into(),
    ];
    let mut values = Vec::new();
    if !f.q.is_empty() {
        clauses.push("contains(lower(title || ' ' || uri), lower(?))".into());
        values.push(f.q);
    }
    if !f.wing.is_empty() {
        clauses.push("wing = ?".into());
        values.push(f.wing);
    }
    for (name, value) in [
        ("document_type", f.document_type),
        ("review_status", f.review_status),
        ("owner", f.owner),
    ] {
        if !value.is_empty() {
            clauses.push(format!("json_extract_string(TRY_CAST(metadata_json AS JSON), '$.knowledge_properties.{name}') = ?"));
            values.push(value);
        }
    }
    if !f.due_before.is_empty() {
        clauses.push("TRY_CAST(json_extract_string(TRY_CAST(metadata_json AS JSON), '$.knowledge_properties.review_due') AS DATE) <= CAST(? AS DATE)".into());
        values.push(f.due_before);
    }
    let condition = clauses.join(" AND ");
    let conn = store.lock()?;
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM documents WHERE {condition}"),
        params_from_iter(values.iter()),
        |r| r.get(0),
    )?;
    let sql=format!("SELECT id,uri,title,wing,revision,metadata_json FROM documents WHERE {condition} ORDER BY title,id LIMIT 50 OFFSET {}",f.offset);
    let mut stmt = conn.prepare(&sql)?;
    let items=stmt.query_map(params_from_iter(values.iter()),|r| {
        let metadata:String=r.get(5)?;
        let metadata:Value=serde_json::from_str(&metadata).unwrap_or(json!({}));
        Ok(json!({"id":r.get::<_,String>(0)?,"uri":r.get::<_,String>(1)?,"title":r.get::<_,String>(2)?,"wing":r.get::<_,Option<String>>(3)?,"revision":r.get::<_,i64>(4)?,"properties":metadata.get("knowledge_properties").cloned().unwrap_or(json!({}))}))
    })?.collect::<std::result::Result<Vec<_>,_>>()?;
    Ok(json!({"items":items,"total":total,"offset":f.offset}))
}
async fn list(State(st): State<HttpState>, Query(f): Query<Filter>) -> Response {
    match super::run_blocking("knowledge view", move || query(&st.store, f)).await {
        Ok(v) => api_ok(v),
        Err(e) => api_err(e),
    }
}
#[derive(Deserialize)]
struct Update {
    id: String,
    revision: i64,
    properties: Properties,
}
fn update_properties(store: &Store, body: Update) -> Result<Value> {
    body.properties.validate()?;
    let mut doc = store
        .get_document(&body.id)?
        .ok_or_else(|| AppError::not_found("wiki page not found"))?;
    if doc.layer != "wiki" {
        return Err(AppError::config("knowledge properties require a wiki page"));
    }
    let mut metadata: Value = serde_json::from_str(&doc.metadata_json)?;
    let object = metadata
        .as_object_mut()
        .ok_or_else(|| AppError::config("metadata must be an object"))?;
    object.insert(
        "knowledge_properties".into(),
        serde_json::to_value(&body.properties)?,
    );
    object.insert(
        "properties_updated_at".into(),
        json!(chrono::Utc::now().to_rfc3339()),
    );
    doc.metadata_json = serde_json::to_string(&metadata)?;
    // Keep the article's content timestamp: metadata edits must not clear legacy staleness.
    let result =
        store.write_document_atomic(&doc, Some(body.revision), DocumentDerivedWrite::Preserve)?;
    Ok(json!({"id":doc.id,"revision":result.revision,"properties":body.properties}))
}
async fn update(State(st): State<HttpState>, Json(body): Json<Update>) -> Response {
    match super::run_blocking("update knowledge properties", move || {
        update_properties(&st.store, body)
    })
    .await
    {
        Ok(v) => api_ok(v),
        Err(e) => api_err(e),
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct View {
    id: String,
    name: String,
    revision: i64,
    filters: Filter,
}
#[derive(Deserialize)]
struct SaveView {
    id: Option<String>,
    name: String,
    revision: Option<i64>,
    filters: Filter,
}
async fn views(State(st): State<HttpState>) -> Response {
    let result = super::run_blocking("saved views", move || {
        let conn = st.store.lock()?;
        let mut stmt = conn.prepare("SELECT payload FROM knowledge_views ORDER BY id LIMIT 500")?;
        let items = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str::<View>(&r?)?))
            .collect::<Result<Vec<_>>>()?;
        Ok(json!({"items":items}))
    })
    .await;
    match result {
        Ok(v) => api_ok(v),
        Err(e) => api_err(e),
    }
}
fn persist_view(store: &Store, mut body: SaveView) -> Result<View> {
    body.filters.validate()?;
    body.filters.offset = 0;
    if body.name.trim().is_empty() || body.name.len() > 200 {
        return Err(AppError::config("view name must contain 1..200 bytes"));
    }
    let conn = store.lock()?;
    let (id, revision) = if let Some(id) = body.id {
        let prior: Option<String> = conn
            .query_row(
                "SELECT payload FROM knowledge_views WHERE id=?",
                [&id],
                |r| r.get(0),
            )
            .optional()?;
        let prior: View =
            serde_json::from_str(&prior.ok_or_else(|| AppError::not_found("view not found"))?)?;
        if Some(prior.revision) != body.revision {
            return Err(AppError::conflict("view changed"));
        }
        (id, prior.revision + 1)
    } else {
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM knowledge_views", [], |r| r.get(0))?;
        if count >= 500 {
            return Err(AppError::config("saved view limit reached"));
        }
        (uuid::Uuid::new_v4().to_string(), 1)
    };
    let view = View {
        id,
        name: body.name.trim().into(),
        revision,
        filters: body.filters,
    };
    conn.execute("INSERT INTO knowledge_views VALUES (?,?) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",params![view.id,serde_json::to_string(&view)?])?;
    Ok(view)
}
async fn save_view(State(st): State<HttpState>, Json(body): Json<SaveView>) -> Response {
    match super::run_blocking("save knowledge view", move || persist_view(&st.store, body)).await {
        Ok(v) => api_ok(v),
        Err(e) => api_err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Document;
    fn fixture() -> (Store, Document) {
        let root = tempfile::tempdir().unwrap().keep();
        let store = Store::open(&root.join("knowledge.duckdb")).unwrap();
        let doc = Document {
            id: "wiki-a".into(),
            uri: "wiki://alpha".into(),
            title: "Alpha".into(),
            layer: "wiki".into(),
            content: "source text".into(),
            metadata_json: json!({"custom":"keep","source_versions":[]}).to_string(),
            ..Default::default()
        };
        store.upsert_document(&doc).unwrap();
        let doc = store.get_document(&doc.id).unwrap().unwrap();
        (store, doc)
    }
    #[test]
    fn properties_keep_content_provenance_and_timestamp_and_enforce_cas() {
        let (store, doc) = fixture();
        let props = Properties {
            document_type: "decision".into(),
            owner: "x' OR 1=1 --".into(),
            review_due: "2026-09-30".into(),
            ..Default::default()
        };
        update_properties(
            &store,
            Update {
                id: doc.id.clone(),
                revision: doc.revision,
                properties: props.clone(),
            },
        )
        .unwrap();
        let after = store.get_document(&doc.id).unwrap().unwrap();
        assert_eq!(after.content, doc.content);
        assert_eq!(after.updated_at, doc.updated_at);
        let meta: Value = serde_json::from_str(&after.metadata_json).unwrap();
        assert_eq!(meta["custom"], "keep");
        assert_eq!(meta["source_versions"], json!([]));
        assert!(matches!(
            update_properties(
                &store,
                Update {
                    id: doc.id.clone(),
                    revision: doc.revision,
                    properties: props.clone()
                }
            ),
            Err(AppError::Conflict(_))
        ));
        assert_eq!(
            query(
                &store,
                Filter {
                    owner: props.owner,
                    ..Default::default()
                }
            )
            .unwrap()["total"],
            1
        );
        assert_eq!(
            query(
                &store,
                Filter {
                    owner: "other".into(),
                    ..Default::default()
                }
            )
            .unwrap()["total"],
            0
        );
        assert!(date("2026-02-30").is_err());
        assert!(Properties {
            review_status: "bogus".into(),
            ..Default::default()
        }
        .validate()
        .is_err());
    }
    #[test]
    fn saved_views_are_dynamic_and_updates_use_revision() {
        let (store, doc) = fixture();
        let v = persist_view(
            &store,
            SaveView {
                id: None,
                revision: None,
                name: "Decisions".into(),
                filters: Filter {
                    document_type: "decision".into(),
                    offset: 50,
                    ..Default::default()
                },
            },
        )
        .unwrap();
        assert_eq!(v.filters.offset, 0);
        assert_eq!(query(&store, v.filters.clone()).unwrap()["total"], 0);
        update_properties(
            &store,
            Update {
                id: doc.id,
                revision: doc.revision,
                properties: Properties {
                    document_type: "decision".into(),
                    ..Default::default()
                },
            },
        )
        .unwrap();
        assert_eq!(query(&store, v.filters.clone()).unwrap()["total"], 1);
        assert!(matches!(
            persist_view(
                &store,
                SaveView {
                    id: Some(v.id),
                    revision: Some(0),
                    name: "Overwrite".into(),
                    filters: v.filters
                }
            ),
            Err(AppError::Conflict(_))
        ));
    }
}
