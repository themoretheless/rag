use super::*;
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct State {
    node: String,
    stack: Vec<String>,
}
#[derive(Clone)]
struct Edge {
    source: String,
    target: String,
    kind: String,
    call: Option<String>,
    evidence: Value,
}
fn provenance(doc: &DocumentFacts) -> Value {
    json!({"document_id":doc.document_id,"path":doc.path,"project":doc.project,"revision":doc.revision,"content_hash":doc.content_hash})
}
fn step(stack: &[String], edge: &Edge, reversed: bool) -> Option<Vec<String>> {
    let mut next = stack.to_vec();
    if let Some(call) = &edge.call {
        let enter = (edge.kind == "call_argument" && !reversed)
            || (edge.kind == "return_to_call" && reversed);
        if enter {
            if next.len() >= 16 {
                return None;
            }
            next.push(call.clone());
        } else if next.last().is_some_and(|top| top != call) {
            return None;
        } else {
            next.pop();
        }
    }
    Some(next)
}
pub(super) fn query(snapshot: &CodeSnapshot, q: &DataSliceQuery) -> Value {
    let limit = q.limit.unwrap_or(200).clamp(1, 500);
    let depth = q.depth.unwrap_or(12).clamp(1, 30);
    let budget = q.max_work.unwrap_or(50000).clamp(1, 200000);
    let direction = q.direction.as_deref().unwrap_or("back");
    let mut selected: Vec<_> = snapshot
        .symbols
        .iter()
        .filter(|s| {
            q.path.as_ref().is_none_or(|p| p == &s.path)
                && (s.id == q.symbol || s.name == q.symbol || s.qualified_name == q.symbol)
        })
        .collect();
    if selected.iter().any(|s| s.id == q.symbol) {
        selected.retain(|s| s.id == q.symbol);
    }
    selected.sort_by(|a, b| a.id.cmp(&b.id));
    let mut report = json!({"status":"seed_unavailable","items":[],"edges":[],"seeds":[],"selectors":selected.iter().take(20).collect::<Vec<_>>(),"selector_total":selected.len(),"index_rev":snapshot.index_revision,"direction":direction,"limits":{"nodes":limit,"depth":depth,"work":budget,"call_stack":16,"states":65536,"edges":2000},"semantics":"python_branching_loop_value_dependency_candidates","structural_call_slice":false,"full_program_slice":false,"control_flow_modeled":false,"finally_completion_paths":true,"protected_expression_error_prefixes":true,"exception_handler_paths":true,"exception_type_matching_proven":false,"exception_filter_evaluation_proven":false,"full_exception_semantics":false,"loop_carried_dependencies":true,"loop_iteration_feasibility_proven":false,"termination_proven":false,"branch_control_dependencies":true,"branch_path_feasibility_proven":false,"alias_analysis_complete":false,"runtime_path_proven":false,"absence_proven":false,"filesystem_freshness_checked":false,"stale_documents":snapshot.stale_documents,"unindexed_documents":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors});
    if selected.len() != 1 {
        if selected.len() > 1 {
            report["status"] = "ambiguous_function".into();
        }
        return report;
    }
    let symbol = selected[0];
    if snapshot.parse_errors.contains(&symbol.document_id)
        || snapshot.stale_documents.contains(&symbol.document_id)
        || snapshot.unindexed_documents.contains(&symbol.document_id)
    {
        report["status"] = "invalid_source".into();
        return report;
    }
    let Some(scope) = snapshot.document_scopes.get(&symbol.document_id) else {
        return report;
    };
    let docs: Vec<_> = snapshot
        .data_flow
        .documents
        .iter()
        .filter(|d| {
            d.project == scope.project
                && !snapshot.stale_documents.contains(&d.document_id)
                && !snapshot.unindexed_documents.contains(&d.document_id)
                && !snapshot.parse_errors.contains(&d.document_id)
        })
        .collect();
    let functions: BTreeMap<_, _> = docs
        .iter()
        .flat_map(|doc| {
            doc.facts
                .functions
                .iter()
                .map(move |f| (f.symbol.as_str(), (*doc, f)))
        })
        .collect();
    let Some((_, function)) = functions.get(symbol.id.as_str()) else {
        report["status"] = "unsupported_language_or_capture".into();
        return report;
    };
    if !function.qualified {
        report["status"] = "unsupported_function".into();
        report["reason"] = json!(function.reason);
        return report;
    }
    let variable_binding = q.variable.as_deref().map(binding_name);
    let mut seeds: Vec<_> = if q.variable.is_some() {
        function
            .nodes
            .iter()
            .filter(|n| {
                matches!(n.kind.as_str(), "definition" | "parameter")
                    && n.binding_name.as_ref() == variable_binding.as_ref()
                    && q.start_byte.is_none_or(|p| p == n.start_byte)
            })
            .map(|n| n.id.clone())
            .collect()
    } else {
        if direction == "forward" {
            function.parameters.clone()
        } else if direction == "both" {
            function
                .parameters
                .iter()
                .chain(&function.returns)
                .cloned()
                .collect()
        } else {
            function.returns.clone()
        }
    };
    seeds.sort();
    seeds.dedup();
    let seed_ids: HashSet<_> = seeds.iter().collect();
    let selected_occurrences: BTreeSet<_> = function
        .nodes
        .iter()
        .filter(|n| seed_ids.contains(&n.id))
        .map(|n| (n.start_byte, n.end_byte, n.kind.as_str()))
        .collect();
    if seeds.is_empty() || q.variable.is_some() && selected_occurrences.len() != 1 {
        report["status"] = if seeds.is_empty() {
            "seed_unavailable"
        } else {
            "ambiguous_local_definition"
        }
        .into();
        report["definition_candidates"] = json!(
            function
                .nodes
                .iter()
                .filter(|n| n.binding_name.as_ref() == variable_binding.as_ref()
                    && matches!(n.kind.as_str(), "definition" | "parameter"))
                .take(20)
                .collect::<Vec<_>>()
        );
        return report;
    }
    let mut nodes: BTreeMap<&str, (&NodeFact, &DocumentFacts)> = BTreeMap::new();
    let mut edges = Vec::new();
    let mut diagnostics = Vec::new();
    let mut work = 0;
    let mut exhausted = false;
    'nodes: for doc in &docs {
        for f in &doc.facts.functions {
            if !f.qualified {
                continue;
            }
            for node in &f.nodes {
                if work == budget {
                    exhausted = true;
                    break 'nodes;
                }
                work += 1;
                nodes.insert(node.id.as_str(), (node, *doc));
            }
        }
    }
    let mut bindings: BTreeMap<(&str, usize, &str), Vec<&CallEdge>> = BTreeMap::new();
    for edge in &snapshot.calls {
        if work == budget {
            exhausted = true;
            break;
        }
        work += 1;
        if let Some(caller) = edge
            .reference
            .caller
            .as_deref()
            .filter(|id| functions.contains_key(id))
        {
            bindings
                .entry((
                    caller,
                    edge.reference.start_byte,
                    edge.reference.expression.as_str(),
                ))
                .or_default()
                .push(edge);
        }
    }
    'edges: for doc in &docs {
        for f in &doc.facts.functions {
            if !f.qualified {
                continue;
            }
            for e in &f.edges {
                if work == budget {
                    exhausted = true;
                    break 'edges;
                }
                work += 1;
                if nodes.contains_key(e.source.as_str()) && nodes.contains_key(e.target.as_str()) {
                    edges.push(Edge {
                        source: e.source.clone(),
                        target: e.target.clone(),
                        kind: e.kind.clone(),
                        call: None,
                        evidence: provenance(doc),
                    });
                }
            }
            for call in &f.calls {
                if work == budget {
                    exhausted = true;
                    break 'edges;
                }
                work += 1;
                let matches = bindings
                    .get(&(f.symbol.as_str(), call.start_byte, call.expression.as_str()))
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let binding = matches.first().copied().filter(|e| {
                    matches.len() == 1
                        && e.resolution == "resolved_static"
                        && e.candidates.len() == 1
                });
                let callee = binding
                    .filter(|_| call.binding_qualified)
                    .and_then(|e| functions.get(e.candidates[0].as_str()))
                    .filter(|(_, f)| {
                        f.qualified
                            && !f.returns.is_empty()
                            && f.target_binding_qualified
                            && f.parameters.len() == call.arguments.len()
                    });
                let Some((target_doc, target)) = callee else {
                    diagnostics.push(json!({"kind":"call_boundary_unverified","function_id":f.symbol,"call_start_byte":call.start_byte,"source":provenance(doc),"expression":call.expression}));
                    continue;
                };
                let frame = call.result.clone();
                for (source, target, kind) in call
                    .arguments
                    .iter()
                    .zip(&target.parameters)
                    .map(|(a, b)| (a, b, "call_argument"))
                    .chain(
                        target
                            .returns
                            .iter()
                            .map(|r| (r, &call.result, "return_to_call")),
                    )
                {
                    if work == budget {
                        exhausted = true;
                        break 'edges;
                    }
                    work += 1;
                    if !nodes.contains_key(source.as_str()) || !nodes.contains_key(target.as_str())
                    {
                        exhausted = true;
                        continue;
                    }
                    edges.push(Edge{source:source.clone(),target:target.clone(),kind:kind.into(),call:Some(frame.clone()),evidence:json!({"call_source":provenance(doc),"callee_source":provenance(target_doc),"binding":binding.unwrap(),"runtime_binding_verified":false})});
                }
            }
        }
    }
    let mut incoming: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    let mut outgoing: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    edges.sort_by(|a, b| {
        (&a.source, &a.target, &a.kind, &a.call).cmp(&(&b.source, &b.target, &b.kind, &b.call))
    });
    for (i, e) in edges.iter().enumerate() {
        incoming.entry(&e.target).or_default().push(i);
        outgoing.entry(&e.source).or_default().push(i);
    }
    let mut visited: BTreeSet<_> = seeds
        .iter()
        .filter(|id| nodes.contains_key(id.as_str()))
        .map(|id| State {
            node: id.clone(),
            stack: Vec::new(),
        })
        .collect();
    let roots = visited.clone();
    let mut frontier = visited.clone();
    let mut predecessors: BTreeMap<State, (State, usize, bool)> = BTreeMap::new();
    let mut reported: BTreeMap<String, State> = visited
        .iter()
        .take(limit)
        .map(|s| (s.node.clone(), s.clone()))
        .collect();
    let mut result_limited = roots.len() > limit;
    let mut depth_limited = false;
    let mut frame_limited = false;
    let mut state_limited = false;
    let mut edge_limited = false;
    let mut traversed = BTreeMap::new();
    for level in 0..=depth {
        let mut next = BTreeSet::new();
        for current in &frontier {
            let neighbors = incoming
                .get(current.node.as_str())
                .into_iter()
                .flatten()
                .map(|i| (*i, true))
                .filter(|_| direction != "forward")
                .chain(
                    outgoing
                        .get(current.node.as_str())
                        .into_iter()
                        .flatten()
                        .map(|i| (*i, false))
                        .filter(|_| direction != "back"),
                );
            for (i, reversed) in neighbors {
                if work == budget {
                    exhausted = true;
                    break;
                }
                work += 1;
                let edge = &edges[i];
                let Some(stack) = step(&current.stack, edge, reversed) else {
                    frame_limited |= current.stack.len() >= 16;
                    continue;
                };
                let target = if reversed { &edge.source } else { &edge.target };
                let state = State {
                    node: target.clone(),
                    stack,
                };
                if visited.contains(&state) {
                    if level < depth
                        && reported.contains_key(&current.node)
                        && reported.contains_key(target)
                    {
                        if traversed.len() < 2000 || traversed.contains_key(&i) {
                            traversed
                                .entry(i)
                                .or_insert((current.clone(), state, reversed));
                        } else {
                            edge_limited = true;
                        }
                    }
                    continue;
                }
                if level == depth {
                    depth_limited = true;
                    continue;
                }
                if visited.len() >= 65536 {
                    state_limited = true;
                    continue;
                }
                if !reported.contains_key(target) && reported.len() >= limit {
                    result_limited = true;
                    continue;
                }
                predecessors.insert(state.clone(), (current.clone(), i, reversed));
                visited.insert(state.clone());
                next.insert(state.clone());
                reported.entry(target.clone()).or_insert(state.clone());
                if reported.contains_key(&current.node) {
                    if traversed.len() < 2000 || traversed.contains_key(&i) {
                        traversed
                            .entry(i)
                            .or_insert((current.clone(), state, reversed));
                    } else {
                        edge_limited = true;
                    }
                }
            }
            if exhausted {
                break;
            }
        }
        if exhausted || next.is_empty() {
            break;
        }
        frontier = next;
    }
    let items:Vec<_>=reported.values().map(|state|{
        let (node,doc)=nodes[state.node.as_str()];let mut at=state;let mut witness=Vec::new();
        while !roots.contains(at) {let Some((previous,i,reversed))=predecessors.get(at) else {break;};let e=&edges[*i];witness.push(json!({"source":e.source,"target":e.target,"relation":e.kind,"reversed":reversed,"call_context":e.call,"from_stack":previous.stack,"to_stack":at.stack,"evidence":e.evidence}));at=previous;}
        witness.reverse();json!({"node":node,"source":provenance(doc),"call_stack":state.stack,"witness":{"seed_node_id":at.node,"complete_to_seed":roots.contains(at),"steps":witness,"call_contexts_matched":true,"runtime_path_proven":false}})
    }).collect();
    report["status"] = if roots.is_empty() {
        "graph_budget_exhausted"
    } else {
        "data_slice"
    }
    .into();
    report["items"] = items.into();
    report["edges"]=traversed.into_iter().map(|(i,(from,to,reversed))|{let e=&edges[i];json!({"source":e.source,"target":e.target,"relation":e.kind,"call_context":e.call,"evidence":e.evidence,"transition":{"from_stack":from.stack,"to_stack":to.stack,"reversed":reversed,"source_state_reachable":true,"runtime_path_proven":false}})}).collect::<Vec<_>>().into();
    report["edge_scope"] = "one_checked_reachable_context_per_physical_edge".into();
    report["edge_limit_reached"] = edge_limited.into();
    report["seeds"] = seeds.into();
    report["work"] = work.into();
    report["work_exhausted"] = exhausted.into();
    report["result_limit_reached"] = result_limited.into();
    report["depth_limit_reached"] = depth_limited.into();
    report["call_context_limit_reached"] = frame_limited.into();
    report["state_limit_reached"] = state_limited.into();
    report["states_visited"] = visited.len().into();
    report["graph_nodes_examined"] = nodes.len().into();
    report["graph_edges_examined"] = edges.len().into();
    report["call_boundary_diagnostics_total"] = diagnostics.len().into();
    report["call_boundary_diagnostics"] =
        diagnostics.into_iter().take(50).collect::<Vec<_>>().into();
    report["capture_truncated"] = docs.iter().any(|d| !d.facts.complete).into();
    report["edge_union_is_single_path"] = false.into();
    report["qualified_subset_traversal_complete"] = (!roots.is_empty()
        && !exhausted
        && !result_limited
        && !depth_limited
        && !frame_limited
        && !state_limited
        && !edge_limited
        && report["call_boundary_diagnostics_total"] == 0
        && !docs.iter().any(|d| !d.facts.complete))
    .into();
    report["capture_format"] = 7.into();
    report["seed_name_binding_qualified"] = function.target_binding_qualified.into();
    report["function_coverage"] = json!({"captured":functions.len(),"qualified":functions.values().filter(|(_,f)|f.qualified).count(),"unsupported":functions.values().filter(|(_,f)|!f.qualified).count(),"exhaustive":false});
    report
}
