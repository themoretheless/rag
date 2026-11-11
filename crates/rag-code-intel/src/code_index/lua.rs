//! Lua lexical function candidates; no execution, require resolution or table dispatch.
use super::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Facts {
    bindings: Vec<Binding>,
    writes: Vec<(String, usize)>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Binding {
    name: String,
    start: usize,
    end: usize,
    visible: usize,
    symbol: Option<String>,
    mutable: bool,
}
fn children(n: Node<'_>) -> Vec<Node<'_>> {
    let mut c = n.walk();
    n.named_children(&mut c).collect()
}
fn scope(n: Node) -> (usize, usize) {
    let mut parent = n.parent();
    while let Some(p) = parent {
        if matches!(p.kind(), "block" | "chunk") {
            let end = if p.parent().is_some_and(|p| p.kind() == "repeat_statement") {
                p.parent().unwrap().end_byte()
            } else {
                p.end_byte()
            };
            return (p.start_byte(), end);
        }
        parent = p.parent();
    }
    (0, usize::MAX)
}
fn terminal(n: Node<'_>) -> Node<'_> {
    n.child_by_field_name("field")
        .or_else(|| n.child_by_field_name("method"))
        .unwrap_or(n)
}
fn assignment(n: Node<'_>) -> (Vec<Node<'_>>, Vec<Node<'_>>) {
    let c = children(n);
    let vars = c
        .iter()
        .find(|n| n.kind() == "variable_list")
        .map(|n| children(*n))
        .unwrap_or_default();
    let values = c
        .iter()
        .find(|n| n.kind() == "expression_list")
        .map(|n| children(*n))
        .unwrap_or_default();
    (
        vars.into_iter()
            .filter(|n| n.kind() != "attribute")
            .collect(),
        values,
    )
}
fn assigned_name(n: Node<'_>) -> Option<Node<'_>> {
    let parent = n.parent()?;
    if parent.kind() != "expression_list" {
        return None;
    }
    let a = parent.parent()?;
    if a.kind() != "assignment_statement" {
        return None;
    }
    let (vars, values) = assignment(a);
    (vars.len() == 1 && values.len() == 1 && values[0].id() == n.id()).then(|| vars[0])
}
fn id(doc: &str, n: Node, name: &str) -> String {
    format!("{doc}:{}:{name}", n.start_byte())
}
fn bind(
    out: &mut IndexedDocument,
    name: &str,
    bounds: (usize, usize),
    visible: usize,
    symbol: Option<String>,
) {
    out.lua.as_mut().unwrap().bindings.push(Binding {
        name: name.into(),
        start: bounds.0,
        end: bounds.1,
        visible,
        symbol,
        mutable: false,
    });
}
fn lookup(facts: &Facts, name: &str, pos: usize) -> Option<usize> {
    facts
        .bindings
        .iter()
        .enumerate()
        .filter(|(_, b)| b.name == name && b.start <= pos && pos < b.end && b.visible <= pos)
        .max_by_key(|(i, b)| (std::cmp::Reverse(b.end - b.start), b.visible, *i))
        .map(|(i, _)| i)
}
fn reference(
    node: Node,
    target: Node,
    source: &str,
    doc: &str,
    caller: Option<&str>,
    call: bool,
    out: &mut IndexedDocument,
) {
    let name = terminal(target);
    out.references.push(Reference {
        build_context: None,
        document_id: doc.into(),
        path: out.path.clone(),
        name: text(name, source).into(),
        expression: text(target, source).into(),
        start_byte: node.start_byte(),
        line: name.start_position().row + 1,
        column: name.start_position().column + 1,
        caller: caller.map(str::to_string),
        is_call: call,
        revision: out.revision,
        content_hash: out.hash.clone(),
    });
}
fn walk(
    n: Node,
    source: &str,
    doc: &str,
    caller: Option<&str>,
    names: &[String],
    out: &mut IndexedDocument,
    depth: usize,
) {
    if depth >= 256 {
        out.parse_errors = true;
        return;
    }
    if matches!(
        n.kind(),
        "comment" | "string" | "label_statement" | "goto_statement"
    ) {
        return;
    }
    if matches!(n.kind(), "function_declaration" | "function_definition") {
        let declared = n.child_by_field_name("name");
        let named = declared.or_else(|| assigned_name(n));
        let name = named
            .map(|v| text(terminal(v), source).to_string())
            .unwrap_or_else(|| format!("<anonymous@{}>", n.start_byte()));
        let symbol = id(doc, n, &name);
        let mut qualified = names.to_vec();
        qualified.push(
            named
                .map(|v| text(v, source).replace([':', '.'], "::"))
                .unwrap_or_else(|| name.clone()),
        );
        let body = n.child_by_field_name("body");
        out.symbols.push(Symbol {
            build_context: None,
            documentation: None,
            documentation_examined: false,
            parameter_contracts: None,
            id: symbol.clone(),
            document_id: doc.into(),
            path: out.path.clone(),
            name: name.clone(),
            qualified_name: qualified.join("::"),
            kind: n.kind().into(),
            signature: source[n.start_byte()..body.map_or(n.end_byte(), |b| b.start_byte())]
                .trim()
                .into(),
            start_line: n.start_position().row + 1,
            end_line: n.end_position().row + 1,
            start_byte: n.start_byte(),
            end_byte: n.end_byte(),
            revision: out.revision,
            content_hash: out.hash.clone(),
        });
        if n.kind() == "function_declaration" && declared.is_some_and(|v| v.kind() == "identifier")
        {
            if n.child(0).is_some_and(|token| token.kind() == "local") {
                bind(out, &name, scope(n), n.start_byte(), Some(symbol.clone()));
            } else {
                out.lua
                    .as_mut()
                    .unwrap()
                    .writes
                    .push((name.clone(), n.start_byte()));
            }
        }
        let bounds = body
            .map(|b| (b.start_byte(), b.end_byte()))
            .unwrap_or((n.start_byte(), n.end_byte()));
        if let Some(parameters) = n.child_by_field_name("parameters") {
            for p in children(parameters)
                .into_iter()
                .filter(|p| p.kind() == "identifier")
            {
                bind(out, text(p, source), bounds, bounds.0, None)
            }
        }
        if declared.is_some_and(|v| v.kind() == "method_index_expression") {
            bind(out, "self", bounds, bounds.0, None)
        }
        if let Some(body) = body {
            walk(body, source, doc, Some(&symbol), &qualified, out, depth + 1)
        }
        return;
    }
    if n.kind() == "variable_declaration" {
        let c = children(n);
        let a = c
            .iter()
            .find(|n| n.kind() == "assignment_statement")
            .copied();
        let (vars, values) = a.map(assignment).unwrap_or_else(|| {
            (
                c.iter()
                    .find(|n| n.kind() == "variable_list")
                    .map(|n| children(*n))
                    .unwrap_or_default(),
                vec![],
            )
        });
        for (i, v) in vars
            .iter()
            .enumerate()
            .filter(|(_, v)| v.kind() == "identifier")
        {
            let symbol = if vars.len() == 1 && values.len() == 1 {
                values
                    .get(i)
                    .filter(|f| f.kind() == "function_definition")
                    .map(|f| id(doc, *f, text(*v, source)))
            } else {
                None
            };
            bind(out, text(*v, source), scope(n), n.end_byte(), symbol);
        }
        for value in values {
            walk(value, source, doc, caller, names, out, depth + 1)
        }
        return;
    }
    if n.kind() == "assignment_statement" {
        let (vars, values) = assignment(n);
        for v in vars {
            if v.kind() == "identifier" {
                out.lua
                    .as_mut()
                    .unwrap()
                    .writes
                    .push((text(v, source).into(), v.start_byte()))
            } else if let Some(table) = v.child_by_field_name("table") {
                walk(table, source, doc, caller, names, out, depth + 1)
            }
        }
        for v in values {
            walk(v, source, doc, caller, names, out, depth + 1)
        }
        return;
    }
    if n.kind() == "for_statement" {
        let body = n.child_by_field_name("body");
        let bounds = body.map(|b| (b.start_byte(), n.end_byte()));
        if let Some(clause) = n.child_by_field_name("clause") {
            if clause.kind() == "for_numeric_clause" {
                if let (Some(name), Some(bounds)) = (clause.child_by_field_name("name"), bounds) {
                    bind(out, text(name, source), bounds, bounds.0, None)
                }
                for field in ["start", "end", "step"] {
                    if let Some(v) = clause.child_by_field_name(field) {
                        walk(v, source, doc, caller, names, out, depth + 1)
                    }
                }
            } else {
                for c in children(clause) {
                    if c.kind() == "variable_list" {
                        if let Some(bounds) = bounds {
                            for v in children(c).into_iter().filter(|v| v.kind() == "identifier") {
                                bind(out, text(v, source), bounds, bounds.0, None)
                            }
                        }
                    } else {
                        walk(c, source, doc, caller, names, out, depth + 1)
                    }
                }
            }
        }
        if let Some(body) = body {
            walk(body, source, doc, caller, names, out, depth + 1)
        }
        return;
    }
    if n.kind() == "function_call" {
        if let Some(target) = n.child_by_field_name("name") {
            reference(n, target, source, doc, caller, true, out);
            if let Some(table) = target.child_by_field_name("table") {
                walk(table, source, doc, caller, names, out, depth + 1)
            } else if target.kind() != "identifier" {
                walk(target, source, doc, caller, names, out, depth + 1)
            }
        }
        if let Some(args) = n.child_by_field_name("arguments") {
            walk(args, source, doc, caller, names, out, depth + 1)
        }
        return;
    }
    if matches!(n.kind(), "dot_index_expression" | "method_index_expression") {
        reference(n, n, source, doc, caller, false, out);
        if let Some(table) = n.child_by_field_name("table") {
            walk(table, source, doc, caller, names, out, depth + 1)
        }
        return;
    }
    if n.kind() == "identifier" {
        reference(n, n, source, doc, caller, false, out);
        return;
    }
    // Named table keys are declarations of fields, not bare lexical reads.
    if n.kind() == "field" {
        for c in children(n) {
            if n.child_by_field_name("name").is_some_and(|key| {
                key.id() == c.id()
                    && c.kind() == "identifier"
                    && n.child(0).is_none_or(|first| first.kind() != "[")
            }) {
                continue;
            }
            walk(c, source, doc, caller, names, out, depth + 1)
        }
        return;
    }
    for c in children(n) {
        walk(c, source, doc, caller, names, out, depth + 1)
    }
}
pub(super) fn parse_source(doc: &str, source: &str, out: &mut IndexedDocument) -> Result<()> {
    out.lua = Some(Facts::default());
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_lua::LANGUAGE.into())
        .map_err(|e| AppError::config(e.to_string()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AppError::config("Lua parse cancelled"))?;
    out.parse_errors = tree.root_node().has_error();
    out.binding_limitations
        .push("LUA_TABLE_IMPORT_ENV_AND_RUNTIME_BINDING_UNVERIFIED".into());
    walk(tree.root_node(), source, doc, None, &[], out, 0);
    let facts = out.lua.as_mut().unwrap();
    for (name, pos) in facts.writes.clone() {
        if let Some(i) = lookup(facts, &name, pos) {
            facts.bindings[i].mutable = true
        }
    }
    Ok(())
}
pub(super) fn resolve(doc: &IndexedDocument, r: &Reference) -> CallEdge {
    let facts = doc.lua.as_ref().unwrap();
    let mut candidates = vec![];
    let reason = if doc.parse_errors {
        "lua_parse_error"
    } else if r.expression != r.name {
        "lua_dynamic_or_table_target_unverified"
    } else if let Some(i) = lookup(facts, &r.name, r.start_byte) {
        let b = &facts.bindings[i];
        if b.mutable {
            "lua_local_binding_reassigned"
        } else if let Some(symbol) = &b.symbol {
            candidates.push(symbol.clone());
            "lua_lexical_local_function_candidate"
        } else {
            "lua_local_parameter_or_value_unresolved"
        }
    } else {
        "lua_global_import_or_environment_unverified"
    };
    CallEdge {
        reference: r.clone(),
        resolution: if candidates.len() == 1 {
            "resolved_static"
        } else {
            "unresolved"
        }
        .into(),
        candidates,
        provenance: "derived",
        resolution_basis: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn facts(source: &str) -> IndexedDocument {
        let doc = super::super::parse("a", "/p/main.lua", source, 1).unwrap();
        assert!(!doc.parse_errors, "{source}");
        doc
    }
    fn calls(doc: &IndexedDocument) -> Vec<CallEdge> {
        doc.references
            .iter()
            .filter(|r| r.is_call)
            .map(|r| resolve(doc, r))
            .collect()
    }
    #[test]
    fn local_declarations_assignments_recursion_and_value_reads_have_provenance() {
        let source="local\nfunction leaf(x) return x end\nlocal function recursive() recursive() end\nlocal worker = function(x) leaf(x); return leaf end\nworker(1)\n";
        let doc = facts(source);
        assert_eq!(doc.symbols.len(), 3);
        let edges = calls(&doc);
        assert_eq!(edges.len(), 3);
        assert!(
            edges.iter().all(|e| e.resolution == "resolved_static"),
            "{edges:?}"
        );
        let worker = doc.symbols.iter().find(|s| s.name == "worker").unwrap();
        assert!(source[worker.start_byte..worker.end_byte].starts_with("function"));
        assert!(doc.references.iter().any(|r| !r.is_call
            && r.name == "leaf"
            && resolve(&doc, r).resolution == "resolved_static"));
        assert!(edges
            .iter()
            .all(|e| e.reference.content_hash == doc.hash && e.reference.revision == 1));
    }
    #[test]
    fn local_parameters_blocks_and_member_names_do_not_alias_outer_functions() {
        let doc=facts("local function target() end\nlocal function use(target) target() end\ndo local target=other; target() end\nM.target()\ntarget()\nlocal t={target=1,[target]=2}\n");
        let edges = calls(&doc);
        assert_eq!(edges.len(), 4);
        assert_eq!(
            edges
                .iter()
                .filter(|e| e.resolution == "resolved_static")
                .count(),
            1,
            "{edges:?}"
        );
        assert_eq!(edges.last().unwrap().reference.line, 5);
        let values: Vec<_> = doc
            .references
            .iter()
            .filter(|r| {
                !r.is_call && r.name == "target" && resolve(&doc, r).resolution == "resolved_static"
            })
            .collect();
        assert_eq!(values.len(), 1, "{values:?}");
    }
    #[test]
    fn local_writes_in_closures_and_function_statement_reassignment_invalidate_links() {
        for source in [
            "local function target() end\nlocal function mutate() target=other end\ntarget()",
            "local function target() end\nfunction target() end\ntarget()",
        ] {
            let doc = facts(source);
            let edges = calls(&doc);
            assert_eq!(edges.last().unwrap().resolution, "unresolved");
            assert_eq!(
                edges.last().unwrap().resolution_basis,
                "lua_local_binding_reassigned"
            );
        }
    }
    #[test]
    fn loops_and_repeat_conditions_obey_lexical_boundaries() {
        let doc=facts("local function step() return true end\nfor step=step(),2 do step() end\nstep()\nrepeat local function check() return true end until check()\ncheck()\n");
        let edges = calls(&doc);
        assert_eq!(edges.len(), 5, "{edges:?}");
        assert_eq!(
            edges
                .iter()
                .map(|e| e.resolution.as_str())
                .collect::<Vec<_>>(),
            vec![
                "resolved_static",
                "unresolved",
                "resolved_static",
                "resolved_static",
                "unresolved"
            ]
        );
    }
    #[test]
    fn generic_loop_bindings_and_local_initializers_shadow_only_after_values() {
        let doc=facts("local function target() return true end\nfor target in target() do target() end\nlocal target=target()\ntarget()\n");
        let edges = calls(&doc);
        assert_eq!(
            edges
                .iter()
                .map(|e| e.resolution.as_str())
                .collect::<Vec<_>>(),
            vec![
                "resolved_static",
                "unresolved",
                "resolved_static",
                "unresolved"
            ]
        );
    }
    #[test]
    fn globals_table_methods_alias_calls_and_initializer_recursion_stay_unresolved() {
        let doc=facts("local function self() end\nfunction M.run(x) return x end\nfunction M:method() self() end\nlocal alias=self\nalias()\nlocal f=function() f() end\nrequire('foreign')\nM.run()\n");
        assert!(doc.symbols.iter().any(|s| s.qualified_name == "M::run"));
        assert!(doc.symbols.iter().any(|s| s.qualified_name == "M::method"));
        let edges = calls(&doc);
        assert!(
            edges.iter().all(|e| e.resolution == "unresolved"),
            "{edges:?}"
        );
        let other = facts("self()");
        assert!(calls(&other).iter().all(|e| e.candidates.is_empty()));
    }
    #[test]
    fn comments_long_strings_and_parse_errors_never_create_false_static_calls() {
        let doc=facts("-- local function fake() end\nlocal text=[=[function fake2() fake() end]=]\nlocal function real() end\nreal()\n");
        assert_eq!(doc.symbols.len(), 1);
        assert_eq!(calls(&doc).len(), 1);
        let bad = super::super::parse("b", "/p/b.lua", "local function f() f()", 1).unwrap();
        assert!(bad.parse_errors);
        assert!(calls(&bad).iter().all(|e| e.resolution == "unresolved"));
    }
}
