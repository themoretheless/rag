//! Incoming static dependencies of literal key/URL occurrences, never runtime dispatch.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
pub(super) fn validate(q: &CodeQuery) -> Option<String> {
    if q.impact_relations
        .as_deref()
        .is_some_and(|m| !matches!(m, "calls" | "references" | "all"))
    {
        return Some("impact_relations must be calls, references or all".into());
    }
    let count = usize::from(q.symbol.is_some())
        + usize::from(q.impact_file.is_some() || q.impact_document_id.is_some())
        + usize::from(q.impact_table.is_some())
        + usize::from(q.impact_key.is_some())
        + usize::from(q.impact_route.is_some());
    if count > 1 {
        return Some(
            "impact accepts one target: symbol, file/document, table, key or route".into(),
        );
    }
    if q.impact_table_mode.is_some() && q.impact_table.is_none() {
        return Some("impact_table_mode requires impact_table".into());
    }
    if q.impact_route_kind.is_some() && q.impact_route.is_none() {
        return Some("impact_route_kind requires impact_route".into());
    }
    if q.impact_route_kind
        .as_deref()
        .is_some_and(|k| !matches!(k, "all" | "registration" | "endpoint"))
    {
        return Some("impact_route_kind must be all, registration or endpoint".into());
    }
    if let Some(key) = &q.impact_key {
        if let Err(e) = translation_keys::validate_key(key) {
            return Some(e.to_string());
        }
    }
    if q.impact_route
        .as_ref()
        .is_some_and(|r| r.is_empty() || r.len() > 4096 || r.chars().any(char::is_control))
    {
        return Some(
            "impact_route must be a nonempty literal URL of at most 4096 bytes without controls"
                .into(),
        );
    }
    None
}
struct Direct<'a> {
    doc: &'a str,
    path: &'a str,
    project: &'a Option<String>,
    owner: Option<&'a Symbol>,
    proof: serde_json::Value,
}
fn owner<'a>(
    snapshot: &'a CodeSnapshot,
    id: Option<&str>,
    doc: &str,
    project: &Option<String>,
) -> Option<&'a Symbol> {
    id.and_then(|id| {
        snapshot.symbols.iter().find(|s| {
            s.id == id
                && s.document_id == doc
                && snapshot
                    .document_scopes
                    .get(doc)
                    .is_some_and(|d| &d.project == project)
        })
    })
}
fn literal_sources<'a>(
    snapshot: &'a CodeSnapshot,
    q: &CodeQuery,
) -> (Vec<Direct<'a>>, BTreeSet<Option<String>>) {
    let mut direct = Vec::new();
    let mut projects = BTreeSet::new();
    let in_project = |p: &Option<String>| {
        q.impact_project
            .as_ref()
            .is_none_or(|v| p.as_ref() == Some(v))
    };
    if let Some(key) = &q.impact_key {
        for o in &snapshot.translation_keys.items {
            if &o.key != key || !in_project(&o.project) {
                continue;
            }
            projects.insert(o.project.clone());
            if q.path.as_ref().is_some_and(|p| p != &o.path) {
                continue;
            }
            let source_owner = owner(snapshot, o.owner.as_deref(), &o.document_id, &o.project);
            let proof = serde_json::json!({"kind":"uses_key","key":o.key,"usage_kind":o.usage_kind,"syntax":o.syntax,"key_encoding":o.key_encoding,"owner_symbol_id":source_owner.map(|s|&s.id),"owner_resolution":if source_owner.is_some(){"captured_lexical_owner"}else{"source_file"},"evidence":{"document_id":o.document_id,"path":o.path,"project":o.project,"revision":o.revision,"content_hash":o.content_hash,"line":o.line,"start_byte":o.start_byte,"end_byte":o.end_byte},"runtime_key_binding_proven":false});
            direct.push(Direct {
                doc: &o.document_id,
                path: &o.path,
                project: &o.project,
                owner: source_owner,
                proof,
            });
        }
    } else if let Some(url) = &q.impact_route {
        let mode = q.impact_route_kind.as_deref().unwrap_or("all");
        let mut groups = BTreeMap::new();
        for e in &snapshot.entrypoints {
            if e.route.as_ref() != Some(url)
                || !in_project(&e.project)
                || snapshot.parse_errors.contains(&e.document_id)
            {
                continue;
            }
            projects.insert(e.project.clone());
            let endpoint = e.kind == "http_endpoint_reference";
            if q.path.as_ref().is_some_and(|p| p != &e.path)
                || (mode == "registration" && endpoint)
                || (mode == "endpoint" && !endpoint)
            {
                continue;
            }
            groups
                .entry((
                    &e.document_id,
                    e.start_byte,
                    e.line,
                    &e.kind,
                    &e.method,
                    &e.reference_kind,
                    &e.request_method,
                ))
                .or_insert_with(Vec::new)
                .push(e);
        }
        for entries in groups.into_values() {
            let e = entries[0];
            // Registration handler candidates never become source owners. Only a
            // captured enclosing definition at the registration/client span can seed.
            let source_owner = snapshot
                .symbols
                .iter()
                .filter(|s| {
                    s.document_id == e.document_id
                        && s.start_byte <= e.start_byte
                        && e.start_byte < s.end_byte
                        && snapshot
                            .document_scopes
                            .get(&s.document_id)
                            .is_some_and(|d| d.project == e.project)
                })
                .min_by_key(|s| (s.end_byte - s.start_byte, &s.id));
            let handlers: BTreeSet<_> = entries
                .iter()
                .filter_map(|e| e.symbol_id.as_ref())
                .collect();
            let proof = serde_json::json!({"kind":if e.kind=="http_endpoint_reference"{"endpoint_reference"}else{"routes"},"url":e.route,"registration_method":e.method,"request_method":e.request_method,"request_method_basis":e.request_method_basis,"reference_kind":e.reference_kind,"owner_symbol_id":source_owner.map(|s|&s.id),"owner_resolution":if source_owner.is_some(){"captured_enclosing_definition"}else{"source_file"},"handler_candidate_ids":handlers.iter().take(50).collect::<Vec<_>>(),"handler_candidates_total":handlers.len(),"handler_candidates_truncated":handlers.len()>50,"handler_candidates_are_seeds":false,"evidence":{"document_id":e.document_id,"path":e.path,"project":e.project,"revision":e.revision,"content_hash":e.content_hash,"line":e.line,"start_byte":e.start_byte},"runtime_url_binding_proven":false});
            direct.push(Direct {
                doc: &e.document_id,
                path: &e.path,
                project: &e.project,
                owner: source_owner,
                proof,
            });
        }
    }
    direct.sort_by_key(|d| {
        (
            d.path,
            d.doc,
            d.proof["evidence"]["line"].as_u64(),
            d.proof["evidence"]["start_byte"].as_u64(),
            d.proof.to_string(),
        )
    });
    (direct, projects)
}
pub(super) fn query(snapshot: &CodeSnapshot, q: &CodeQuery, limit: usize) -> serde_json::Value {
    let (direct, projects) = literal_sources(snapshot, q);
    let key_target = q.impact_key.is_some();
    let target = q.impact_key.as_ref().or(q.impact_route.as_ref()).unwrap();
    let coverage: Vec<_> = snapshot
        .translation_keys
        .coverage
        .iter()
        .filter(|c| {
            q.impact_project
                .as_ref()
                .is_none_or(|p| c["project"].as_str() == Some(p))
                && q.path
                    .as_ref()
                    .is_none_or(|p| c["path"].as_str() == Some(p))
        })
        .collect();
    let mut result = serde_json::json!({"target_kind":if key_target{"translation_key"}else{"route"},"target":target,"impact_project":q.impact_project,"target_observed":!projects.is_empty(),"target_candidates":projects.iter().take(limit).map(|p|serde_json::json!({"project":p,"target":target})).collect::<Vec<_>>(),"target_candidates_total":projects.len(),"target_candidates_truncated":projects.len()>limit,"needs_disambiguation":projects.len()>1,"items":[],"runtime_impact_proven":false,"absence_proven":false,"source_bodies_included":false,"stale_documents":snapshot.stale_documents,"unindexed_documents":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors,"query_contract_version":1,"program_impact_complete":false});
    if key_target {
        result["key_capture_coverage"] = serde_json::json!({"items":coverage.iter().take(limit).collect::<Vec<_>>(),"total":coverage.len(),"truncated":coverage.len()>limit,"limited_captures":coverage.iter().filter(|c|c["complete"]!=true).count(),"scope":"project_and_source_path_not_key_specific"});
    }
    if projects.len() > 1 {
        return result;
    }
    let roots: BTreeSet<_> = direct
        .iter()
        .filter_map(|d| d.owner.map(|s| s.id.as_str()))
        .collect();
    let owners: Vec<_> = snapshot
        .symbols
        .iter()
        .filter(|s| roots.contains(s.id.as_str()))
        .collect();
    let documents: BTreeSet<_> = direct.iter().map(|d| d.doc.to_owned()).collect();
    let mut traversal = q.clone();
    traversal.impact_relations = Some(
        q.impact_relations
            .clone()
            .unwrap_or_else(|| "references".into()),
    );
    let code = impact::query(snapshot, &traversal, &owners, limit);
    if code.get("error").is_some() {
        return code;
    }
    let files = file_impact::from_documents(snapshot, &traversal, &documents, limit);
    if files.get("error").is_some() {
        return files;
    }
    let mut direct_files = BTreeMap::new();
    for d in &direct {
        let entry = direct_files
            .entry(d.doc)
            .or_insert((d.path, d.project, 0usize));
        entry.2 += 1;
    }
    result["direct_occurrences"] = serde_json::json!(direct
        .iter()
        .take(limit)
        .map(|d| &d.proof)
        .collect::<Vec<_>>());
    result["direct_total"] = direct.len().into();
    result["direct_truncated"] = (direct.len() > limit).into();
    result["direct_owner_symbols"] = owners.len().into();
    result["ownerless_direct_uses"] = direct.iter().filter(|d| d.owner.is_none()).count().into();
    result["direct_files"]=serde_json::json!(direct_files.iter().take(limit).map(|(id,(path,project,count))|serde_json::json!({"document_id":id,"path":path,"project":project,"occurrences":count})).collect::<Vec<_>>());
    result["direct_files_total"] = direct_files.len().into();
    result["direct_files_truncated"] = (direct_files.len() > limit).into();
    result["code_impact"] = code;
    result["file_impact"] = files;
    result["items"] = result["code_impact"]["items"].clone();
    result["affected_files"] = result["code_impact"]["affected_files"].clone();
    result["truncated"] = serde_json::json!(
        result["direct_truncated"] == true
            || result["direct_files_truncated"] == true
            || result["code_impact"]["truncated"] == true
            || result["file_impact"]["truncated"] == true
    );
    result["depth_limited"] = serde_json::json!(
        result["code_impact"]["depth_limited"] == true
            || result["file_impact"]["depth_limited"] == true
    );
    result["traversal_scope"] = "all_direct_owners_and_source_files_before_output_limits".into();
    result["depth_origin"] =
        "direct_owners_and_files_are_depth_zero_indirect_results_start_at_one".into();
    result["impact_relations"] = serde_json::json!(traversal.impact_relations);
    result["impact_route_kind"] = serde_json::json!(q.impact_route_kind);
    result["direct_usage_semantics"] =
        "captured_literal_syntax_not_runtime_requests_or_translations".into();
    result
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
    fn run(store: &Store, q: &CodeQuery) -> serde_json::Value {
        store
            .code_snapshot_in_project(q.impact_project.as_deref())
            .unwrap()
            .query("impact", q)
    }
    fn names(store: &Store, result: &serde_json::Value) -> BTreeSet<String> {
        let snapshot = store.code_snapshot().unwrap();
        result["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| {
                snapshot
                    .symbols
                    .iter()
                    .find(|s| Some(s.id.as_str()) == i["symbol_id"].as_str())
            })
            .map(|s| s.name.clone())
            .collect()
    }
    #[test]
    fn key_impact_seeds_all_php_owners_and_ownerless_template_files_before_limits() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("key.duckdb")).unwrap();
        put(&store,"php","/p/api.php","p","<?php function render(){Translate('key');} function serve(){render();} function main(){serve();}");
        put(&store, "view", "/p/view.html", "p", "%{key}");
        put(
            &store,
            "site",
            "/p/site.html",
            "p",
            "<iframe src='view.html'></iframe>",
        );
        store.index_code().unwrap();
        let q = CodeQuery {
            impact_key: Some("key".into()),
            impact_project: Some("p".into()),
            ..Default::default()
        };
        let r = run(&store, &q);
        assert_eq!(r["direct_total"], 2, "{r}");
        assert_eq!(r["direct_owner_symbols"], 1);
        assert_eq!(r["ownerless_direct_uses"], 1);
        assert_eq!(r["direct_files_total"], 2);
        assert_eq!(
            names(&store, &r),
            BTreeSet::from(["serve".into(), "main".into()])
        );
        assert_eq!(r["file_impact"]["items"][0]["path"], "/p/site.html");
        assert_eq!(r["file_impact"]["seed_documents_total"], 2);
        let limited = run(
            &store,
            &CodeQuery {
                limit: Some(1),
                ..q.clone()
            },
        );
        assert_eq!(limited["direct_truncated"], true);
        assert_eq!(limited["file_impact"]["seed_documents_total"], 2);
        assert_eq!(limited["direct_owner_symbols"], 1);
        assert_eq!(limited["file_impact"]["items"][0]["path"], "/p/site.html");
        let calls = run(
            &store,
            &CodeQuery {
                impact_relations: Some("calls".into()),
                ..q
            },
        );
        assert!(calls["file_impact"]["items"].as_array().unwrap().is_empty());
        assert_eq!(calls["direct_total"], 2);
    }
    #[test]
    fn route_impact_seeds_source_owners_not_registered_handler_candidates() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("url.duckdb")).unwrap();
        put(
            &store,
            "api",
            "/p/api.py",
            "p",
            "@app.get('/items')\ndef handle(): pass\ndef handler_caller(): handle()\n",
        );
        put(
            &store,
            "js",
            "/p/client.js",
            "p",
            "function load(){fetch('/items',{method:'POST'});} function main(){load();}",
        );
        put(
            &store,
            "html",
            "/p/view.html",
            "p",
            "<a href='/items'>Link</a>",
        );
        store.index_code().unwrap();
        let q = CodeQuery {
            impact_route: Some("/items".into()),
            impact_project: Some("p".into()),
            ..Default::default()
        };
        let r = run(&store, &q);
        assert_eq!(r["direct_total"], 3, "{r}");
        assert_eq!(r["direct_owner_symbols"], 1);
        assert_eq!(names(&store, &r), BTreeSet::from(["main".into()]));
        let registration = run(
            &store,
            &CodeQuery {
                impact_route_kind: Some("registration".into()),
                ..q.clone()
            },
        );
        assert_eq!(registration["direct_total"], 1);
        assert_eq!(registration["direct_owner_symbols"], 0);
        assert!(registration["items"].as_array().unwrap().is_empty());
        assert_eq!(
            registration["direct_occurrences"][0]["handler_candidates_total"],
            1
        );
        assert_eq!(
            registration["direct_occurrences"][0]["handler_candidates_are_seeds"],
            false
        );
        let endpoints = run(
            &store,
            &CodeQuery {
                impact_route_kind: Some("endpoint".into()),
                ..q
            },
        );
        assert_eq!(endpoints["direct_total"], 2);
        assert_eq!(names(&store, &endpoints), BTreeSet::from(["main".into()]));
    }
    #[test]
    fn literal_identity_disambiguation_precedes_path_kind_and_stale_bridges_are_excluded() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("scope.duckdb")).unwrap();
        put(
            &store,
            "a",
            "/p/view.html",
            "p",
            "%{key}<a href='/items'>Link</a>",
        );
        put(
            &store,
            "site",
            "/p/site.html",
            "p",
            "<iframe src='view.html'></iframe>",
        );
        put(
            &store,
            "b",
            "/q/view.html",
            "q",
            "%{key}<a href='/items'>Link</a>",
        );
        store.index_code().unwrap();
        let ambiguous = run(
            &store,
            &CodeQuery {
                impact_key: Some("key".into()),
                path: Some("/p/view.html".into()),
                ..Default::default()
            },
        );
        assert_eq!(ambiguous["needs_disambiguation"], true);
        assert_eq!(ambiguous["target_candidates_total"], 2);
        let ambiguous = run(
            &store,
            &CodeQuery {
                impact_route: Some("/items".into()),
                impact_route_kind: Some("registration".into()),
                path: Some("/p/view.html".into()),
                ..Default::default()
            },
        );
        assert_eq!(ambiguous["needs_disambiguation"], true);
        put(&store, "site", "/p/site.html", "p", "changed");
        let q = CodeQuery {
            impact_key: Some("key".into()),
            impact_project: Some("p".into()),
            ..Default::default()
        };
        let stale = run(&store, &q);
        assert!(stale["file_impact"]["items"].as_array().unwrap().is_empty());
        assert_eq!(stale["stale_documents"].as_array().unwrap().len(), 1);
        store.index_code().unwrap();
        assert!(run(&store, &q)["file_impact"]["items"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn sql_impact_identity_includes_schema_only_candidates_before_source_and_access_filters() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("sql.duckdb")).unwrap();
        put(
            &store,
            "read",
            "/p/use.sql",
            "p",
            "SELECT * FROM public.logs;",
        );
        put(
            &store,
            "ddl",
            "/p/schema.sql",
            "p",
            "CREATE TABLE admin.logs(id INT);",
        );
        store.index_code().unwrap();
        let q = CodeQuery {
            impact_table: Some("logs".into()),
            impact_project: Some("p".into()),
            impact_table_mode: Some("write".into()),
            path: Some("/p/use.sql".into()),
            ..Default::default()
        };
        let r = run(&store, &q);
        assert_eq!(r["needs_disambiguation"], true);
        assert_eq!(r["table_candidates_total"], 2);
        let r = run(
            &store,
            &CodeQuery {
                impact_table: Some("admin.logs".into()),
                ..q
            },
        );
        assert_eq!(r["target_observed"], true);
        assert_eq!(r["direct_total"], 0);
    }
    #[test]
    fn mixed_targets_and_misplaced_modes_are_rejected_without_traversal() {
        for q in [
            CodeQuery {
                impact_key: Some("key".into()),
                symbol: Some("x".into()),
                ..Default::default()
            },
            CodeQuery {
                impact_key: Some("key".into()),
                impact_table: Some("users".into()),
                ..Default::default()
            },
            CodeQuery {
                impact_route: Some("/x".into()),
                impact_file: Some("x".into()),
                ..Default::default()
            },
            CodeQuery {
                impact_route_kind: Some("endpoint".into()),
                ..Default::default()
            },
            CodeQuery {
                impact_key: Some("key".into()),
                impact_relations: Some("unknown".into()),
                ..Default::default()
            },
        ] {
            assert!(validate(&q).is_some());
        }
        assert!(validate(&CodeQuery {
            impact_file: Some("/p/a".into()),
            impact_document_id: Some("a".into()),
            ..Default::default()
        })
        .is_none());
    }
}

