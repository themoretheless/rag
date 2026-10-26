//! Project-scoped route/reference selection and optimistic captured-view pages.
use super::*;
use blake3::Hasher;
use std::collections::BTreeMap;

#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RouteKind {
    #[default]
    All,
    Registration,
    Endpoint,
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct RouteQuery {
    #[serde(flatten)]
    pub selection: CodeQuery,
    /// Exact project/wing. Also scopes coverage and cache decoding.
    pub project: Option<String>,
    /// Exact captured URL/registration literal. No path or template expansion.
    pub route: Option<String>,
    /// Case-sensitive literal URL substring; %, _ and regex metacharacters are literal.
    pub route_contains: Option<String>,
    #[serde(default)]
    pub kind: RouteKind,
    /// Exact registration method, ASCII case-insensitive. ANY matches unknown only.
    pub method: Option<String>,
    /// Exact captured client method from options or client-name syntax; basis is returned.
    pub request_method: Option<String>,
    /// Exact endpoint category, such as html:form:action, js:fetch or js:jquery:post.
    pub reference_kind: Option<String>,
    /// Matching-record offset. Nonzero requires expected_view_token.
    pub offset: Option<usize>,
    /// Token from the same query's first page. Changed captured views return conflict.
    pub expected_view_token: Option<String>,
}
fn matches(entry: &analysis::EntryPoint, q: &RouteQuery, selected: &HashSet<&str>) -> bool {
    entry.route.is_some()
        && q.selection.path.as_ref().is_none_or(|p| p == &entry.path)
        && (q.selection.symbol.is_none()
            || entry
                .symbol_id
                .as_deref()
                .is_some_and(|id| selected.contains(id)))
        && q.route
            .as_ref()
            .is_none_or(|url| entry.route.as_ref() == Some(url))
        && q.route_contains
            .as_ref()
            .is_none_or(|part| entry.route.as_ref().is_some_and(|url| url.contains(part)))
        && match q.kind {
            RouteKind::All => true,
            RouteKind::Registration => entry.kind == "http_route_candidate",
            RouteKind::Endpoint => entry.kind == "http_endpoint_reference",
        }
        && q.method.as_ref().is_none_or(|m| {
            entry
                .method
                .as_ref()
                .is_some_and(|v| v.eq_ignore_ascii_case(m))
        })
        && q.request_method.as_ref().is_none_or(|m| {
            entry
                .request_method
                .as_ref()
                .is_some_and(|v| v.eq_ignore_ascii_case(m))
        })
        && q.reference_kind
            .as_ref()
            .is_none_or(|kind| entry.reference_kind.as_ref() == Some(kind))
}
fn hash_value(hash: &mut Hasher, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    hash.update(&(bytes.len() as u64).to_le_bytes());
    hash.update(&bytes);
    Ok(())
}
fn render(snapshot: &CodeSnapshot, q: &RouteQuery) -> Result<serde_json::Value> {
    let selected: HashSet<_> = snapshot
        .symbols
        .iter()
        .filter(|symbol| {
            q.selection.path.as_ref().is_none_or(|p| p == &symbol.path)
                && q.selection.symbol.as_ref().is_none_or(|name| {
                    name == &symbol.id || name == &symbol.name || name == &symbol.qualified_name
                })
        })
        .map(|s| s.id.as_str())
        .collect();
    let entries: Vec<_> = snapshot
        .entrypoints
        .iter()
        .filter(|e| matches(e, q, &selected))
        .collect();
    let mut counts = BTreeMap::<&str, usize>::new();
    for entry in &entries {
        *counts.entry(&entry.kind).or_default() += 1;
    }
    let mut hash = Hasher::new();
    hash.update(b"rag-routes-captured-view-v1");
    hash_value(
        &mut hash,
        &serde_json::json!({
            "revision":snapshot.index_revision,"project":q.project,
            "path":q.selection.path,"symbol":q.selection.symbol,"route":q.route,
            "route_contains":q.route_contains,"kind":q.kind,
            "method":q.method.as_ref().map(|m| m.to_ascii_uppercase()),
            "request_method":q.request_method.as_ref().map(|m| m.to_ascii_uppercase()),
            "reference_kind":q.reference_kind,"stale":snapshot.stale_documents,
            "unindexed":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors,
            "binding_diagnostics":snapshot.binding_diagnostics
        }),
    )?;
    for entry in &entries {
        hash_value(&mut hash, entry)?;
    }
    let token = format!("rv1:{}", hash.finalize().to_hex());
    if q.expected_view_token
        .as_ref()
        .is_some_and(|expected| expected != &token)
    {
        return Err(AppError::Conflict(
            "ROUTES_VIEW_CHANGED: restart from offset 0 without expected_view_token".into(),
        ));
    }
    let offset = q.offset.unwrap_or(0);
    let limit = q.selection.limit.unwrap_or(50).clamp(1, 500);
    let page: Vec<_> = entries.iter().skip(offset).take(limit).collect();
    let returned = page.len();
    let has_more = entries.len().saturating_sub(offset) > returned;
    // Reuse the common static-analysis envelope; all selection/page fields are
    // replaced below, before returning. No filtering of an already limited page.
    let mut result = snapshot.query("routes", &q.selection);
    result["items"] = serde_json::to_value(page)?;
    result["total"] = entries.len().into();
    result["truncated"] = (returned < entries.len()).into();
    result["project"] = serde_json::json!(q.project);
    result["query_contract_version"] = 1.into();
    result["observation_consistency"] = "single_read_transaction".into();
    result["view_token"] = token.into();
    result["pagination"] = serde_json::json!({"offset":offset,"limit":limit,"returned":returned,
        "has_more":has_more,"next_offset":if has_more {offset.checked_add(returned)} else {None},
        "token_scope":"query_filters_matching_facts_coverage_and_global_index_revision"});
    result["counts_by_kind"] = serde_json::to_value(counts)?;
    result["filter_semantics"] =
        "exact_project_source_path_and_literal_url_filters_before_pagination".into();
    Ok(result)
}
impl Store {
    pub fn code_routes(&self, q: &RouteQuery) -> Result<serde_json::Value> {
        if q.offset.unwrap_or(0) > 0 && q.expected_view_token.is_none() {
            return Err(AppError::Invalid(
                "expected_view_token is required for nonzero routes offset".into(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let snapshot = snapshot_locked(&transaction, q.project.as_deref())?;
        let result = render(&snapshot, q)?;
        transaction.commit()?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Document;
    fn put(store: &Store, id: &str, project: &str, path: &str, content: &str) {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("doc://{id}"),
                source_file: Some(path.into()),
                wing: Some(project.into()),
                content: content.into(),
                ..Default::default()
            })
            .unwrap();
    }
    fn setup() -> (tempfile::TempDir, Store) {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("routes.duckdb")).unwrap();
        put(&store,"a","alpha","/same.py","@app.get('/same')\ndef same(): pass\n@app.route('/unknown')\ndef unknown(): pass\n@app.post('/literal_%.*')\ndef post(): pass\n");
        put(
            &store,
            "b",
            "beta",
            "/same.py",
            "@app.delete('/foreign')\ndef foreign(): pass\n",
        );
        put(
            &store,
            "j",
            "alpha",
            "/p/client.js",
            "fetch('/same', {method:'POST'}); fetch('/same');",
        );
        put(
            &store,
            "h",
            "alpha",
            "/p/view.html",
            "<form action='/same'></form>",
        );
        store.index_code().unwrap();
        (root, store)
    }
    #[test]
    fn filters_are_literal_typed_and_applied_before_limits() {
        let (_root, store) = setup();
        let mut q = RouteQuery {
            project: Some("alpha".into()),
            route: Some("/same".into()),
            ..Default::default()
        };
        let all = store.code_routes(&q).unwrap();
        assert_eq!(all["total"], 4);
        assert_eq!(all["counts_by_kind"]["http_endpoint_reference"], 3);
        q.kind = RouteKind::Endpoint;
        q.request_method = Some("post".into());
        q.selection.limit = Some(1);
        let post = store.code_routes(&q).unwrap();
        assert_eq!(post["total"], 1, "{post}");
        assert_eq!(post["items"][0]["request_method"], "POST");
        q.request_method = None;
        q.method = Some("get".into());
        assert_eq!(store.code_routes(&q).unwrap()["total"], 0);
        q.kind = RouteKind::Registration;
        assert_eq!(store.code_routes(&q).unwrap()["total"], 1);
        q.method = Some("ANY".into());
        assert_eq!(store.code_routes(&q).unwrap()["total"], 0);
        q.route = None;
        assert_eq!(store.code_routes(&q).unwrap()["total"], 1);
        q.method = None;
        q.route_contains = Some("_%.*".into());
        assert_eq!(store.code_routes(&q).unwrap()["total"], 1);
        q.route_contains = None;
        q.reference_kind = Some("html:form:action".into());
        q.kind = RouteKind::Endpoint;
        assert_eq!(store.code_routes(&q).unwrap()["total"], 1);
    }
    #[test]
    fn pages_require_tokens_and_reject_changed_fresh_subsets_without_new_revision() {
        let (_root, store) = setup();
        let mut q = RouteQuery {
            project: Some("alpha".into()),
            ..Default::default()
        };
        q.selection.limit = Some(1);
        let first = store.code_routes(&q).unwrap();
        assert_eq!(first["total"], 6);
        assert_eq!(first["pagination"]["has_more"], true);
        q.offset = Some(1);
        assert!(matches!(store.code_routes(&q), Err(AppError::Invalid(_))));
        q.expected_view_token = Some(first["view_token"].as_str().unwrap().into());
        q.method = Some("GET".into());
        assert!(matches!(store.code_routes(&q), Err(AppError::Conflict(_))));
        q.method = None;
        let next = store.code_routes(&q).unwrap();
        assert_ne!(first["items"], next["items"]);
        q.selection.limit = Some(3);
        assert_eq!(
            store.code_routes(&q).unwrap()["view_token"],
            first["view_token"]
        );
        put(&store, "j", "alpha", "/p/client.js", "fetch('/changed');");
        assert!(matches!(store.code_routes(&q), Err(AppError::Conflict(_))));
        q.offset = None;
        q.expected_view_token = None;
        let changed = store.code_routes(&q).unwrap();
        assert_eq!(changed["index_rev"], first["index_rev"]);
        assert_ne!(changed["view_token"], first["view_token"]);
        assert_eq!(changed["stale_documents"].as_array().unwrap().len(), 1);
        q.expected_view_token = Some(changed["view_token"].as_str().unwrap().into());
        q.offset = Some(usize::MAX);
        let empty = store.code_routes(&q).unwrap();
        assert_eq!(empty["pagination"]["returned"], 0);
        assert_eq!(empty["pagination"]["has_more"], false);
    }
    #[test]
    fn scoped_cache_decode_excludes_foreign_corruption_and_parameter_injection() {
        let (_root, store) = setup();
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE code_index_documents SET payload='broken' WHERE document_id='b'",
                [],
            )
            .unwrap();
        let q = RouteQuery {
            project: Some("alpha".into()),
            selection: CodeQuery {
                path: Some("/same.py".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let result = store.code_routes(&q).unwrap();
        assert_eq!(result["total"], 3);
        assert!(result["parse_errors"].as_array().unwrap().is_empty());
        assert!(store.code_routes(&RouteQuery::default()).is_err());
        let injected = store
            .code_routes(&RouteQuery {
                project: Some("alpha' OR 1=1 --".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(injected["total"], 0);
    }
}
