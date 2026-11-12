//! Position-independent frozen syntax/value candidate comparison, never semantic proof.
use super::*;
use crate::code_index::history::File;

#[cfg(test)]
mod tests;

const BUDGET: usize = 200000;
struct Budget {
    used: usize,
}
impl Budget {
    fn spend(&mut self) -> std::result::Result<(), &'static str> {
        if self.used >= BUDGET {
            return Err("normalization_budget_exhausted");
        }
        self.used += 1;
        Ok(())
    }
}
#[derive(Default)]
struct Model {
    documents: BTreeMap<String, Option<String>>,
    functions: BTreeMap<String, bool>,
    contextual: HashSet<String>,
    owner_documents: BTreeMap<String, String>,
    owner_evidence: BTreeMap<String, Value>,
    nodes: BTreeMap<String, Record>,
    edges: BTreeMap<String, Record>,
    boundaries: usize,
}
struct Record {
    owners: Vec<String>,
    value: Value,
    evidence: Value,
}
fn key(value: &Value) -> String {
    content_hash(&value.to_string())
}
fn owner(file: &File, symbol: &Symbol) -> String {
    key(
        &json!({"document_id":file.id,"project":file.project,"name":binding_name(&symbol.qualified_name),"kind":symbol.kind}),
    )
}
fn source(file: &File) -> Value {
    json!({"document_id":file.id,"project":file.project,"path":file.path,"revision":file.revision,"content_hash":file.hash})
}
fn syntax(
    root: Node,
    text: &str,
    budget: &mut Budget,
) -> std::result::Result<HashMap<(usize, usize, String), String>, &'static str> {
    let mut stack = vec![root];
    let mut ordered = Vec::new();
    while let Some(node) = stack.pop() {
        budget.spend()?;
        if node.kind() == "comment" {
            continue;
        }
        ordered.push(node);
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    let mut hashes: HashMap<usize, String> = HashMap::new();
    let mut spans = HashMap::new();
    for node in ordered.into_iter().rev() {
        budget.spend()?;
        let mut cursor = node.walk();
        let children: Vec<_> = node
            .children(&mut cursor)
            .filter(|n| n.kind() != "comment")
            .map(|n| hashes[&n.id()].clone())
            .collect();
        let leaf = if children.is_empty() {
            let raw = text.get(node.byte_range()).ok_or("invalid_syntax_span")?;
            Some(if node.grammar_name() == "identifier" {
                binding_name(raw)
            } else {
                raw.to_string()
            })
        } else {
            None
        };
        let hash = key(&json!({"kind":node.kind(),"leaf":leaf,"children":children}));
        hashes.insert(node.id(), hash.clone());
        if node.is_named() {
            spans.insert(
                (node.start_byte(), node.end_byte(), node.kind().to_string()),
                hash,
            );
        }
    }
    Ok(spans)
}
fn ast_kinds(kind: &str) -> &[&str] {
    match kind {
        "parameter" | "definition" | "read" | "iteration_item" => {
            &["identifier", "as_pattern_target"]
        }
        "literal" => &["integer", "float", "true", "false", "none", "string"],
        "expression" => &[
            "binary_operator",
            "unary_operator",
            "comparison_operator",
            "parenthesized_expression",
        ],
        "call_result" => &["call"],
        "return" => &["return_statement"],
        "merge" => &["if_statement"],
        "loop_header" | "loop_exit" | "iteration_control" => &["for_statement", "while_statement"],
        "completion_binding" | "try_merge" => &["try_statement"],
        "exception_match" | "caught_exception" => &["except_clause"],
        "raise" => &["raise_statement"],
        "exception_trigger_iteration" => &["for_statement"],
        "exception_trigger_call" => &["call"],
        "exception_trigger_operation" => {
            &["binary_operator", "unary_operator", "comparison_operator"]
        }
        "exception_trigger_truth" => &[
            "identifier",
            "integer",
            "float",
            "true",
            "false",
            "none",
            "string",
            "call",
            "binary_operator",
            "unary_operator",
            "comparison_operator",
            "parenthesized_expression",
        ],
        "break_statement" => &["break_statement"],
        "continue_statement" => &["continue_statement"],
        _ => &[],
    }
}
fn edge(
    model: &mut Model,
    source: &str,
    target: &str,
    relation: &str,
    frame: Option<&str>,
    owners: Vec<String>,
    evidence: Value,
) {
    let value = json!({"source_node_key":source,"target_node_key":target,"relation":relation,"call_context_key":frame});
    model.edges.insert(
        key(&value),
        Record {
            owners,
            value,
            evidence,
        },
    );
}
fn model(
    graph: &Value,
    files: &[File],
    budget: &mut Budget,
) -> std::result::Result<Model, &'static str> {
    let selected: HashSet<_> = files.iter().map(|f| f.id.as_str()).collect();
    let mut captures: HashMap<&str, Vec<&Value>> = HashMap::new();
    for doc in graph["data_flow"]["documents"]
        .as_array()
        .ok_or("invalid_frozen_value_capture")?
    {
        if let Some(id) = doc["document_id"]
            .as_str()
            .filter(|id| selected.contains(id))
        {
            captures.entry(id).or_default().push(doc);
        }
    }
    let mut out = Model::default();
    let mut physical_nodes = HashMap::new();
    let mut valid_functions = Vec::new();
    for file in files.iter().filter(|f| f.path.ends_with(".py")) {
        let result = (|| -> std::result::Result<(), String> {
            budget.spend().map_err(str::to_string)?;
            let indexed: IndexedDocument =
                serde_json::from_str(file.analysis.as_deref().ok_or("missing_frozen_analysis")?)
                    .map_err(|_| "invalid_frozen_analysis")?;
            if indexed.parse_errors {
                return Err("parse_errors".into());
            }
            let docs = captures
                .get(file.id.as_str())
                .ok_or("missing_frozen_value_document")?;
            if docs.len() != 1 {
                return Err("duplicate_frozen_value_document".into());
            }
            let captured: DocumentFacts = serde_json::from_value(docs[0].clone())
                .map_err(|_| "invalid_frozen_value_document")?;
            if captured.content_hash != file.hash
                || captured.revision != file.revision
                || captured.path != file.path
                || captured.project != file.project
                || captured.facts.format != graph["data_flow_format"].as_u64().unwrap_or(0) as u32
                || !captured.facts.complete
            {
                return Err("incomplete_or_mismatched_frozen_capture".into());
            }
            let mut parser = tree_sitter::Parser::new();
            parser
                .set_language(&tree_sitter_python::LANGUAGE.into())
                .map_err(|_| "parser_unavailable")?;
            let tree = parser.parse(&file.source, None).ok_or("parse_cancelled")?;
            if tree.root_node().has_error() {
                return Err("parse_errors".into());
            }
            let spans = syntax(tree.root_node(), &file.source, budget).map_err(str::to_string)?;
            let mut symbols = HashMap::new();
            let mut identities: HashMap<String, usize> = HashMap::new();
            for symbol in &indexed.symbols {
                budget.spend().map_err(str::to_string)?;
                if symbol.kind == "function_definition" {
                    *identities.entry(owner(file, symbol)).or_default() += 1;
                    symbols.insert(symbol.id.as_str(), symbol);
                }
            }
            let ast_functions: HashSet<_> = spans
                .keys()
                .filter(|(_, _, kind)| kind == "function_definition")
                .map(|(start, end, _)| (*start, *end))
                .collect();
            let symbol_functions: HashSet<_> = symbols
                .values()
                .map(|s| (s.start_byte, s.end_byte))
                .collect();
            let captured_ids: HashSet<_> = captured
                .facts
                .functions
                .iter()
                .map(|f| f.symbol.as_str())
                .collect();
            if ast_functions != symbol_functions
                || captured_ids.len() != captured.facts.functions.len()
                || captured_ids != symbols.keys().copied().collect()
            {
                return Err("incomplete_or_duplicate_frozen_function_inventory".into());
            }
            for function in &captured.facts.functions {
                budget.spend().map_err(str::to_string)?;
                let symbol = symbols
                    .get(function.symbol.as_str())
                    .ok_or("missing_frozen_function_owner")?;
                let owner = owner(file, symbol);
                if captured.facts.format == 5
                    && function.nodes.iter().any(|n| !n.flow_context.is_empty())
                {
                    return Err("legacy_capture_has_execution_contexts".into());
                }
                if function.nodes.iter().any(|n| !n.flow_context.is_empty()) {
                    out.contextual.insert(owner.clone());
                }
                let qualified = function.qualified && identities[&owner] == 1;
                out.functions
                    .entry(owner.clone())
                    .and_modify(|v| *v = false)
                    .or_insert(qualified);
                out.owner_documents.insert(owner.clone(), file.id.clone());
                out.owner_evidence.insert(owner.clone(),json!({"source":source(file),"function_id":symbol.id,"function_name":symbol.qualified_name}));
                if !qualified {
                    continue;
                }
                let mut ordinals = HashMap::<String, usize>::new();
                let mut nodes: Vec<_> = function.nodes.iter().collect();
                nodes.sort_by_key(|n| (n.start_byte, n.end_byte, n.kind.as_str(), n.id.as_str()));
                for node in nodes {
                    budget.spend().map_err(str::to_string)?;
                    if node.function != function.symbol
                        || node.start_byte < symbol.start_byte
                        || node.end_byte > symbol.end_byte
                    {
                        return Err("invalid_frozen_node_owner".into());
                    }
                    let syntax_hash = ast_kinds(&node.kind)
                        .iter()
                        .find_map(|kind| {
                            spans.get(&(node.start_byte, node.end_byte, kind.to_string()))
                        })
                        .ok_or("invalid_frozen_node_span_or_kind")?;
                    let raw = file
                        .source
                        .get(node.start_byte..node.end_byte)
                        .ok_or("invalid_frozen_node_span")?;
                    if node.name.as_ref().is_some_and(|n| n != raw)
                        || node.binding_name != node.name.as_deref().map(binding_name)
                    {
                        return Err("invalid_frozen_node_identity".into());
                    }
                    let suffix = if node.flow_context.is_empty() {
                        String::new()
                    } else {
                        format!("@{}", key(&json!(node.flow_context)))
                    };
                    let physical_id = node
                        .id
                        .strip_suffix(&suffix)
                        .ok_or("invalid_frozen_execution_context")?;
                    let prefix = format!(
                        "{}:{}:{}:{}",
                        file.id, node.start_byte, node.end_byte, node.kind
                    );
                    let slot = if matches!(
                        node.kind.as_str(),
                        "merge" | "loop_header" | "loop_exit" | "completion_binding" | "try_merge"
                    ) {
                        physical_id
                            .strip_prefix(&format!("{prefix}:"))
                            .ok_or("invalid_synthetic_slot")?
                    } else {
                        if physical_id != prefix {
                            return Err("invalid_frozen_node_id".into());
                        }
                        ""
                    };
                    let shape = json!({"owner":owner,"kind":node.kind,"binding_name":node.binding_name,"slot":slot,"flow_context":node.flow_context});
                    let ordinal = ordinals.entry(shape.to_string()).or_default();
                    let identity = json!({"shape":shape,"occurrence":*ordinal});
                    *ordinal += 1;
                    let node_key = key(&identity);
                    if physical_nodes
                        .insert(node.id.clone(), node_key.clone())
                        .is_some()
                    {
                        return Err("duplicate_frozen_node_id".into());
                    }
                    out.nodes.insert(node_key,Record{owners:vec![owner.clone()],value:json!({"identity":identity,"syntax_hash":syntax_hash}),evidence:json!({"source":source(file),"function_id":symbol.id,"function_name":symbol.qualified_name,"start_byte":node.start_byte,"end_byte":node.end_byte,"kind":node.kind,"name":node.name})});
                }
                valid_functions.push((file, function.clone(), owner));
            }
            Ok(())
        })();
        out.documents.insert(file.id.clone(), result.err());
    }
    for (file, function, owner) in &valid_functions {
        for local in &function.edges {
            budget.spend()?;
            let source = physical_nodes
                .get(&local.source)
                .ok_or("missing_frozen_edge_source")?;
            let target = physical_nodes
                .get(&local.target)
                .ok_or("missing_frozen_edge_target")?;
            if out.nodes[source].owners != vec![owner.clone()]
                || out.nodes[target].owners != vec![owner.clone()]
            {
                return Err("invalid_frozen_local_edge_owner");
            }
            edge(
                &mut out,
                source,
                target,
                &local.kind,
                None,
                vec![owner.clone()],
                json!({"source":self::source(file)}),
            );
        }
    }
    let functions: HashMap<_, _> = valid_functions
        .iter()
        .map(|(file, f, owner)| (f.symbol.as_str(), (*file, f, owner)))
        .collect();
    let mut bindings: HashMap<(String, usize, String), Vec<&Value>> = HashMap::new();
    for binding in graph["calls"]
        .as_array()
        .ok_or("missing_frozen_call_bindings")?
    {
        let reference = &binding["reference"];
        if let (Some(caller), Some(start), Some(expression)) = (
            reference["caller"].as_str(),
            reference["start_byte"].as_u64(),
            reference["expression"].as_str(),
        ) {
            if let Some((file, _, _)) = functions.get(caller) {
                if reference["document_id"] != file.id
                    || reference["content_hash"] != file.hash
                    || reference["revision"] != file.revision
                    || reference["path"] != file.path
                {
                    return Err("mismatched_frozen_call_provenance");
                }
                budget.spend()?;
                bindings
                    .entry((caller.to_string(), start as usize, expression.to_string()))
                    .or_default()
                    .push(binding);
            }
        }
    }
    for (file, function, caller) in &valid_functions {
        for call in &function.calls {
            budget.spend()?;
            let bound = bindings.get(&(
                function.symbol.clone(),
                call.start_byte,
                call.expression.clone(),
            ));
            let target = bound
                .filter(|b| b.len() == 1)
                .and_then(|b| {
                    let candidates = b[0]["candidates"].as_array()?;
                    (b[0]["resolution"] == "resolved_static" && candidates.len() == 1)
                        .then(|| candidates[0].as_str())
                        .flatten()
                })
                .and_then(|id| functions.get(id));
            let Some((target_file, target, callee)) = target.filter(|(f, t, _)| {
                f.project == file.project
                    && call.binding_qualified
                    && !t.returns.is_empty()
                    && t.target_binding_qualified
                    && t.parameters.len() == call.arguments.len()
            }) else {
                out.boundaries += 1;
                continue;
            };
            let result = physical_nodes
                .get(&call.result)
                .ok_or("missing_frozen_call_result")?;
            if out.nodes[result].owners != vec![caller.clone()] {
                return Err("invalid_call_result_owner");
            }
            let owners = vec![caller.clone(), (*callee).clone()];
            for (argument, parameter) in call.arguments.iter().zip(&target.parameters) {
                budget.spend()?;
                let a = physical_nodes
                    .get(argument)
                    .ok_or("missing_frozen_argument")?;
                let p = physical_nodes
                    .get(parameter)
                    .ok_or("missing_frozen_parameter")?;
                if out.nodes[a].owners != vec![caller.clone()]
                    || out.nodes[p].owners != vec![(*callee).clone()]
                {
                    return Err("invalid_frozen_argument_or_parameter_owner");
                }
                edge(
                    &mut out,
                    a,
                    p,
                    "call_argument",
                    Some(result),
                    owners.clone(),
                    json!({"caller":source(file),"callee":source(target_file),"call_start_byte":call.start_byte}),
                );
            }
            for returned in &target.returns {
                budget.spend()?;
                let r = physical_nodes
                    .get(returned)
                    .ok_or("missing_frozen_return")?;
                if out.nodes[r].owners != vec![(*callee).clone()] {
                    return Err("invalid_frozen_return_owner");
                }
                edge(
                    &mut out,
                    r,
                    result,
                    "return_to_call",
                    Some(result),
                    owners.clone(),
                    json!({"caller":source(file),"callee":source(target_file),"call_start_byte":call.start_byte}),
                );
            }
        }
    }
    Ok(out)
}