#[cfg(test)]
mod extra_tests {
    use super::*;
    use crate::models::Document;
    fn put(store: &Store, id: &str, path: &str, body: &str) {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("doc://{id}"),
                source_file: Some(path.into()),
                wing: Some("p".into()),
                content: body.into(),
                ..Default::default()
            })
            .unwrap();
    }
    #[test]
    fn every_owner_remains_a_seed_when_direct_output_is_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("roots.duckdb")).unwrap();
        put(&store,"a","/p/a.php","<?php function one(){Translate('key');} function two(){Translate('key');} function use_one(){one();} function use_two(){two();}");
        store.index_code().unwrap();
        let r = store.code_snapshot().unwrap().query(
            "impact",
            &CodeQuery {
                impact_key: Some("key".into()),
                impact_project: Some("p".into()),
                limit: Some(1),
                ..Default::default()
            },
        );
        assert_eq!(r["direct_total"], 2);
        assert_eq!(r["direct_owner_symbols"], 2);
        assert_eq!(r["code_impact"]["matched_symbols"], 2);
        assert_eq!(r["direct_truncated"], true);
    }
    #[test]
    fn parse_error_callers_cannot_bridge_literal_impact_to_otherwise_valid_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("errors.duckdb")).unwrap();
        put(
            &store,
            "client",
            "/p/client.js",
            "export function load(){fetch('/items');}",
        );
        put(
            &store,
            "broken",
            "/p/broken.js",
            "import {load} from './client.js'; export function bridge(){load();}\nconst = ;",
        );
        put(
            &store,
            "outer",
            "/p/outer.js",
            "import {bridge} from './broken.js'; export function outer(){bridge();}",
        );
        store.index_code().unwrap();
        let r = store.code_snapshot().unwrap().query(
            "impact",
            &CodeQuery {
                impact_route: Some("/items".into()),
                impact_project: Some("p".into()),
                ..Default::default()
            },
        );
        assert_eq!(r["parse_errors"].as_array().unwrap().len(), 1, "{r}");
        assert!(r["items"].as_array().unwrap().is_empty(), "{r}");
        assert!(
            r["file_impact"]["items"].as_array().unwrap().is_empty(),
            "{r}"
        );
    }
}

#[cfg(test)]
mod selector_tests {
    use super::*;
    use crate::models::Document;
    #[test]
    fn long_captured_endpoint_target_roundtrips_references_symbol_and_impact() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("long-url.duckdb")).unwrap();
        let url = format!("/{}", "x".repeat(3000));
        store
            .upsert_document(&Document {
                id: "a".into(),
                uri: "doc://a".into(),
                source_file: Some("/p/a.js".into()),
                wing: Some("p".into()),
                content: format!("function load(){{fetch('{url}');}}"),
                ..Default::default()
            })
            .unwrap();
        store.index_code().unwrap();
        let references = store
            .code_references(&ReferenceQuery {
                route: Some(url.clone()),
                project: Some("p".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(references["total"], 1);
        let node = store
            .code_symbols(&SymbolQuery {
                name: Some(format!("route:{url}")),
                project: Some("p".into()),
                ..Default::default()
            })
            .unwrap();
        let q: CodeQuery =
            serde_json::from_value(node["items"][0]["impact_target"].clone()).unwrap();
        let impact = store.code_snapshot().unwrap().query("impact", &q);
        assert_eq!(impact["direct_total"], 1);
    }
}
