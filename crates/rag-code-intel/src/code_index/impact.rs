//! Bounded incoming-dependency traversal. Candidate relations never become calls.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy)]
pub(super) enum Evidence<'a> {
    Reference(&'a CallEdge),
    FileImport(&'a file_imports::FileImportEdge),
    Resource(&'a file_resources::ResourceEdge),
    Inheritance(&'a inheritance::InheritanceEdge),
    Override(&'a inheritance::OverrideEdge),
}
impl Evidence<'_> {
    pub(super) fn value(self) -> serde_json::Value {
        match self {
            Self::Resource(e) => serde_json::to_value(e).unwrap(),
            Self::FileImport(e) => serde_json::to_value(e).unwrap(),
            Self::Reference(e) => serde_json::to_value(e).unwrap(),
            Self::Inheritance(e) => serde_json::to_value(e).unwrap(),
            Self::Override(e) => serde_json::to_value(e).unwrap(),
        }
    }
    pub(super) fn location(self) -> (&'static str, usize) {
        match self {
            Self::Reference(e) => (
                if e.reference.is_call {
                    "call"
                } else {
                    "bound_reference"
                },
                e.reference.start_byte,
            ),
            Self::Resource(e) => ("file_resource_candidate", e.start_byte),
            Self::FileImport(e) => ("file_import_candidate", e.start_byte),
            Self::Inheritance(e) => ("inheritance_candidate", e.line),
            Self::Override(e) => ("override_candidate", e.line),
        }
    }
}
#[derive(Clone, Copy)]
struct Incoming<'a> {
    source: &'a str,
    evidence: Evidence<'a>,
}
fn insert<'a>(
    incoming: &mut BTreeMap<&'a str, Vec<Incoming<'a>>>,
    known: &HashSet<&str>,
    projects: &HashMap<&str, Option<&str>>,
    source: &'a str,
    targets: &'a [String],
    evidence: Evidence<'a>,
) {
    if !known.contains(source) {
        return;
    }
    for target in targets
        .iter()
        .map(String::as_str)
        .filter(|id| known.contains(id) && projects.get(source) == projects.get(id))
    {
        incoming
            .entry(target)
            .or_default()
            .push(Incoming { source, evidence });
    }
}
#[derive(Serialize)]
struct FileImpact<'a> {
    document_id: &'a str,
    path: &'a str,
    project: Option<&'a str>,
    revision: i64,
    content_hash: &'a str,
    symbol_count: usize,
    min_depth: usize,
    relation_counts: BTreeMap<String, usize>,
}
fn affected_files<'a>(
    snapshot: &'a CodeSnapshot,
    items: &[serde_json::Value],
) -> Vec<FileImpact<'a>> {
    let symbols: HashMap<_, _> = snapshot
        .symbols
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();
    let mut files: BTreeMap<&str, FileImpact<'_>> = BTreeMap::new();
    for item in items {
        let Some(symbol) = item["symbol_id"].as_str().and_then(|id| symbols.get(id)) else {
            continue;
        };
        let depth = item["depth"].as_u64().unwrap_or(0) as usize;
        let file = files
            .entry(&symbol.document_id)
            .or_insert_with(|| FileImpact {
                document_id: &symbol.document_id,
                path: &symbol.path,
                project: snapshot
                    .document_scopes
                    .get(&symbol.document_id)
                    .and_then(|s| s.project.as_deref()),
                revision: symbol.revision,
                content_hash: &symbol.content_hash,
                symbol_count: 0,
                min_depth: depth,
                relation_counts: BTreeMap::new(),
            });
        file.symbol_count += 1;
        file.min_depth = file.min_depth.min(depth);
        *file
            .relation_counts
            .entry(item["via_kind"].as_str().unwrap_or("unknown").into())
            .or_default() += 1;
    }
    let mut files: Vec<_> = files.into_values().collect();
    files.sort_by(|a, b| {
        b.symbol_count
            .cmp(&a.symbol_count)
            .then_with(|| (&a.path, &a.document_id).cmp(&(&b.path, &b.document_id)))
    });
    files
}
pub(super) fn query(
    snapshot: &CodeSnapshot,
    q: &CodeQuery,
    selected: &[&Symbol],
    limit: usize,
) -> serde_json::Value {
    let mode = q.impact_relations.as_deref().unwrap_or("calls");
    if !matches!(mode, "calls" | "references" | "all") {
        return serde_json::json!({"error":"impact_relations must be calls, references or all","items":[],"truncated":false});
    }
    let excluded: HashSet<_> = snapshot
        .stale_documents
        .iter()
        .chain(&snapshot.unindexed_documents)
        .chain(&snapshot.parse_errors)
        .map(String::as_str)
        .collect();
    let known: HashSet<_> = snapshot
        .symbols
        .iter()
        .filter(|s| !excluded.contains(s.document_id.as_str()))
        .map(|s| s.id.as_str())
        .collect();
    let selected_roots_excluded = selected
        .iter()
        .filter(|s| !known.contains(s.id.as_str()))
        .count();
    let selected: Vec<_> = selected
        .iter()
        .copied()
        .filter(|s| known.contains(s.id.as_str()))
        .collect();
    let projects: HashMap<_, _> = snapshot
        .symbols
        .iter()
        .map(|s| {
            (
                s.id.as_str(),
                snapshot
                    .document_scopes
                    .get(&s.document_id)
                    .and_then(|d| d.project.as_deref()),
            )
        })
        .collect();
    let mut incoming: BTreeMap<&str, Vec<Incoming<'_>>> = BTreeMap::new();
    for edge in if mode == "calls" {
        &snapshot.calls
    } else {
        &snapshot.bound_references
    } {
        if let Some(source) = edge.reference.caller.as_deref() {
            insert(
                &mut incoming,
                &known,
                &projects,
                source,
                &edge.candidates,
                Evidence::Reference(edge),
            );
        }
    }
    if mode == "all" {
        for e in &snapshot.inheritance {
            insert(
                &mut incoming,
                &known,
                &projects,
                &e.derived,
                &e.candidates,
                Evidence::Inheritance(e),
            );
        }
        for e in &snapshot.overrides {
            insert(
                &mut incoming,
                &known,
                &projects,
                &e.derived,
                &e.candidates,
                Evidence::Override(e),
            );
        }
    }
    for edges in incoming.values_mut() {
        edges.sort_by(|a, b| {
            (a.source, a.evidence.location()).cmp(&(b.source, b.evidence.location()))
        });
    }
    let mut visited: BTreeSet<_> = selected.iter().map(|s| s.id.as_str()).collect();
    let mut frontier = visited.clone();
    let mut items = Vec::new();
    let mut truncated = false;
    for depth in 1..=q.depth.unwrap_or(3).clamp(1, 10) {
        let mut next = BTreeSet::new();
        for target in &frontier {
            for edge in incoming.get(target).into_iter().flatten() {
                if visited.contains(edge.source) {
                    continue;
                }
                if items.len() >= limit {
                    truncated = true;
                    continue;
                }
                visited.insert(edge.source);
                next.insert(edge.source);
                items.push(serde_json::json!({"symbol_id":edge.source,"depth":depth,"via_kind":edge.evidence.location().0,"via_target":target,"via":edge.evidence.value()}));
            }
        }
        if next.is_empty() {
            frontier.clear();
            break;
        }
        frontier = next;
    }
    let depth_limited = frontier.iter().any(|target| {
        incoming
            .get(target)
            .into_iter()
            .flatten()
            .any(|e| !visited.contains(e.source))
    });
    let files = affected_files(snapshot, &items);
    serde_json::json!({"affected_files":files,"affected_files_total":files.len(),"file_summary_scope":"returned_symbols","file_summary_complete_for_returned_symbols":true,"program_impact_complete":false,"items":items,"truncated":truncated,"depth_limited":depth_limited,"traversal_complete":!truncated&&!depth_limited,
        "matched_symbols":selected.len(),"selected_roots_excluded":selected_roots_excluded,"invalid_input_documents_excluded":true,"excluded_input_documents_total":excluded.len(),"impact_relations":mode,"resolution":"conservative_static_bindings","runtime_impact_proven":false,"absence_proven":false,
        "path_scope":"selected_seed_symbols","traversal_scope":"fresh_bound_symbol_dependency_graph","module_level_references_traversed":false,"witness_policy":"one_shortest_discovered_relation_per_symbol",
        "stale_documents":snapshot.stale_documents,"unindexed_documents":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors})
}

