//! Non-code reference targets retain literal identity, not runtime database/router binding.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ReferenceQuery {
    #[serde(flatten)]
    pub selection: CodeQuery,
    /// Exact project/wing, also scopes snapshot decoding and freshness diagnostics.
    pub project: Option<String>,
    /// SQL table target, qualified/quoted if ambiguous. Cannot combine with symbol or route.
    pub table: Option<String>,
    /// Exact captured route/endpoint URL literal. No template or deployment expansion.
    pub route: Option<String>,
    /// Exact captured translation key; PHP translator-name candidates and template placeholders.
    pub key: Option<String>,
    /// Table occurrences: all (default), read or write. Valid only with table.
    pub table_mode: Option<String>,
}
impl ReferenceQuery {
    fn validate(&self) -> Result<()> {
        if usize::from(self.table.is_some())
            + usize::from(self.route.is_some())
            + usize::from(self.key.is_some())
            + usize::from(self.selection.symbol.is_some())
            > 1
        {
            return Err(AppError::config(
                "references accepts one target: symbol, table, route or key",
            ));
        }
        if self.table.is_none() && self.table_mode.is_some() {
            return Err(AppError::config("table_mode requires table"));
        }
        if self
            .table_mode
            .as_deref()
            .is_some_and(|m| !matches!(m, "all" | "read" | "write"))
        {
            return Err(AppError::config("table_mode must be all, read or write"));
        }
        if (self.table.is_some() || self.route.is_some() || self.key.is_some())
            && self
                .selection
                .reference_mode
                .as_deref()
                .is_some_and(|m| m != "bound")
        {
            return Err(AppError::config(
                "lexical reference_mode is only available for symbols",
            ));
        }
        if let Some(key) = &self.key {
            translation_keys::validate_key(key)?;
        }
        if let Some(table) = &self.table {
            sql::canonical(table, false)?;
        }
        if self
            .route
            .as_ref()
            .is_some_and(|r| r.is_empty() || r.len() > 4096 || r.chars().any(char::is_control))
        {
            return Err(AppError::config("route must be a nonempty literal URL of at most 4096 bytes without control characters"));
        }
        Ok(())
    }
}
fn origin(symbol: Option<&Symbol>, document_id: &str, path: &str) -> serde_json::Value {
    match symbol {
        Some(s) => {
            serde_json::json!({"kind":"symbol","symbol_id":s.id,"name":s.name,"qualified_name":s.qualified_name,"document_id":s.document_id,"path":s.path,"revision":s.revision,"content_hash":s.content_hash,"start_byte":s.start_byte})
        }
        None => serde_json::json!({"kind":"file","document_id":document_id,"path":path}),
    }
}
fn owner<'a>(
    snapshot: &'a CodeSnapshot,
    id: Option<&str>,
    document_id: &str,
    project: &Option<String>,
) -> Option<&'a Symbol> {
    id.and_then(|id| {
        snapshot.symbols.iter().find(|s| {
            s.id == id
                && s.document_id == document_id
                && snapshot
                    .document_scopes
                    .get(document_id)
                    .is_some_and(|d| &d.project == project)
        })
    })
}
fn envelope(
    snapshot: &CodeSnapshot,
    q: &ReferenceQuery,
    kind: &str,
    target: &str,
) -> serde_json::Value {
    serde_json::json!({"query_contract_version":1,"target_kind":kind,"target":target,"project":q.project,"index_rev":snapshot.index_revision,"view":"fresh_subset_of_revision_inputs","observation_consistency":"single_project_scoped_read_snapshot","items":[],"total":0,"truncated":false,"ownerless_total":0,"counts_by_kind":{},"needs_disambiguation":false,"stale_documents":snapshot.stale_documents,"unindexed_documents":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors,"runtime_binding_proven":false,"absence_proven":false,"body_text_included":false,"path_scope":"source_occurrences_only_identity_disambiguation_precedes_path_and_access_filters"})
}
fn finish(result: &mut serde_json::Value, mut items: Vec<serde_json::Value>, limit: usize) {
    items.sort_by(|a, b| {
        (
            a["evidence"]["path"].as_str(),
            a["evidence"]["line"].as_u64(),
            a["evidence"]["start_byte"].as_u64(),
            a["kind"].as_str(),
            a["access"].as_str(),
            a["operation"].as_str(),
            a["evidence"]["document_id"].as_str(),
            a["registration_method"].as_str(),
            a["request_method"].as_str(),
            a["reference_kind"].as_str(),
            a["from"]["symbol_id"].as_str(),
        )
            .cmp(&(
                b["evidence"]["path"].as_str(),
                b["evidence"]["line"].as_u64(),
                b["evidence"]["start_byte"].as_u64(),
                b["kind"].as_str(),
                b["access"].as_str(),
                b["operation"].as_str(),
                b["evidence"]["document_id"].as_str(),
                b["registration_method"].as_str(),
                b["request_method"].as_str(),
                b["reference_kind"].as_str(),
                b["from"]["symbol_id"].as_str(),
            ))
    });
    let mut counts = BTreeMap::<&str, usize>::new();
    for item in &items {
        *counts
            .entry(item["kind"].as_str().unwrap_or("unknown"))
            .or_default() += 1;
    }
    result["counts_by_kind"] = serde_json::json!(counts);
    result["total"] = items.len().into();
    result["truncated"] = (items.len() > limit).into();
    result["ownerless_total"] = items
        .iter()
        .filter(|i| i["from"]["kind"] == "file")
        .count()
        .into();
    items.truncate(limit);
    result["items"] = serde_json::json!(items);
}
fn table_references(
    snapshot: &CodeSnapshot,
    q: &ReferenceQuery,
    limit: usize,
) -> Result<serde_json::Value> {
    let target = sql::canonical(q.table.as_deref().unwrap_or(""), false)?;
    let mut result = envelope(snapshot, q, "table", &target);
    let mut identities: BTreeSet<(Option<String>, String)> = BTreeSet::new();
    for table in &snapshot.sql.tables {
        if sql::table_matches(&table.name, &target) {
            identities.insert((table.project.clone(), table.name.clone()));
        }
    }
    for usage in &snapshot.sql.usages {
        if sql::table_matches(&usage.table, &target) {
            identities.insert((usage.evidence.project.clone(), usage.table.clone()));
        }
    }
    result["target_candidates"] = serde_json::json!(identities
        .iter()
        .take(limit)
        .map(|(project, table)| serde_json::json!({"project":project,"table":table}))
        .collect::<Vec<_>>());
    result["target_candidates_total"] = identities.len().into();
    result["target_candidates_truncated"] = (identities.len() > limit).into();
    result["target_observed"] = (!identities.is_empty()).into();
    let diagnostics: Vec<_> = snapshot
        .sql
        .diagnostics
        .iter()
        .filter(|d| {
            q.selection
                .path
                .as_ref()
                .is_none_or(|p| d["evidence"]["path"].as_str() == Some(p))
        })
        .collect();
    result["sql_diagnostics"] = serde_json::json!({"items":diagnostics.iter().take(limit).collect::<Vec<_>>(),"total":diagnostics.len(),"truncated":diagnostics.len()>limit,"scope":"selected_project_and_source_path_not_table_specific"});
    if identities.len() > 1 {
        result["needs_disambiguation"] = true.into();
        return Ok(result);
    }
    if let Some((project, table)) = identities.first() {
        result["resolved_target"] = serde_json::json!({"project":project,"table":table});
    }
    let mode = q.table_mode.as_deref().unwrap_or("all");
    let items=snapshot.sql.usages.iter().filter(|u|sql::table_matches(&u.table,&target) && q.selection.path.as_ref().is_none_or(|p|p==&u.evidence.path) && (mode=="all" || mode==u.mode)).map(|u|{
        let symbol=owner(snapshot,u.symbol.as_deref(),&u.evidence.document_id,&u.evidence.project);
        serde_json::json!({"kind":"queries","from":origin(symbol,&u.evidence.document_id,&u.evidence.path),"owner_resolution":if symbol.is_some(){"captured_symbol"}else{"ownerless_or_unqualified"},"target":{"project":u.evidence.project,"table":u.table},"access":u.mode,"operation":u.operation,"evidence":u.evidence,"provenance":"derived_static_sql","database_binding_proven":false})
    }).collect();
    result["table_mode"] = serde_json::json!(mode);
    finish(&mut result, items, limit);
    Ok(result)
}
fn route_references(
    snapshot: &CodeSnapshot,
    q: &ReferenceQuery,
    limit: usize,
) -> serde_json::Value {
    let target = q.route.as_deref().unwrap_or("");
    let mut result = envelope(snapshot, q, "route", target);
    let entries: Vec<_> = snapshot
        .entrypoints
        .iter()
        .filter(|e| e.route.as_deref() == Some(target))
        .collect();
    let identities: BTreeSet<_> = entries.iter().map(|e| &e.project).collect();
    result["target_candidates"] = serde_json::json!(identities
        .iter()
        .take(limit)
        .map(|p| serde_json::json!({"project":p,"route":target}))
        .collect::<Vec<_>>());
    result["target_candidates_total"] = identities.len().into();
    result["target_candidates_truncated"] = (identities.len() > limit).into();
    result["target_observed"] = (!identities.is_empty()).into();
    if identities.len() > 1 {
        result["needs_disambiguation"] = true.into();
        return result;
    }
    if let Some(project) = identities.first() {
        result["resolved_target"] = serde_json::json!({"project":project,"route":target});
    }
    // A registration can have several handler candidates. They are alternatives at
    // one occurrence, not multiple uses of the URL or the registration's owner.
    let mut occurrences = BTreeMap::new();
    for entry in entries
        .into_iter()
        .filter(|e| q.selection.path.as_ref().is_none_or(|p| p == &e.path))
    {
        occurrences
            .entry((
                &entry.document_id,
                entry.start_byte,
                entry.line,
                &entry.kind,
                &entry.method,
                &entry.reference_kind,
                &entry.request_method,
            ))
            .or_insert_with(Vec::new)
            .push(entry);
    }
    let items=occurrences.into_values().map(|entries| {
        let e=entries[0];
        let mut handler_ids=BTreeSet::new();
        let handlers:Vec<_>=entries.iter().filter_map(|entry|entry.symbol_id.as_deref()).filter(|id|handler_ids.insert(*id)).filter_map(|id|snapshot.symbols.iter().find(|s|s.id==id && snapshot.document_scopes.get(&s.document_id).is_some_and(|d|d.project==e.project))).collect();
        let (kind,relation)=if e.kind=="http_endpoint_reference" {("endpoint_reference","literal_client_url")}else{("routes","registration_candidate")};
        serde_json::json!({"kind":kind,"from":origin(None,&e.document_id,&e.path),"owner_resolution":"source_file","handler_candidates":handlers.iter().take(50).map(|s|origin(Some(s),&s.document_id,&s.path)).collect::<Vec<_>>(),"handler_candidates_total":handlers.len(),"handler_candidates_truncated":handlers.len()>50,"target":{"project":e.project,"route":e.route},"relation":relation,"registration_method":e.method,"request_method":e.request_method,"request_method_basis":e.request_method_basis,"reference_kind":e.reference_kind,"handler_resolution":e.resolution,"evidence":{"document_id":e.document_id,"path":e.path,"line":e.line,"start_byte":e.start_byte,"project":e.project,"revision":e.revision,"content_hash":e.content_hash},"provenance":e.provenance,"handler_binding_from_url_proven":false})
    }).collect();
    finish(&mut result, items, limit);
    result
}
fn key_references(snapshot: &CodeSnapshot, q: &ReferenceQuery, limit: usize) -> serde_json::Value {
    let key = q.key.as_deref().unwrap_or("");
    let mut result = envelope(snapshot, q, "translation_key", key);
    let facts = &snapshot.translation_keys;
    let matching: Vec<_> = facts.items.iter().filter(|o| o.key == key).collect();
    let projects: BTreeSet<_> = matching.iter().map(|o| &o.project).collect();
    result["target_observed"] = (!matching.is_empty()).into();
    result["target_candidates"] = serde_json::json!(projects
        .iter()
        .take(limit)
        .map(|p| serde_json::json!({"project":p,"key":key}))
        .collect::<Vec<_>>());
    result["target_candidates_total"] = projects.len().into();
    result["target_candidates_truncated"] = (projects.len() > limit).into();
    result["translation_definition_proven"] = false.into();
    let coverage: Vec<_> = facts
        .coverage
        .iter()
        .filter(|c| {
            q.selection
                .path
                .as_ref()
                .is_none_or(|p| c["path"].as_str() == Some(p))
        })
        .collect();
    result["key_capture_coverage"] = serde_json::json!({"items":coverage.iter().take(limit).collect::<Vec<_>>(),"total":coverage.len(),"truncated":coverage.len()>limit,"limited_captures":coverage.iter().filter(|c|c["complete"]!=true).count(),"scope":"selected_project_and_source_path_supported_syntax_only","runtime_binding_proven":false,"unsupported_formats_scanned":false});
    if projects.len() > 1 {
        result["needs_disambiguation"] = true.into();
        return result;
    }
    if let Some(project) = projects.first() {
        result["resolved_target"] = serde_json::json!({"project":project,"key":key});
    }
    let items=matching.into_iter().filter(|o|q.selection.path.as_ref().is_none_or(|p|p==&o.path)).map(|o|{
        let symbol=owner(snapshot,o.owner.as_deref(),&o.document_id,&o.project);
        serde_json::json!({"kind":"uses_key","from":origin(symbol,&o.document_id,&o.path),"target":{"project":o.project,"key":o.key},"usage_kind":o.usage_kind,"key_encoding":o.key_encoding,"source_spelling_may_be_encoded":o.key_encoding!="literal_placeholder","syntax":o.syntax,"evidence":{"document_id":o.document_id,"path":o.path,"project":o.project,"revision":o.revision,"content_hash":o.content_hash,"start_byte":o.start_byte,"end_byte":o.end_byte,"line":o.line,"column":o.column,"column_unit":"UTF8_bytes_1_based"},"provenance":"derived_literal_syntax","callee_binding_proven":false,"template_evaluation_proven":false,"translation_definition_proven":false})
    }).collect();
    finish(&mut result, items, limit);
    result
}
impl Store {
    pub fn code_references(&self, q: &ReferenceQuery) -> Result<serde_json::Value> {
        q.validate()?;
        let snapshot = self.code_snapshot_in_project(q.project.as_deref())?;
        let limit = q.selection.limit.unwrap_or(50).clamp(1, 500);
        if q.table.is_some() {
            table_references(&snapshot, q, limit)
        } else if q.route.is_some() {
            Ok(route_references(&snapshot, q, limit))
        } else if q.key.is_some() {
            Ok(key_references(&snapshot, q, limit))
        } else {
            let mut value = snapshot.query("references", &q.selection);
            value["project"] = serde_json::json!(q.project);
            Ok(value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Document;
    fn put(store: &Store, id: &str, path: &str, project: &str, content: &str) {
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
    fn table(name: &str, project: Option<&str>) -> ReferenceQuery {
        ReferenceQuery {
            table: Some(name.into()),
            project: project.map(str::to_owned),
            ..Default::default()
        }
    }
    #[test]
    fn sql_target_retains_owners_ownerless_uses_and_independent_counts() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("table.duckdb")).unwrap();
        put(&store,"code","/p/api.py","p","def load():\n q='SELECT * FROM users'\ndef save():\n q='INSERT INTO users(id) VALUES(1)'\n");
        put(&store, "raw", "/p/raw.sql", "p", "SELECT * FROM users;");
        put(
            &store,
            "ddl",
            "/p/schema.sql",
            "p",
            "CREATE TABLE empty_table(id INT);",
        );
        store.index_code().unwrap();
        let q = table("users", Some("p"));
        let r = store.code_references(&q).unwrap();
        assert_eq!(r["total"], 3, "{r}");
        assert_eq!(r["ownerless_total"], 1);
        assert_eq!(r["counts_by_kind"]["queries"], 3);
        assert_eq!(
            r["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|i| i["from"]["kind"] == "symbol")
                .count(),
            2
        );
        assert!(r["items"].as_array().unwrap().iter().all(
            |i| i["evidence"]["revision"].is_i64() && i["evidence"]["content_hash"].is_string()
        ));
        assert_eq!(r, store.code_references(&q).unwrap());
        let read = store
            .code_references(&ReferenceQuery {
                table_mode: Some("read".into()),
                selection: CodeQuery {
                    limit: Some(1),
                    ..Default::default()
                },
                ..table("users", Some("p"))
            })
            .unwrap();
        assert_eq!(read["total"], 2);
        assert_eq!(read["items"].as_array().unwrap().len(), 1);
        assert_eq!(read["truncated"], true);
        let empty = store
            .code_references(&table("empty_table", Some("p")))
            .unwrap();
        assert_eq!(empty["target_observed"], true);
        assert_eq!(empty["total"], 0);
        assert_eq!(empty["absence_proven"], false);
    }
    #[test]
    fn table_identity_disambiguation_precedes_access_and_source_filters() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("identity.duckdb")).unwrap();
        put(
            &store,
            "public",
            "/p/read.sql",
            "p",
            "SELECT * FROM public.logs;",
        );
        put(
            &store,
            "admin",
            "/p/schema.sql",
            "p",
            "CREATE TABLE admin.logs(id INT);",
        );
        put(
            &store,
            "foreign",
            "/q/read.sql",
            "q",
            "SELECT * FROM public.logs;",
        );
        store.index_code().unwrap();
        let mut q = table("logs", Some("p"));
        q.table_mode = Some("write".into());
        q.selection.path = Some("/p/read.sql".into());
        let r = store.code_references(&q).unwrap();
        assert_eq!(r["needs_disambiguation"], true);
        assert_eq!(r["target_candidates_total"], 2);
        assert_eq!(r["total"], 0);
        let r = store.code_references(&table("public.logs", None)).unwrap();
        assert_eq!(r["needs_disambiguation"], true);
        assert_eq!(r["target_candidates_total"], 2);
        let r = store
            .code_references(&table("public.logs", Some("p")))
            .unwrap();
        assert_eq!(r["total"], 1);
        assert_eq!(r["needs_disambiguation"], false);
        assert!(store
            .code_references(&table("users; DROP TABLE users", Some("p")))
            .is_err());
        assert!(store
            .code_references(&ReferenceQuery {
                route: Some("/x".into()),
                ..table("users", Some("p"))
            })
            .is_err());
    }
    #[test]
    fn route_literals_keep_registrations_separate_from_client_mentions_and_projects() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("route.duckdb")).unwrap();
        put(
            &store,
            "api",
            "/p/api.py",
            "p",
            "@app.get('/items')\ndef items():\n return 'PRIVATE_BODY'\n",
        );
        put(
            &store,
            "js",
            "/p/client.js",
            "p",
            "fetch('/items', {method:'POST'}); fetch('/itemsets');\n",
        );
        put(
            &store,
            "html",
            "/p/index.html",
            "p",
            "<a href='/items'>Link</a>",
        );
        put(&store, "foreign", "/q/client.js", "q", "fetch('/items');");
        store.index_code().unwrap();
        let q = ReferenceQuery {
            route: Some("/items".into()),
            project: Some("p".into()),
            ..Default::default()
        };
        let r = store.code_references(&q).unwrap();
        assert_eq!(r["total"], 3, "{r}");
        assert_eq!(r["counts_by_kind"]["routes"], 1);
        assert_eq!(r["counts_by_kind"]["endpoint_reference"], 2);
        let items = r["items"].as_array().unwrap();
        assert!(items.iter().any(|i| i["kind"] == "routes"
            && i["registration_method"] == "GET"
            && i["request_method"].is_null()));
        assert!(items.iter().any(|i| i["kind"] == "endpoint_reference"
            && i["request_method"] == "POST"
            && i["registration_method"].is_null()));
        assert!(!r.to_string().contains("PRIVATE_BODY"));
        let scoped = store
            .code_references(&ReferenceQuery {
                selection: CodeQuery {
                    path: Some("/p/client.js".into()),
                    ..Default::default()
                },
                ..q
            })
            .unwrap();
        assert_eq!(scoped["total"], 1);
        let ambiguous = store
            .code_references(&ReferenceQuery {
                route: Some("/items".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(ambiguous["needs_disambiguation"], true);
        assert_eq!(ambiguous["target_candidates_total"], 2);
    }
    #[test]
    fn ambiguous_handlers_are_alternatives_at_one_registration_not_source_owners() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("handlers.duckdb")).unwrap();
        put(
            &store,
            "js",
            "/p/api.js",
            "p",
            "function handle() {}\nfunction handle() {}\napp.get('/items', handle);\n",
        );
        store.index_code().unwrap();
        let r = store
            .code_references(&ReferenceQuery {
                route: Some("/items".into()),
                project: Some("p".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(r["total"], 1, "{r}");
        let row = &r["items"][0];
        assert_eq!(row["from"]["kind"], "file");
        assert_eq!(row["handler_candidates_total"], 2);
        assert_eq!(row["handler_resolution"], "ambiguous");
    }
    #[test]
    fn symbol_compatibility_project_scope_and_stale_noncode_exclusion() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("stale.duckdb")).unwrap();
        put(
            &store,
            "code",
            "/p/api.py",
            "p",
            "def work():\n pass\ndef use():\n work()\n q='SELECT * FROM users'\n",
        );
        store.index_code().unwrap();
        let q = ReferenceQuery {
            selection: CodeQuery {
                symbol: Some("work".into()),
                ..Default::default()
            },
            project: Some("p".into()),
            ..Default::default()
        };
        let r = store.code_references(&q).unwrap();
        assert_eq!(r["total"], 1);
        assert_eq!(r["reference_mode"], "bound");
        put(&store, "code", "/p/api.py", "p", "def work():\n pass\n");
        let stale = store.code_references(&table("users", Some("p"))).unwrap();
        assert_eq!(stale["total"], 0);
        assert_eq!(stale["stale_documents"].as_array().unwrap().len(), 1);
        store.index_code().unwrap();
        let fresh = store.code_references(&table("users", Some("p"))).unwrap();
        assert_eq!(fresh["target_observed"], false);
        assert!(fresh["stale_documents"].as_array().unwrap().is_empty());
    }
}
