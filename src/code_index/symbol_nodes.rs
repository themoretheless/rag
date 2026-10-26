//! Typed inventory of captured identities, separate from source definition ownership.
use super::*;
use std::collections::BTreeMap;
#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Deserialize,
    Serialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    #[default]
    Code,
    TranslationKey,
    Table,
    Route,
    All,
}
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum NameMatch {
    #[default]
    Exact,
    Contains,
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct SymbolQuery {
    #[serde(flatten)]
    pub selection: CodeQuery,
    /// Exact project/wing, also scopes index-cache decoding and diagnostics.
    pub project: Option<String>,
    /// Alias of symbol for reference-style clients. Do not combine both selectors.
    pub name: Option<String>,
    /// code (default), translation_key, table, route, or all.
    pub symbol_kind: Option<SymbolKind>,
    /// Exact (default) or literal case-sensitive substring, never SQL/regex wildcards.
    pub match_mode: Option<NameMatch>,
    /// Source proof records retained per shared identity (default 5, clamped 1..50).
    pub evidence_limit: Option<usize>,
}
type ProofKey = (String, String, u64, u64, String, String);
#[derive(Default)]
struct NodeFacts {
    usages: usize,
    schema: usize,
    schema_complete: Option<bool>,
    counts: BTreeMap<String, usize>,
    proofs: Vec<(ProofKey, serde_json::Value)>,
    total: usize,
}
type Identity = (SymbolKind, Option<String>, String);
fn selected_kind(q: &SymbolQuery) -> SymbolKind {
    q.symbol_kind.unwrap_or_else(|| {
        let name = q
            .name
            .as_deref()
            .or(q.selection.symbol.as_deref())
            .unwrap_or("");
        if name.starts_with("tpl:") && !name.starts_with("tpl::") {
            SymbolKind::TranslationKey
        } else if name.starts_with("@table:") {
            SymbolKind::Table
        } else if name.starts_with("route:") && !name.starts_with("route::") {
            SymbolKind::Route
        } else {
            SymbolKind::Code
        }
    })
}
fn namespace(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::TranslationKey => "tpl:",
        SymbolKind::Table => "@table:",
        SymbolKind::Route => "route:",
        _ => "",
    }
}
fn id(kind: SymbolKind, project: &Option<String>, name: &str) -> String {
    format!(
        "captured:{}",
        content_hash(&serde_json::json!(["symbol_node_v1", kind, project, name]).to_string())
    )
}
fn matches(q: &SymbolQuery, id: &str, name: &str, qualified: &str, kind: SymbolKind) -> bool {
    let Some(selector) = q.name.as_deref().or(q.selection.symbol.as_deref()) else {
        return true;
    };
    if q.match_mode.unwrap_or_default() == NameMatch::Contains {
        return name.contains(selector) || qualified.contains(selector);
    }
    if selector == id || selector == name || selector == qualified {
        return true;
    }
    kind == SymbolKind::Table
        && sql::canonical(selector.strip_prefix("@table:").unwrap_or(selector), false)
            .ok()
            .is_some_and(|s| sql::table_matches(name, &s))
}
fn add(
    nodes: &mut BTreeMap<Identity, NodeFacts>,
    q: &SymbolQuery,
    identity: Identity,
    path: &str,
    proof: serde_json::Value,
    schema: bool,
) {
    if q.selection.path.as_deref().is_some_and(|p| p != path) {
        return;
    }
    let facts = nodes.entry(identity).or_default();
    facts.total += 1;
    if schema {
        facts.schema += 1;
    } else {
        facts.usages += 1;
    }
    *facts
        .counts
        .entry(proof["kind"].as_str().unwrap_or("unknown").into())
        .or_default() += 1;
    let key = (
        path.to_string(),
        proof["document_id"].as_str().unwrap_or("").to_string(),
        proof["line"].as_u64().unwrap_or(0),
        proof["start_byte"].as_u64().unwrap_or(0),
        proof["kind"].as_str().unwrap_or("").to_string(),
        proof.to_string(),
    );
    let position = facts
        .proofs
        .binary_search_by(|(k, _)| k.cmp(&key))
        .unwrap_or_else(|i| i);
    facts.proofs.insert(position, (key, proof));
    facts
        .proofs
        .truncate(q.evidence_limit.unwrap_or(5).clamp(1, 50));
}
fn inventory(snapshot: &CodeSnapshot, q: &SymbolQuery) -> Vec<serde_json::Value> {
    let selected = selected_kind(q);
    let mut nodes = BTreeMap::new();
    if matches!(selected, SymbolKind::TranslationKey | SymbolKind::All) {
        for o in &snapshot.translation_keys.items {
            add(
                &mut nodes,
                q,
                (SymbolKind::TranslationKey, o.project.clone(), o.key.clone()),
                &o.path,
                serde_json::json!({"kind":"uses_key","document_id":o.document_id,"path":o.path,"project":o.project,"revision":o.revision,"content_hash":o.content_hash,"line":o.line,"start_byte":o.start_byte,"end_byte":o.end_byte,"usage_kind":o.usage_kind,"key_encoding":o.key_encoding}),
                false,
            );
        }
    }
    if matches!(selected, SymbolKind::Table | SymbolKind::All) {
        for table in &snapshot.sql.tables {
            let identity = (SymbolKind::Table, table.project.clone(), table.name.clone());
            for e in &table.evidence {
                add(
                    &mut nodes,
                    q,
                    identity.clone(),
                    &e.path,
                    serde_json::json!({"kind":"schema_evidence","document_id":e.document_id,"path":e.path,"project":e.project,"revision":e.revision,"content_hash":e.content_hash,"line":e.line}),
                    true,
                );
            }
            if let Some(facts) = nodes.get_mut(&identity) {
                facts.schema_complete = Some(table.complete);
            }
        }
        for u in &snapshot.sql.usages {
            add(
                &mut nodes,
                q,
                (
                    SymbolKind::Table,
                    u.evidence.project.clone(),
                    u.table.clone(),
                ),
                &u.evidence.path,
                serde_json::json!({"kind":"queries","document_id":u.evidence.document_id,"path":u.evidence.path,"project":u.evidence.project,"revision":u.evidence.revision,"content_hash":u.evidence.content_hash,"line":u.evidence.line,"access":u.mode,"operation":u.operation}),
                false,
            );
        }
    }
    if matches!(selected, SymbolKind::Route | SymbolKind::All) {
        let mut seen = std::collections::BTreeSet::new();
        for e in &snapshot.entrypoints {
            let Some(route) = &e.route else {
                continue;
            };
            // Handler alternatives are not additional source occurrences.
            if !seen.insert((
                &e.document_id,
                e.start_byte,
                e.line,
                &e.kind,
                &e.method,
                &e.request_method,
                &e.reference_kind,
            )) {
                continue;
            }
            add(
                &mut nodes,
                q,
                (SymbolKind::Route, e.project.clone(), route.clone()),
                &e.path,
                serde_json::json!({"kind":if e.kind=="http_endpoint_reference"{"endpoint_reference"}else{"routes"},"document_id":e.document_id,"path":e.path,"project":e.project,"revision":e.revision,"content_hash":e.content_hash,"line":e.line,"start_byte":e.start_byte,"registration_method":e.method,"request_method":e.request_method,"reference_kind":e.reference_kind,"handler_binding_proven":false}),
                false,
            );
        }
    }
    nodes.into_iter().filter_map(|((kind,project,name),facts)| {
        let identity=id(kind,&project,&name);let qualified=format!("{}{name}",namespace(kind));if !matches(q,&identity,&name,&qualified,kind) {return None;}
        let selector=match kind {SymbolKind::TranslationKey=>"key",SymbolKind::Table=>"table",SymbolKind::Route=>"route",_=>unreachable!()};
        let mut reference_target=serde_json::json!({});reference_target[selector]=serde_json::json!(name);if let Some(p)=&project {reference_target["project"]=serde_json::json!(p);}
        let impact_selector=match kind {SymbolKind::TranslationKey=>"impact_key",SymbolKind::Table=>"impact_table",SymbolKind::Route=>"impact_route",_=>unreachable!()};
        let mut impact_target=serde_json::json!({});impact_target[impact_selector]=serde_json::json!(name);if let Some(p)=&project {impact_target["impact_project"]=serde_json::json!(p);}
        Some(serde_json::json!({"id":identity,"node_domain":kind,"kind":kind,"name":name,"qualified_name":qualified,"project":project,"path":null,"start_line":null,"end_line":null,"signature":null,"definition_location_available":false,"captured_schema_evidence":facts.schema>0,"captured_schema_complete":facts.schema_complete,"translation_definition_proven":false,"runtime_binding_proven":false,"observed_usage_total":facts.usages,"schema_evidence_total":facts.schema,"counts_by_relation":facts.counts,"source_evidence":facts.proofs.into_iter().map(|(_,v)|v).collect::<Vec<_>>(),"source_evidence_total":facts.total,"source_evidence_truncated":facts.total>q.evidence_limit.unwrap_or(5).clamp(1,50),"reference_target":reference_target,"impact_target":impact_target,"reference_target_scope":"project_required_when_other_projects_share_the_name_unassigned_project_not_selectable","evidence_scope":"selected_project_and_optional_source_path","source_bodies_included":false}))
    }).collect()
}
impl Store {
    pub fn code_symbols(&self, q: &SymbolQuery) -> Result<serde_json::Value> {
        if q.name.is_some() && q.selection.symbol.is_some() {
            return Err(AppError::config("choose symbol or name, not both"));
        }
        let snapshot = self.code_snapshot_in_project(q.project.as_deref())?;
        let selected = selected_kind(q);
        let mode = q.match_mode.unwrap_or_default();
        let limit = q.selection.limit.unwrap_or(50).clamp(1, 500);
        if selected == SymbolKind::Code && mode == NameMatch::Exact {
            let mut selection = q.selection.clone();
            if let Some(name) = &q.name {
                selection.symbol = Some(name.clone());
            }
            let mut result = snapshot.query("symbol", &selection);
            result["project"] = serde_json::json!(q.project);
            result["symbol_kind"] = serde_json::json!(selected);
            result["query_contract_version"] = 1.into();
            return Ok(result);
        }
        let mut items = inventory(&snapshot, q);
        if matches!(selected, SymbolKind::Code | SymbolKind::All) {
            for s in &snapshot.symbols {
                if q.selection.path.as_ref().is_none_or(|p| p == &s.path)
                    && matches(q, &s.id, &s.name, &s.qualified_name, SymbolKind::Code)
                {
                    let mut row = serde_json::to_value(s)?;
                    row["node_domain"] = serde_json::json!("code");
                    row["project"] = serde_json::json!(snapshot
                        .document_scopes
                        .get(&s.document_id)
                        .and_then(|d| d.project.as_ref()));
                    items.push(row);
                }
            }
        }
        items.sort_by_key(|i| {
            (
                i["node_domain"].as_str().unwrap_or("").to_owned(),
                i["project"].as_str().map(str::to_owned),
                i["qualified_name"].as_str().unwrap_or("").to_owned(),
                i["id"].as_str().unwrap_or("").to_owned(),
            )
        });
        let mut counts = BTreeMap::<String, usize>::new();
        for i in &items {
            *counts
                .entry(i["node_domain"].as_str().unwrap_or("code").into())
                .or_default() += 1;
        }
        let total = items.len();
        items.truncate(limit);
        Ok(
            serde_json::json!({"query_contract_version":1,"symbol_kind":selected,"match_mode":mode,"project":q.project,"items":items,"total":total,"truncated":total>limit,"counts_by_domain":counts,"index_rev":snapshot.index_revision,"view":"fresh_subset_of_revision_inputs","observation_consistency":"single_read_snapshot","source_bodies_included":false,"absence_proven":false,"stale_documents":snapshot.stale_documents,"unindexed_documents":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors,"shared_node_location_semantics":"no_single_definition_path_source_evidence_instead","key_capture_coverage":{"supported_documents":snapshot.translation_keys.coverage.len(),"limited_documents":snapshot.translation_keys.coverage.iter().filter(|c|c["complete"]!=true).count(),"scope":"selected_project_not_selector_or_path_specific","runtime_binding_proven":false}}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Document;
    fn put(store: &Store, id: &str, path: &str, project: &str, body: &str) {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("doc://{id}"),
                source_file: Some(path.into()),
                wing: Some(project.into()),
                content: body.into(),
                ..Default::default()
            })
            .unwrap();
    }
    fn query(kind: SymbolKind) -> SymbolQuery {
        SymbolQuery {
            project: Some("p".into()),
            symbol_kind: Some(kind),
            ..Default::default()
        }
    }
    #[test]
    fn shared_keys_have_stable_scoped_identity_no_fake_definition_and_bounded_proofs() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("keys.duckdb")).unwrap();
        put(&store, "a", "/p/a.html", "p", "%{key} %{key} @{key}");
        put(
            &store,
            "b",
            "/p/b.php",
            "p",
            "<?php function render(){Translate('key');}",
        );
        put(&store, "q", "/q/a.html", "q", "%{key}");
        store.index_code().unwrap();
        let r = store
            .code_symbols(&SymbolQuery {
                evidence_limit: Some(1),
                ..query(SymbolKind::TranslationKey)
            })
            .unwrap();
        assert_eq!(r["total"], 1);
        let node = &r["items"][0];
        assert_eq!(node["observed_usage_total"], 4);
        assert_eq!(node["source_evidence_total"], 4);
        assert_eq!(node["source_evidence"].as_array().unwrap().len(), 1);
        assert_eq!(node["source_evidence_truncated"], true);
        assert!(node["path"].is_null());
        assert_eq!(
            node["reference_target"],
            serde_json::json!({"key":"key","project":"p"})
        );
        let via_prefix = store
            .code_symbols(&SymbolQuery {
                name: Some("tpl:key".into()),
                project: Some("p".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(via_prefix["items"][0]["id"], node["id"]);
        put(&store, "a", "/p/a.html", "p", "\n%{key}");
        store.index_code().unwrap();
        let changed = store
            .code_symbols(&query(SymbolKind::TranslationKey))
            .unwrap();
        assert_eq!(changed["items"][0]["id"], node["id"]);
        assert_eq!(changed["items"][0]["observed_usage_total"], 2);
        let all = store
            .code_symbols(&SymbolQuery {
                symbol_kind: Some(SymbolKind::TranslationKey),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(all["total"], 2);
        assert_ne!(all["items"][0]["id"], all["items"][1]["id"]);
    }
    #[test]
    fn tables_keep_schema_only_and_usage_only_identities_with_exact_scoped_filters() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("table.duckdb")).unwrap();
        put(
            &store,
            "ddl",
            "/p/schema.sql",
            "p",
            "CREATE TABLE public.logs(id INT); CREATE TABLE admin.logs(id INT);",
        );
        put(
            &store,
            "use",
            "/p/use.sql",
            "p",
            "SELECT * FROM public.logs; SELECT * FROM users;",
        );
        store.index_code().unwrap();
        let r = store
            .code_symbols(&SymbolQuery {
                name: Some("logs".into()),
                selection: CodeQuery {
                    limit: Some(1),
                    ..Default::default()
                },
                ..query(SymbolKind::Table)
            })
            .unwrap();
        assert_eq!(r["total"], 2);
        assert_eq!(r["truncated"], true);
        let r = store
            .code_symbols(&SymbolQuery {
                name: Some("@table:admin.logs".into()),
                project: Some("p".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(r["items"][0]["observed_usage_total"], 0);
        assert_eq!(r["items"][0]["captured_schema_evidence"], true);
        let r = store
            .code_symbols(&SymbolQuery {
                name: Some("users".into()),
                ..query(SymbolKind::Table)
            })
            .unwrap();
        assert_eq!(r["items"][0]["captured_schema_evidence"], false);
        assert_eq!(r["items"][0]["observed_usage_total"], 1);
        let scoped = store
            .code_symbols(&SymbolQuery {
                selection: CodeQuery {
                    path: Some("/p/use.sql".into()),
                    ..Default::default()
                },
                ..query(SymbolKind::Table)
            })
            .unwrap();
        assert_eq!(scoped["total"], 2);
        assert!(scoped["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["schema_evidence_total"] == 0));
    }
    #[test]
    fn route_handler_alternatives_are_one_occurrence_and_reference_targets_roundtrip() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("route.duckdb")).unwrap();
        put(
            &store,
            "route",
            "/p/api.js",
            "p",
            "function handler(){} function handler(){} app.get('/items',handler); fetch('/items');",
        );
        store.index_code().unwrap();
        let r = store.code_symbols(&query(SymbolKind::Route)).unwrap();
        assert_eq!(r["total"], 1);
        assert_eq!(r["items"][0]["observed_usage_total"], 2);
        assert_eq!(r["items"][0]["counts_by_relation"]["routes"], 1);
        assert_eq!(r["items"][0]["counts_by_relation"]["endpoint_reference"], 1);
        let target: ReferenceQuery =
            serde_json::from_value(r["items"][0]["reference_target"].clone()).unwrap();
        assert_eq!(store.code_references(&target).unwrap()["total"], 2);
    }
    #[test]
    fn code_defaults_alias_contains_all_and_stale_source_coverage_preserve_legacy() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("code.duckdb")).unwrap();
        put(&store, "code", "/p/api.py", "p", "def Worker():\n pass\n");
        put(
            &store,
            "html",
            "/p/view.html",
            "p",
            "%{Work_key} %{literal%_key}",
        );
        store.index_code().unwrap();
        let r = store
            .code_symbols(&SymbolQuery {
                project: Some("p".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(r["total"], 1);
        assert_eq!(r["items"][0]["name"], "Worker");
        assert!(r["items"][0]["start_line"].is_number());
        let r = store
            .code_symbols(&SymbolQuery {
                name: Some("Work".into()),
                match_mode: Some(NameMatch::Contains),
                ..query(SymbolKind::All)
            })
            .unwrap();
        assert_eq!(r["total"], 2);
        assert_eq!(r["counts_by_domain"]["code"], 1);
        assert_eq!(r["counts_by_domain"]["translation_key"], 1);
        assert!(store
            .code_symbols(&SymbolQuery {
                name: Some("Worker".into()),
                selection: CodeQuery {
                    symbol: Some("Worker".into()),
                    ..Default::default()
                },
                ..Default::default()
            })
            .is_err());
        put(&store, "html", "/p/view.html", "p", "changed");
        let stale = store
            .code_symbols(&query(SymbolKind::TranslationKey))
            .unwrap();
        assert_eq!(stale["total"], 0);
        assert_eq!(stale["stale_documents"].as_array().unwrap().len(), 1);
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    use crate::models::Document;
    #[test]
    fn proof_retention_is_independent_bounded_and_uses_numeric_source_order() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("proofs.duckdb")).unwrap();
        store
            .upsert_document(&Document {
                id: "a".into(),
                uri: "doc://a".into(),
                source_file: Some("/p/a.html".into()),
                wing: Some("p".into()),
                content: "%{key}\n".repeat(80),
                ..Default::default()
            })
            .unwrap();
        store.index_code().unwrap();
        let mut q = SymbolQuery {
            symbol_kind: Some(SymbolKind::TranslationKey),
            project: Some("p".into()),
            evidence_limit: Some(100000),
            selection: CodeQuery {
                limit: Some(1),
                ..Default::default()
            },
            ..Default::default()
        };
        let r = store.code_symbols(&q).unwrap();
        let n = &r["items"][0];
        assert_eq!(n["source_evidence_total"], 80);
        assert_eq!(n["source_evidence"].as_array().unwrap().len(), 50);
        assert_eq!(n["source_evidence"][0]["line"], 1);
        assert_eq!(n["source_evidence"][49]["line"], 50);
        assert_eq!(n["source_evidence_truncated"], true);
        q.evidence_limit = Some(0);
        let r = store.code_symbols(&q).unwrap();
        assert_eq!(
            r["items"][0]["source_evidence"].as_array().unwrap().len(),
            1
        );
        assert_eq!(r["items"][0]["observed_usage_total"], 80);
    }
}
