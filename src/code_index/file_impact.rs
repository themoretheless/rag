//! Incoming file dependencies from fresh bound symbols/occurrences, never live IO.
use super::impact::Evidence;
use super::*;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Copy)]
struct Dependency<'a> {
    source: &'a str,
    evidence: Evidence<'a>,
}
fn add<'a>(
    edges: &mut BTreeMap<&'a str, Vec<Dependency<'a>>>,
    fresh: &BTreeSet<&str>,
    scopes: &HashMap<String, DocumentScope>,
    source: &'a str,
    target: &'a str,
    evidence: Evidence<'a>,
) {
    if source == target || !fresh.contains(source) || !fresh.contains(target) {
        return;
    }
    if scopes.get(source).map(|s| &s.project) != scopes.get(target).map(|s| &s.project) {
        return;
    }
    edges
        .entry(target)
        .or_default()
        .push(Dependency { source, evidence });
}
pub(super) fn query(snapshot: &CodeSnapshot, q: &CodeQuery, limit: usize) -> serde_json::Value {
    run(snapshot, q, limit, None)
}
pub(super) fn from_documents(
    snapshot: &CodeSnapshot,
    q: &CodeQuery,
    roots: &BTreeSet<String>,
    limit: usize,
) -> serde_json::Value {
    run(snapshot, q, limit, Some(roots))
}
fn run(
    snapshot: &CodeSnapshot,
    q: &CodeQuery,
    limit: usize,
    explicit_roots: Option<&BTreeSet<String>>,
) -> serde_json::Value {
    if explicit_roots.is_none()
        && (q.symbol.is_some()
            || q.path.is_some()
            || q.impact_table.is_some()
            || q.impact_table_mode.is_some())
    {
        return serde_json::json!({"error":"file impact cannot be combined with symbol, path or SQL table arguments","items":[]});
    }
    let mode = q.impact_relations.as_deref().unwrap_or("references");
    if !matches!(mode, "calls" | "references" | "all") {
        return serde_json::json!({"error":"impact_relations must be calls, references or all","items":[]});
    }
    let mut scopes = snapshot.document_scopes.clone();
    for target in &snapshot.file_resource_targets {
        scopes.insert(
            target.document_id.clone(),
            DocumentScope {
                path: target.path.clone(),
                project: target.project.clone(),
            },
        );
    }
    let mut targets: Vec<_> = scopes
        .iter()
        .filter(|(id, s)| {
            explicit_roots.is_none_or(|roots| roots.contains(id.as_str()))
                && q.impact_file.as_ref().is_none_or(|p| p == &s.path)
                && q.impact_document_id.as_ref().is_none_or(|p| p == *id)
                && q.impact_project
                    .as_ref()
                    .is_none_or(|p| s.project.as_ref() == Some(p))
        })
        .collect();
    targets.sort_by_key(|(id, _)| *id);
    if targets.len() > 1 && explicit_roots.is_none() {
        return serde_json::json!({"target_kind":"file","needs_disambiguation":true,"target_candidates":targets.iter().take(limit).map(|(id,s)|serde_json::json!({"document_id":id,"path":s.path,"project":s.project})).collect::<Vec<_>>(),"target_candidates_total":targets.len(),"target_candidates_truncated":targets.len()>limit,"items":[],"absence_proven":false});
    }
    let excluded: HashSet<_> = snapshot
        .stale_documents
        .iter()
        .chain(&snapshot.unindexed_documents)
        .chain(&snapshot.parse_errors)
        .chain(
            snapshot
                .file_resource_targets
                .iter()
                .filter(|t| !t.fresh())
                .map(|t| &t.document_id),
        )
        .map(String::as_str)
        .collect();
    let fresh: BTreeSet<_> = scopes
        .keys()
        .map(String::as_str)
        .filter(|id| !excluded.contains(id))
        .collect();
    let symbols: HashMap<_, _> = snapshot
        .symbols
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();
    let mut edges = BTreeMap::new();
    for e in if mode == "calls" {
        &snapshot.calls
    } else {
        &snapshot.bound_references
    } {
        for target in e
            .candidates
            .iter()
            .filter_map(|id| symbols.get(id.as_str()))
        {
            add(
                &mut edges,
                &fresh,
                &scopes,
                &e.reference.document_id,
                &target.document_id,
                Evidence::Reference(e),
            );
        }
    }
    if mode != "calls" {
        for e in &snapshot.file_resources {
            for target in &e.candidates {
                add(
                    &mut edges,
                    &fresh,
                    &scopes,
                    &e.document_id,
                    target,
                    Evidence::Resource(e),
                );
            }
        }
        for e in &snapshot.file_imports {
            for target in &e.candidates {
                add(
                    &mut edges,
                    &fresh,
                    &scopes,
                    &e.document_id,
                    target,
                    Evidence::FileImport(e),
                );
            }
        }
    }
    if mode == "all" {
        for e in &snapshot.inheritance {
            if let Some(source) = symbols.get(e.derived.as_str()) {
                for target in e
                    .candidates
                    .iter()
                    .filter_map(|id| symbols.get(id.as_str()))
                {
                    add(
                        &mut edges,
                        &fresh,
                        &scopes,
                        &source.document_id,
                        &target.document_id,
                        Evidence::Inheritance(e),
                    );
                }
            }
        }
        for e in &snapshot.overrides {
            if let Some(source) = symbols.get(e.derived.as_str()) {
                for target in e
                    .candidates
                    .iter()
                    .filter_map(|id| symbols.get(id.as_str()))
                {
                    add(
                        &mut edges,
                        &fresh,
                        &scopes,
                        &source.document_id,
                        &target.document_id,
                        Evidence::Override(e),
                    );
                }
            }
        }
    }
    for links in edges.values_mut() {
        links.sort_by_key(|e| (e.source, e.evidence.location()));
    }
    let mut visited: BTreeSet<_> = targets
        .iter()
        .map(|(id, _)| id.as_str())
        .filter(|id| fresh.contains(id))
        .collect();
    let available = !visited.is_empty();
    let mut frontier = visited.clone();
    let mut items = Vec::new();
    let mut truncated = false;
    for depth in 1..=q.depth.unwrap_or(3).clamp(1, 10) {
        let mut next = BTreeSet::new();
        for target in &frontier {
            for edge in edges.get(target).into_iter().flatten() {
                if visited.contains(edge.source) {
                    continue;
                }
                if items.len() >= limit {
                    truncated = true;
                    continue;
                }
                visited.insert(edge.source);
                next.insert(edge.source);
                let s = &scopes[edge.source];
                items.push(serde_json::json!({"document_id":edge.source,"path":s.path,"project":s.project,"depth":depth,"via_target_document_id":target,"via_kind":edge.evidence.location().0,"via":edge.evidence.value()}));
            }
        }
        if next.is_empty() {
            frontier.clear();
            break;
        }
        frontier = next;
    }
    let depth_limited = frontier.iter().any(|target| {
        edges
            .get(target)
            .into_iter()
            .flatten()
            .any(|e| !visited.contains(e.source))
    });
    let mut result = serde_json::json!({"target_kind":"file","result_kind":"files","impact_relations":mode,"target_found":!targets.is_empty(),"target_available":available,"needs_disambiguation":false,
        "target":targets.first().map(|(id,s)|serde_json::json!({"document_id":id,"path":s.path,"project":s.project,"captured_resource_target":snapshot.file_resource_targets.iter().find(|t|&t.document_id==*id)})),"items":items,"truncated":truncated,"depth_limited":depth_limited,
        "traversal_complete":available&&!truncated&&!depth_limited,"traversal_scope":"fresh_bound_cross_document_dependency_graph","ownerless_bound_occurrences_included":mode!="calls","static_file_import_candidates_included":mode!="calls","static_file_resource_candidates_included":mode!="calls",
        "resource_target_inventory_available":snapshot.resource_target_inventory_available,"resource_target_diagnostics":resource_targets::diagnostics(&snapshot.file_resource_targets,limit),
        "runtime_impact_proven":false,"absence_proven":false,"witness_policy":"one_shortest_discovered_relation_per_document",
        "stale_documents":snapshot.stale_documents,"unindexed_documents":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors});
    if explicit_roots.is_some() {
        result["target_kind"] = "file_set".into();
        result["target"] = serde_json::Value::Null;
        result["seed_documents"] = serde_json::json!(targets
            .iter()
            .take(limit)
            .map(|(id, s)| serde_json::json!({"document_id":id,"path":s.path,"project":s.project}))
            .collect::<Vec<_>>());
        result["seed_documents_total"] = targets.len().into();
        result["seed_documents_truncated"] = (targets.len() > limit).into();
        result["seed_policy"] = "all_explicit_direct_source_documents_before_result_limits".into();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(files: &[(&str, &str, &str)]) -> CodeSnapshot {
        let mut docs = HashMap::new();
        let mut out = CodeSnapshot::default();
        for (i, (path, project, source)) in files.iter().enumerate() {
            let mut d = parse(&i.to_string(), path, source, 1).unwrap();
            assert!(!d.parse_errors, "{path}");
            d.project = Some((*project).into());
            out.document_scopes.insert(
                i.to_string(),
                DocumentScope {
                    path: (*path).into(),
                    project: Some((*project).into()),
                },
            );
            out.symbols.extend(d.symbols.clone());
            out.references.extend(d.references.clone());
            docs.insert(i.to_string(), d);
        }
        out.bound_references = resolve::references(&out.symbols, &docs);
        out.calls = out
            .bound_references
            .iter()
            .filter(|e| e.reference.is_call)
            .cloned()
            .collect();
        out.inheritance = inheritance::edges(&out.symbols, &docs);
        out.overrides = inheritance::overrides(&out.symbols, &docs);
        out
    }
    fn fixture() -> CodeSnapshot {
        snapshot(&[
            ("/p/a.py", "p", "def work(): pass\n"),
            (
                "/p/b.py",
                "p",
                "from a import work\nregistered = work\ndef run(): pass\n",
            ),
            ("/p/c.py", "p", "from b import run\ndef outer(): run()\n"),
        ])
    }
    #[test]
    fn file_impact_cycles_exclude_the_seed_and_missing_targets_stay_unproven() {
        let snap = snapshot(&[
            ("/p/a.py", "p", "from c import outer\ndef work(): pass\n"),
            (
                "/p/b.py",
                "p",
                "from a import work\nregistered = work\ndef run(): pass\n",
            ),
            ("/p/c.py", "p", "from b import run\ndef outer(): run()\n"),
        ]);
        let mut q = CodeQuery {
            impact_file: Some("/p/a.py".into()),
            depth: Some(10),
            ..Default::default()
        };
        let r = snap.query("impact", &q);
        assert_eq!(r["items"].as_array().unwrap().len(), 2, "{r}");
        assert_eq!(r["traversal_complete"], true);
        q.impact_file = Some("/p/missing.py".into());
        let r = snap.query("impact", &q);
        assert_eq!(r["target_found"], false);
        assert_eq!(r["absence_proven"], false);
        assert_eq!(r["traversal_complete"], false);
        q.impact_relations = Some("invalid".into());
        assert!(snap.query("impact", &q)["error"].is_string());
    }
    #[test]
    fn file_impact_includes_ownerless_values_and_transitive_file_callers() {
        let snap = fixture();
        let mut q = CodeQuery {
            impact_file: Some("/p/a.py".into()),
            ..Default::default()
        };
        let r = snap.query("impact", &q);
        let items = r["items"].as_array().unwrap();
        assert_eq!(items.len(), 2, "{r}");
        assert_eq!(items[0]["path"], "/p/b.py");
        assert_eq!(items[1]["path"], "/p/c.py");
        assert_eq!(items[1]["depth"], 2);
        assert!(items[0]["via"]["reference"]["caller"].is_null());
        assert_eq!(r["ownerless_bound_occurrences_included"], true);
        q.impact_relations = Some("calls".into());
        assert!(snap.query("impact", &q)["items"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    #[test]
    fn file_impact_bounds_are_stable_and_stale_files_cannot_bridge_the_graph() {
        let mut snap = fixture();
        let mut q = CodeQuery {
            impact_file: Some("/p/a.py".into()),
            depth: Some(1),
            ..Default::default()
        };
        let r = snap.query("impact", &q);
        assert_eq!(r["depth_limited"], true);
        snap.bound_references.reverse();
        assert_eq!(snap.query("impact", &q), r);
        q.depth = Some(10);
        q.limit = Some(1);
        assert_eq!(snap.query("impact", &q)["truncated"], true);
        snap.stale_documents.push("1".into());
        assert!(snap.query("impact", &q)["items"]
            .as_array()
            .unwrap()
            .is_empty());
        snap.stale_documents.push("0".into());
        let r = snap.query("impact", &q);
        assert_eq!(r["target_found"], true);
        assert_eq!(r["target_available"], false);
        assert_eq!(r["traversal_complete"], false);
    }
    #[test]
    fn file_impact_disambiguates_document_project_identity_and_rejects_mixed_targets() {
        let snap = snapshot(&[
            ("/same/a.py", "alpha", "def work(): pass\n"),
            ("/same/a.py", "beta", "def work(): pass\n"),
            (
                "/same/b.py",
                "alpha",
                "from a import work\nregistered = work\n",
            ),
        ]);
        let mut q = CodeQuery {
            impact_file: Some("/same/a.py".into()),
            ..Default::default()
        };
        assert_eq!(snap.query("impact", &q)["needs_disambiguation"], true);
        q.impact_project = Some("alpha".into());
        assert_eq!(
            snap.query("impact", &q)["items"].as_array().unwrap().len(),
            1
        );
        q.impact_project = None;
        q.impact_document_id = Some("1".into());
        assert!(snap.query("impact", &q)["items"]
            .as_array()
            .unwrap()
            .is_empty());
        q.impact_table = Some("users".into());
        assert!(snap.query("impact", &q)["error"].is_string());
    }
}
