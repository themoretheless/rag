//! Candidate availability on changed PHP classes/traits; these are never call edges.
use super::*;
pub(super) struct Hosts<'a> {
    pub paths: BTreeMap<&'a str, Value>,
    pub work: usize,
    pub exhausted: bool,
    pub depth_limited: bool,
    pub truncated: bool,
    pub diagnostics: Value,
}
fn node(s: &Symbol) -> bool {
    php_names::supported(&s.path)
        && matches!(s.kind.as_str(), "class_declaration" | "trait_declaration")
}
pub(super) fn walk<'a>(
    snapshot: &'a CodeSnapshot,
    seeds: &[&'a Symbol],
    known: &BTreeMap<&'a str, &'a Symbol>,
    hosts: &HashSet<&str>,
    depth: usize,
    budget: usize,
) -> Hosts<'a> {
    let roots: Vec<_> = seeds.iter().copied().filter(|s| node(s)).collect();
    let mut paths = BTreeMap::new();
    for root in &roots {
        if hosts.contains(root.id.as_str()) {
            paths.insert(root.id.as_str(),json!({"steps":[],"seed_symbol_id":root.id,"seed_context":null,"complete_to_seed":true}));
        }
    }
    if roots.is_empty() {
        return Hosts {
            paths,
            work: 0,
            exhausted: false,
            depth_limited: false,
            truncated: false,
            diagnostics: json!({"applied":false,"candidate_availability_only":true,"calls_inferred":false}),
        };
    }
    let mut incoming: BTreeMap<&str, Vec<Incoming<'_>>> = BTreeMap::new();
    let mut work = 0;
    let mut exhausted = false;
    let project = |id: &str| {
        known
            .get(id)
            .and_then(|s| snapshot.document_scopes.get(&s.document_id))
            .map(|d| d.project.as_deref())
    };
    for edge in &snapshot.inheritance {
        if work == budget {
            exhausted = true;
            break;
        }
        work += 1;
        let source = edge.derived.as_str();
        let Some(source_symbol) = known.get(source).filter(|s| node(s)) else {
            continue;
        };
        if edge.path != source_symbol.path
            || !matches!(
                edge.relation.as_str(),
                "extends" | "php_trait_use_candidate"
            )
        {
            continue;
        }
        for target in &edge.candidates {
            if work == budget {
                exhausted = true;
                break;
            }
            work += 1;
            let Some(target_symbol) = known.get(target.as_str()).filter(|s| node(s)) else {
                continue;
            };
            if project(source) != project(target) {
                continue;
            }
            let kinds = if edge.relation == "extends" {
                source_symbol.kind == "class_declaration"
                    && target_symbol.kind == "class_declaration"
            } else {
                target_symbol.kind == "trait_declaration"
            };
            if !kinds {
                continue;
            }
            incoming.entry(target).or_default().push(Incoming {
                source,
                evidence: impact::Evidence::Inheritance(edge),
            });
        }
        if exhausted {
            break;
        }
    }
    for edges in incoming.values_mut() {
        edges.sort_by_key(|e| (e.source, e.evidence.location()));
    }
    let known_ids = known.keys().copied().collect();
    let projects = known
        .keys()
        .map(|id| (*id, project(id).flatten()))
        .collect();
    let walked = impact::context_walk::walk(
        &roots,
        &incoming,
        &known_ids,
        &projects,
        impact::context_walk::Options {
            limit: usize::MAX,
            depth_limit: depth,
            include_paths: true,
            work_limit: budget.saturating_sub(work),
            deduplicate_symbols: true,
            output_symbols: Some(hosts),
        },
    );
    work += walked.diagnostics["work_steps"].as_u64().unwrap_or(0) as usize;
    exhausted |= walked.diagnostics["work_limited"] == true;
    for item in &walked.items {
        let Some(id) = item["symbol_id"].as_str() else {
            continue;
        };
        // Borrow stable symbol IDs, rather than the temporary JSON item.
        let Some((id, _)) = known.get_key_value(id) else {
            continue;
        };
        let mut path = item["state_path"].clone();
        for step in path["steps"].as_array_mut().into_iter().flatten() {
            if let Some(s) = step["source_symbol_id"]
                .as_str()
                .and_then(|id| known.get(id))
            {
                step["evidence"]["source_provenance"] = json!({"document_id":s.document_id,"path":s.path,"revision":s.revision,"content_hash":s.content_hash,"line":step["evidence"]["line"]});
            }
        }
        paths.insert(*id, path);
    }
    let mut diagnostics = walked.diagnostics;
    diagnostics["applied"] = true.into();
    diagnostics["candidate_availability_only"] = true.into();
    diagnostics["calls_inferred"] = false.into();
    diagnostics["roots"] = roots.len().into();
    diagnostics["work_including_graph_candidates"] = work.into();
    Hosts {
        paths,
        work,
        exhausted,
        depth_limited: walked.depth_limited,
        truncated: walked.truncated || exhausted,
        diagnostics,
    }
}
pub(super) fn attach(
    entry: &analysis::EntryPoint,
    body: &Symbol,
    host: &Symbol,
    path: &Value,
) -> Value {
    let context = entry
        .test_context
        .as_ref()
        .expect("qualified PHP host context");
    let source_context = context
        .trait_body
        .then_some(context.lexical_owner_symbol_id.as_str());
    let association = json!({"source_symbol_id":body.id,"target_symbol_id":host.id,"source_context":source_context,"target_context":null,"relation":"phpunit_host_candidate","evidence":{"kind":"phpunit_host_candidate","body_document_id":body.document_id,"body_path":body.path,"body_revision":body.revision,"body_content_hash":body.content_hash,"body_start_byte":body.start_byte,"host_symbol_id":host.id,"host_document_id":host.document_id,"host_content_hash":host.content_hash,"effective_method":context.effective_method,"test_context":context,"provenance":"derived_candidate_inventory","call_edge":false,"runtime_registration_verified":false}});
    let mut result = path.clone();
    let mut steps = vec![association];
    steps.extend(path["steps"].as_array().into_iter().flatten().cloned());
    result["steps"] = steps.into();
    result["test_host_dependency"] = true.into();
    result["captured_trait_frames_matched"] = true.into();
    result["runtime_context_correlation_proven"] = false.into();
    result
}
