//! Internal Python identifier projection with original source identities preserved.
use super::*;
use unicode_normalization::UnicodeNormalization;
mod flow;
#[cfg(test)]
mod tests;
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Scope {
    kind: String,
    start: usize,
    end: usize,
    body: usize,
    parameters: Vec<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(in crate::code_index) struct Facts {
    format: u32,
    #[serde(default)]
    receiver_flow_format: Option<u32>,
    #[serde(default)]
    receiver_flows: Vec<flow::Function>,
    complete: bool,
    scopes: Vec<Scope>,
    guarded: Vec<(usize, usize)>,
    #[serde(default)]
    loop_targets: Vec<(String, usize, usize)>,
    #[serde(default)]
    handler_aliases: Vec<(String, usize, usize, usize)>,
    #[serde(default)]
    assignments: Vec<(usize, usize)>,
    #[serde(default)]
    augmented: Vec<(String, usize, usize)>,
    #[serde(default)]
    annotations: Vec<(String, usize)>,
}
fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}
pub(in crate::code_index) fn handler_parts(
    node: Node<'_>,
) -> std::result::Result<(Vec<Node<'_>>, Option<Node<'_>>), String> {
    let mut cursor = node.walk();
    let mut types: Vec<_> = node.children_by_field_name("value", &mut cursor).collect();
    let mut alias = node.child_by_field_name("alias");
    if types.len() == 1 && types[0].kind() == "as_pattern" {
        let pattern = types[0];
        alias = pattern.child_by_field_name("alias");
        types = children(pattern)
            .into_iter()
            .filter(|n| Some(n.id()) != alias.map(|a| a.id()) && n.kind() != "comment")
            .collect();
    }
    if let Some(target) = alias.filter(|n| n.kind() == "as_pattern_target") {
        let values = children(target);
        if values.len() == 1
            && values[0].kind() == "identifier"
            && values[0].byte_range() == target.byte_range()
        {
            alias = Some(values[0]);
        }
    }
    if alias.is_some_and(|n| n.grammar_name() != "identifier") {
        return Err("exception_alias_pattern_unverified".into());
    }
    Ok((types, alias))
}
fn norm(value: &str) -> String {
    value.nfkc().collect()
}
pub(in crate::code_index) fn current(doc: &IndexedDocument) -> bool {
    !doc.path.ends_with(".py")
        || doc.python_names.as_ref().is_some_and(|f| {
            f.format == 5
                && f.receiver_flow_format == Some(1)
                && (!f.complete
                    || f.receiver_flows.len()
                        == f.scopes
                            .iter()
                            .filter(|s| s.kind == "function_definition")
                            .count())
        })
}
pub(in crate::code_index) fn capture(root: Node, source: &str) -> Facts {
    let mut facts = Facts {
        format: 5,
        receiver_flow_format: None,
        receiver_flows: Vec::new(),
        complete: true,
        scopes: Vec::new(),
        guarded: Vec::new(),
        loop_targets: Vec::new(),
        handler_aliases: Vec::new(),
        assignments: Vec::new(),
        augmented: Vec::new(),
        annotations: Vec::new(),
    };
    let mut stack = vec![root];
    let mut work = 100000;
    while let Some(node) = stack.pop() {
        if work == 0
            || facts.scopes.len() >= 2048
            || facts.loop_targets.len() >= 2048
            || facts.handler_aliases.len() >= 2048
            || facts.assignments.len() >= 8192
            || facts.augmented.len() >= 2048
            || facts.annotations.len() >= 2048
        {
            facts.complete = false;
            break;
        }
        work -= 1;
        if matches!(node.kind(), "assignment" | "augmented_assignment") {
            if let Some(left) = node
                .child_by_field_name("left")
                .filter(|n| n.kind() == "identifier")
            {
                if node.kind() == "augmented_assignment" {
                    facts.augmented.push((
                        norm(text(left, source)),
                        node.start_byte(),
                        node.end_byte(),
                    ));
                } else if node.child_by_field_name("right").is_some() {
                    facts.assignments.push((node.start_byte(), node.end_byte()));
                } else if node.child_by_field_name("type").is_some() {
                    facts
                        .annotations
                        .push((norm(text(left, source)), node.start_byte()));
                }
            }
        }
        if matches!(node.kind(), "function_definition" | "class_definition") {
            if let Some(body) = node.child_by_field_name("body") {
                let parameters = node
                    .child_by_field_name("parameters")
                    .map(children)
                    .unwrap_or_default()
                    .iter()
                    .map(Node::start_byte)
                    .collect();
                facts.scopes.push(Scope {
                    kind: node.kind().into(),
                    start: node.start_byte(),
                    end: node.end_byte(),
                    body: body.start_byte(),
                    parameters,
                });
            }
        }
        if matches!(
            node.kind(),
            "lambda"
                | "list_comprehension"
                | "set_comprehension"
                | "dictionary_comprehension"
                | "generator_expression"
        ) {
            facts.guarded.push((node.start_byte(), node.end_byte()));
        }
        let complex_for = if node.kind() == "for_statement" {
            match (
                node.child_by_field_name("left"),
                node.child_by_field_name("body"),
            ) {
                (Some(left), Some(body)) if left.kind() == "identifier" => {
                    facts.loop_targets.push((
                        norm(text(left, source)),
                        left.start_byte(),
                        body.start_byte().saturating_sub(1),
                    ));
                    false
                }
                _ => true,
            }
        } else {
            false
        };
        let complex_handler = if node.kind() == "except_clause" {
            match handler_parts(node) {
                Ok((_, None)) => false,
                Ok((_, Some(alias))) if children(node).iter().any(|n| n.kind() == "block") => {
                    facts.handler_aliases.push((
                        norm(text(alias, source)),
                        alias.start_byte(),
                        alias.end_byte(),
                        node.end_byte(),
                    ));
                    false
                }
                _ => true,
            }
        } else {
            false
        };
        if matches!(
            node.kind(),
            "global_statement"
                | "nonlocal_statement"
                | "with_statement"
                | "delete_statement"
                | "named_expression"
        ) || complex_for
            || complex_handler
        {
            let mut parent = node.parent();
            while let Some(owner) = parent {
                if matches!(owner.kind(), "function_definition" | "module") {
                    facts.guarded.push((owner.start_byte(), owner.end_byte()));
                    break;
                }
                parent = owner.parent();
            }
        }
        stack.extend(children(node));
    }
    facts
}
pub(in crate::code_index) fn finish(root: Node, source: &str, doc: &mut IndexedDocument) {
    if let Some(facts) = &mut doc.python_names {
        facts.receiver_flows = flow::capture(root, source, &doc.bindings, facts);
        facts.receiver_flow_format = Some(1);
        if facts.receiver_flows.len()
            != facts
                .scopes
                .iter()
                .filter(|s| s.kind == "function_definition")
                .count()
        {
            facts.complete = false;
        }
    }
}
fn at(facts: &Facts, byte: usize) -> Option<usize> {
    facts
        .scopes
        .iter()
        .enumerate()
        .filter(|(_, s)| s.body <= byte && byte < s.end)
        .min_by_key(|(_, s)| s.end - s.body)
        .map(|(i, _)| i)
}
pub(in crate::code_index) fn visible(
    doc: &IndexedDocument,
    owner: Option<usize>,
    byte: usize,
) -> bool {
    let Some(owner) = owner else {
        return true;
    };
    doc.python_names
        .as_ref()
        .is_some_and(|facts| at(facts, byte) == Some(owner))
}
pub(in crate::code_index) fn class_body(doc: &IndexedDocument, byte: usize) -> Option<usize> {
    let facts = doc.python_names.as_ref()?;
    let scope = &facts.scopes[at(facts, byte)?];
    (scope.kind == "class_definition").then_some(scope.start)
}
pub(in crate::code_index) fn key(
    doc: &IndexedDocument,
    b: &resolve::Binding,
    byte: usize,
) -> (usize, bool, usize) {
    if !doc.path.ends_with(".py") {
        return (b.start, false, b.declared);
    }
    // Prefer initialized bindings within the nearest function scope; a future
    // local still shadows outer scopes when that function has no preceding value.
    let initialized = b.declared <= byte;
    (
        b.start,
        initialized,
        if initialized {
            b.declared
        } else {
            usize::MAX - b.declared
        },
    )
}
pub(in crate::code_index) fn references(
    symbols: &[Symbol],
    docs: &HashMap<String, IndexedDocument>,
    selected: &HashSet<String>,
) -> Vec<CallEdge> {
    let projects: HashSet<_> = selected
        .iter()
        .filter_map(|id| docs.get(id))
        .map(|d| d.project.as_ref())
        .collect();
    let mut projected: HashMap<_, _> = docs
        .iter()
        .filter(|(_, d)| d.path.ends_with(".py") && projects.contains(&d.project.as_ref()))
        .map(|(id, d)| {
            (
                id.clone(),
                IndexedDocument {
                    version: d.version,
                    hash: d.hash.clone(),
                    revision: d.revision,
                    path: d.path.clone(),
                    parse_errors: d.parse_errors,
                    references: d.references.clone(),
                    bindings: d.bindings.clone(),
                    project: d.project.clone(),
                    exports: d.exports.clone(),
                    bases: d.bases.clone(),
                    blocked_members: d.blocked_members.clone(),
                    binding_limitations: d.binding_limitations.clone(),
                    python_names: d.python_names.clone(),
                    ..Default::default()
                },
            )
        })
        .collect();
    let mut definitions: Vec<_> = symbols
        .iter()
        .filter(|s| projected.contains_key(&s.document_id))
        .cloned()
        .collect();
    for s in &mut definitions {
        if s.path.ends_with(".py") {
            s.name = norm(&s.name);
            s.qualified_name = norm(&s.qualified_name);
        }
    }
    let mut originals = HashMap::new();
    for (id, doc) in projected
        .iter_mut()
        .filter(|(_, d)| d.path.ends_with(".py"))
    {
        let original = &docs[id];
        if doc.python_names.as_ref().is_none_or(|f| !f.complete) {
            doc.binding_limitations
                .push("PYTHON_SCOPE_CAPTURE_UNAVAILABLE".into());
        }
        if let Some(facts) = &doc.python_names {
            for (local, declaration, committed) in &facts.augmented {
                doc.bindings.push(Binding {
                    local: local.clone(),
                    target: None,
                    ty: None,
                    start: 0,
                    end: usize::MAX,
                    declared: *committed,
                    hoisted: false,
                    rust_namespace: false,
                    rust_extern: false,
                    declaration_at: Some(*declaration),
                    python_class_scope: None,
                });
            }
            for (local, declaration) in &facts.annotations {
                if at(facts, *declaration)
                    .is_some_and(|i| facts.scopes[i].kind == "function_definition")
                {
                    doc.bindings.push(Binding {
                        local: local.clone(),
                        target: None,
                        ty: None,
                        start: 0,
                        end: usize::MAX,
                        declared: usize::MAX,
                        hoisted: true,
                        rust_namespace: false,
                        rust_extern: false,
                        declaration_at: Some(*declaration),
                        python_class_scope: None,
                    });
                }
            }
            for (local, declaration, initialized, cleared) in &facts.handler_aliases {
                for declared in [*initialized, *cleared] {
                    doc.bindings.push(Binding {
                        local: local.clone(),
                        target: None,
                        ty: None,
                        start: 0,
                        end: usize::MAX,
                        declared,
                        hoisted: false,
                        rust_namespace: false,
                        rust_extern: false,
                        declaration_at: Some(*declaration),
                        python_class_scope: None,
                    });
                }
            }
            for (local, declaration, initialized) in &facts.loop_targets {
                doc.bindings.push(Binding {
                    local: local.clone(),
                    target: None,
                    ty: None,
                    start: 0,
                    end: usize::MAX,
                    declared: *initialized,
                    hoisted: false,
                    rust_namespace: false,
                    rust_extern: false,
                    declaration_at: Some(*declaration),
                    python_class_scope: None,
                });
            }
        }
        let commits: HashMap<_, _> = doc
            .python_names
            .as_ref()
            .map(|f| f.assignments.iter().copied().collect())
            .unwrap_or_default();
        for b in &mut doc.bindings {
            if let Some(commit) = b.declaration_at.and_then(|position| commits.get(&position)) {
                b.declared = *commit;
            }

            b.local = norm(&b.local);
            b.target = b.target.as_deref().map(norm);
            b.ty = b.ty.as_deref().map(norm);
            if let Some(facts) = &doc.python_names {
                let declaration = b.declaration_at.unwrap_or(b.declared);
                let parameter = facts
                    .scopes
                    .iter()
                    .enumerate()
                    .find(|(_, s)| s.parameters.contains(&declaration))
                    .map(|(i, _)| i);
                let owner = parameter.or_else(|| at(facts, declaration));
                if let Some(owner) = owner {
                    let scope = &facts.scopes[owner];
                    b.start = scope.body;
                    b.end = scope.end;
                    if scope.kind == "class_definition" {
                        b.python_class_scope = Some(owner);
                    }
                    if scope.kind == "function_definition" {
                        b.hoisted = true;
                    }
                }
            }
        }
        for base in &mut doc.bases {
            base.derived = norm(&base.derived);
            base.base = norm(&base.base);
        }
        doc.blocked_members = doc.blocked_members.iter().map(|n| norm(n)).collect();
        doc.exports = doc
            .exports
            .iter()
            .map(|(k, v)| (norm(k), norm(v)))
            .collect();
        for (r, raw) in doc.references.iter_mut().zip(&original.references) {
            r.name = norm(&r.name);
            r.expression = norm(&r.expression);
            originals.insert(
                (
                    id.clone(),
                    r.start_byte,
                    r.is_call,
                    r.name.clone(),
                    r.expression.clone(),
                ),
                raw.clone(),
            );
        }
    }
    let mut edges = resolve::references_selected_raw(&definitions, &projected, Some(selected));
    flow::apply(&definitions, &projected, &mut edges);
    for e in &mut edges {
        let doc = &projected[&e.reference.document_id];
        let byte = e.reference.start_byte;
        let opaque = doc
            .python_names
            .as_ref()
            .is_some_and(|f| f.guarded.iter().any(|(a, b)| *a <= byte && byte < *b));
        let head = e
            .reference
            .expression
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end_matches("()");
        let shadowed_super = head == "super"
            && doc.bindings.iter().any(|b| {
                b.local == "super"
                    && b.start <= byte
                    && byte < b.end
                    && (b.hoisted || b.declared <= byte)
                    && visible(doc, b.python_class_scope, byte)
            });
        if opaque || shadowed_super {
            e.candidates.clear();
            e.resolution = "unresolved".into();
            e.resolution_basis = if opaque {
                "python_binding_scope_not_qualified"
            } else {
                "python_super_shadowed"
            }
            .into();
        }
        if let Some(raw) = originals.get(&(
            e.reference.document_id.clone(),
            e.reference.start_byte,
            e.reference.is_call,
            e.reference.name.clone(),
            e.reference.expression.clone(),
        )) {
            e.reference = raw.clone();
        }
    }
    edges
}