pub(in crate::code_index) fn compare(
    before: &Value,
    after: &Value,
    before_files: &[File],
    after_files: &[File],
    limit: usize,
) -> Value {
    let mut report = json!({"available":false,"normalization_format":3,"capture_format":7,"supported_capture_formats":[5,6,7],"before_capture_format":before["data_flow_format"],"after_capture_format":after["data_flow_format"],"before_capture_available":before["data_flow_format"].as_u64().is_some_and(|v|v>=1),"after_capture_available":after["data_flow_format"].as_u64().is_some_and(|v|v>=1),"source_semantic_change_proven":false,"runtime_path_proven":false,"absence_proven":false,"bodies_included":false,"scope":"frozen_python_syntax_value_and_unique_positional_call_candidates","truncated":false,"limits":{"items_per_collection":limit,"normalization_work":BUDGET}});
    if !matches!(before["data_flow_format"].as_u64(), Some(5..=7))
        || !matches!(after["data_flow_format"].as_u64(), Some(5..=7))
    {
        report["reason"] = "incompatible_or_legacy_value_capture".into();
        return report;
    }
    let mut budget = Budget { used: 0 };
    let models = model(before, before_files, &mut budget)
        .and_then(|a| model(after, after_files, &mut budget).map(|b| (a, b)));
    report["normalization_work"] = budget.used.into();
    let (old, new) = match models {
        Ok(v) => v,
        Err(reason) => {
            report["reason"] = reason.into();
            return report;
        }
    };
    let files: HashMap<_, _> = before_files
        .iter()
        .chain(after_files)
        .map(|f| (f.id.as_str(), f))
        .collect();
    let mut excluded_documents = Vec::new();
    for id in old
        .documents
        .keys()
        .chain(new.documents.keys())
        .collect::<BTreeSet<_>>()
    {
        let a = old.documents.get(id).and_then(Option::as_ref);
        let b = new.documents.get(id).and_then(Option::as_ref);
        if a.is_some() || b.is_some() {
            excluded_documents.push(
                json!({"source":source(files[id.as_str()]),"before_reason":a,"after_reason":b}),
            );
        }
    }
    let excluded_ids: HashSet<_> = excluded_documents
        .iter()
        .filter_map(|d| d["source"]["document_id"].as_str())
        .collect();
    let mut eligible = HashSet::new();
    let mut excluded_functions = Vec::new();
    for owner in old
        .functions
        .keys()
        .chain(new.functions.keys())
        .collect::<BTreeSet<_>>()
    {
        let excluded_doc = old
            .owner_documents
            .get(owner)
            .into_iter()
            .chain(new.owner_documents.get(owner))
            .any(|id| excluded_ids.contains(id.as_str()));
        let changed_context_model = before["data_flow_format"] != after["data_flow_format"]
            && (old.contextual.contains(owner) || new.contextual.contains(owner));
        if changed_context_model
            || old.functions.get(owner) == Some(&false)
            || new.functions.get(owner) == Some(&false)
            || excluded_doc
        {
            excluded_functions.push(json!({"owner_key":owner,"capture_context_format_changed":changed_context_model,"before_qualified":old.functions.get(owner),"after_qualified":new.functions.get(owner),"before":old.owner_evidence.get(owner),"after":new.owner_evidence.get(owner)}));
        } else {
            eligible.insert(owner.clone());
        }
    }
    let valid = |r: &&Record| r.owners.iter().all(|o| eligible.contains(o));
    let old_nodes: BTreeMap<_, _> = old.nodes.iter().filter(|(_, r)| valid(r)).collect();
    let new_nodes: BTreeMap<_, _> = new.nodes.iter().filter(|(_, r)| valid(r)).collect();
    let mut changes = Vec::new();
    for id in old_nodes
        .keys()
        .chain(new_nodes.keys())
        .collect::<BTreeSet<_>>()
    {
        let a = old_nodes.get(id);
        let b = new_nodes.get(id);
        if a.map(|r| &r.value) == b.map(|r| &r.value) {
            continue;
        }
        changes.push(json!({"node_key":id,"kind":if a.is_none(){"added"}else if b.is_none(){"removed"}else{"changed"},"before":a.map(|r|json!({"node":r.value,"evidence":r.evidence})),"after":b.map(|r|json!({"node":r.value,"evidence":r.evidence}))}));
    }
    let old_edges: BTreeMap<_, _> = old.edges.iter().filter(|(_, r)| valid(r)).collect();
    let new_edges: BTreeMap<_, _> = new.edges.iter().filter(|(_, r)| valid(r)).collect();
    let added: Vec<_> = new_edges
        .iter()
        .filter(|(k, _)| !old_edges.contains_key(*k))
        .map(|(k, r)| json!({"edge_key":k,"edge":r.value,"evidence":r.evidence}))
        .collect();
    let removed: Vec<_> = old_edges
        .iter()
        .filter(|(k, _)| !new_edges.contains_key(*k))
        .map(|(k, r)| json!({"edge_key":k,"edge":r.value,"evidence":r.evidence}))
        .collect();
    report["available"] = true.into();
    report["complete_for_captured_scope"] = (excluded_documents.is_empty()
        && excluded_functions.is_empty()
        && old.boundaries == 0
        && new.boundaries == 0)
        .into();
    report["eligible_functions"] = eligible.len().into();
    report["excluded_documents_total"] = excluded_documents.len().into();
    report["excluded_documents"] = excluded_documents
        .iter()
        .take(limit)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    report["excluded_functions_total"] = excluded_functions.len().into();
    report["excluded_functions"] = excluded_functions
        .iter()
        .take(limit)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    report["unverified_call_boundaries"] = json!({"before":old.boundaries,"after":new.boundaries});
    report["nodes_changed_total"] = changes.len().into();
    report["nodes_changed"] = changes
        .iter()
        .take(limit)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    report["edges_added_total"] = added.len().into();
    report["edges_added"] = added.iter().take(limit).cloned().collect::<Vec<_>>().into();
    report["edges_removed_total"] = removed.len().into();
    report["edges_removed"] = removed
        .iter()
        .take(limit)
        .cloned()
        .collect::<Vec<_>>()
        .into();
    report["truncated"] = (changes.len() > limit
        || added.len() > limit
        || removed.len() > limit
        || excluded_documents.len() > limit
        || excluded_functions.len() > limit)
        .into();
    report
}