/// SQL usage evidence is retained even when a raw statement has no owning symbol.
pub(super) fn table_query(
    snapshot: &CodeSnapshot,
    q: &CodeQuery,
    limit: usize,
) -> serde_json::Value {
    if q.symbol.is_some() {
        return serde_json::json!({"error":"impact_table cannot be combined with symbol","items":[]});
    }
    let mode = q.impact_table_mode.as_deref().unwrap_or("all");
    if !matches!(mode, "all" | "read" | "write") {
        return serde_json::json!({"error":"impact_table_mode must be all, read or write","items":[]});
    }
    let table = match sql::canonical(q.impact_table.as_deref().unwrap_or(""), false) {
        Ok(value) => value,
        Err(e) => return serde_json::json!({"error":e.to_string(),"items":[]}),
    };
    let excluded: HashSet<_> = snapshot
        .stale_documents
        .iter()
        .chain(&snapshot.unindexed_documents)
        .chain(&snapshot.parse_errors)
        .map(String::as_str)
        .collect();
    let uses: Vec<_> = snapshot
        .sql
        .usages
        .iter()
        .filter(|u| {
            !excluded.contains(u.evidence.document_id.as_str())
                && q.impact_project
                    .as_ref()
                    .is_none_or(|p| u.evidence.project.as_ref() == Some(p))
                && sql::table_matches(&u.table, &table)
        })
        .collect();
    let diagnostics: Vec<_> = snapshot
        .sql
        .diagnostics
        .iter()
        .filter(|d| {
            q.impact_project
                .as_ref()
                .is_none_or(|p| d["evidence"]["project"].as_str() == Some(p.as_str()))
                && q.path
                    .as_ref()
                    .is_none_or(|p| d["evidence"]["path"].as_str() == Some(p.as_str()))
        })
        .collect();
    let diagnostics = serde_json::json!({"items":diagnostics.iter().take(limit).collect::<Vec<_>>(),"total":diagnostics.len(),"truncated":diagnostics.len()>limit,"scope":"project_path_not_table_specific"});
    let mut identities: BTreeSet<_> = uses
        .iter()
        .map(|u| (&u.evidence.project, &u.table))
        .collect();
    identities.extend(
        snapshot
            .sql
            .tables
            .iter()
            .filter(|t| {
                q.impact_project
                    .as_ref()
                    .is_none_or(|p| t.project.as_ref() == Some(p))
                    && sql::table_matches(&t.name, &table)
            })
            .map(|t| (&t.project, &t.name)),
    );
    if identities.len() > 1 {
        return serde_json::json!({"target_kind":"table","target":table,"needs_disambiguation":true,"sql_diagnostics":diagnostics,"stale_documents":snapshot.stale_documents,"unindexed_documents":snapshot.unindexed_documents,"table_candidates":identities.iter().take(limit).map(|(project,table)|serde_json::json!({"project":project,"table":table})).collect::<Vec<_>>(),"table_candidates_total":identities.len(),"table_candidates_truncated":identities.len()>limit,"items":[],"runtime_impact_proven":false,"absence_proven":false});
    }
    let mut uses: Vec<_> = uses
        .into_iter()
        .filter(|u| {
            (mode == "all" || u.mode == mode)
                && q.path.as_ref().is_none_or(|p| p == &u.evidence.path)
        })
        .collect();
    uses.sort_by(|a, b| {
        (
            &a.evidence.document_id,
            a.evidence.line,
            &a.mode,
            &a.operation,
            &a.table,
        )
            .cmp(&(
                &b.evidence.document_id,
                b.evidence.line,
                &b.mode,
                &b.operation,
                &b.table,
            ))
    });
    let root_ids: HashSet<_> = uses.iter().filter_map(|u| u.symbol.as_deref()).collect();
    let roots: Vec<_> = snapshot
        .symbols
        .iter()
        .filter(|s| {
            root_ids.contains(s.id.as_str())
                && uses.iter().any(|u| {
                    u.symbol.as_deref() == Some(&s.id)
                        && u.evidence.document_id == s.document_id
                        && snapshot
                            .document_scopes
                            .get(&s.document_id)
                            .is_some_and(|d| d.project == u.evidence.project)
                })
        })
        .collect();
    let mut result = query(snapshot, q, &roots, limit);
    if result.get("error").is_some() {
        return result;
    }
    result["sql_diagnostics"] = diagnostics;
    result["direct_usage_found"] = serde_json::json!(!uses.is_empty());
    result["target_kind"] = serde_json::json!("table");
    result["target"] = serde_json::json!(table);
    result["target_observed"] = serde_json::json!(!identities.is_empty());
    result["needs_disambiguation"] = serde_json::json!(false);
    result["impact_project"] = serde_json::json!(q.impact_project);
    result["impact_table_mode"] = serde_json::json!(mode);
    result["direct_readers_writers"] =
        serde_json::json!(uses.iter().take(limit).collect::<Vec<_>>());
    result["direct_total"] = serde_json::json!(uses.len());
    result["direct_truncated"] = serde_json::json!(uses.len() > limit);
    result["direct_owner_symbols"] = serde_json::json!(roots.len());
    result["ownerless_direct_uses"] =
        serde_json::json!(uses.iter().filter(|u| u.symbol.is_none()).count());
    result["depth_origin"] = serde_json::json!("direct_owner_symbols_depth_zero");
    result["file_summary_scope"] = serde_json::json!("returned_indirect_symbols");
    result["direct_evidence_scope"] =
        serde_json::json!("fresh_static_sql_mentions_not_executed_queries");
    result
}

#[cfg(test)]
mod tests;
