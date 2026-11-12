use super::*;
fn modifier(node: Node, kind: &str) -> bool {
    let mut c = node.walk();
    node.children(&mut c).any(|n| n.kind() == kind)
}
fn domain(node: Node, default: &str) -> String {
    if modifier(node, "function") {
        "function"
    } else if modifier(node, "const") {
        "const"
    } else {
        default
    }
    .into()
}
fn import_clause(node: Node, source: &str, prefix: &str, default: &str) -> Option<Import> {
    let list = children(node);
    let name = list.iter().find(|n| {
        matches!(
            n.kind(),
            "name" | "qualified_name" | "relative_name" | "namespace_name"
        )
    })?;
    let target = plain(text(*name, source))?;
    let target = if prefix.is_empty() {
        target
    } else {
        format!("{prefix}\\{target}")
    };
    let alias = node
        .child_by_field_name("alias")
        .map(|n| text(n, source).to_string())
        .unwrap_or_else(|| target.rsplit('\\').next().unwrap().into());
    let alias = plain(&alias)?;
    if alias.contains('\\') {
        return None;
    }
    Some(Import {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        kind: domain(node, default),
        alias,
        target,
    })
}
fn imports(node: Node, source: &str, scope: &mut Scope) {
    let list = children(node);
    let mut default = domain(node, "class");
    if node.child_by_field_name("body").is_none() {
        if let Some(first) = list.iter().find(|n| n.kind() == "namespace_use_clause") {
            default = domain(*first, &default);
        }
    }
    if let Some(group) = list.iter().find(|n| n.kind() == "namespace_use_group") {
        let prefix = list
            .iter()
            .find(|n| n.kind() == "namespace_name")
            .and_then(|n| plain(text(*n, source)));
        if let Some(prefix) = prefix {
            for clause in children(*group)
                .into_iter()
                .filter(|n| n.kind() == "namespace_use_clause")
            {
                if let Some(import) = import_clause(clause, source, &prefix, &default) {
                    if default != "class" && import.kind != default {
                        scope.complete = false;
                    }
                    scope.imports.push(import);
                } else {
                    scope.complete = false;
                }
            }
        } else {
            scope.complete = false;
        }
    } else {
        for clause in list
            .into_iter()
            .filter(|n| n.kind() == "namespace_use_clause")
        {
            if let Some(import) = import_clause(clause, source, "", &default) {
                if default != "class" && import.kind != default {
                    scope.complete = false;
                }
                scope.imports.push(import);
            } else {
                scope.complete = false;
            }
        }
    }
    if scope.imports.len() > 1024 {
        scope.imports.truncate(1024);
        scope.complete = false;
    }
    let mut keys = HashSet::new();
    if scope.imports.iter().any(|i| {
        !keys.insert((
            i.kind.clone(),
            if i.kind == "const" {
                i.alias.clone()
            } else {
                i.alias.to_ascii_lowercase()
            },
        ))
    }) {
        scope.complete = false;
    }
}
fn conditional(node: Node) -> bool {
    let mut p = node.parent();
    while let Some(n) = p {
        if matches!(
            n.kind(),
            "function_definition"
                | "method_declaration"
                | "anonymous_function"
                | "arrow_function"
                | "if_statement"
                | "while_statement"
                | "for_statement"
                | "foreach_statement"
                | "try_statement"
                | "switch_statement"
        ) {
            return true;
        }
        if n.kind() == "compound_statement"
            && n.parent()
                .is_none_or(|p| p.kind() != "namespace_definition")
        {
            return true;
        }
        p = n.parent();
    }
    false
}
fn owner(node: Node, ids: &HashMap<usize, String>) -> (Option<String>, Option<String>, bool) {
    let mut n = node.parent();
    let mut routine = None;
    let mut instance = false;
    let mut class_id = None;
    while let Some(p) = n {
        if matches!(
            p.kind(),
            "anonymous_function" | "arrow_function" | "function_definition"
        ) {
            if routine.is_none() {
                routine = ids.get(&p.start_byte()).cloned();
            }
            break;
        }
        if p.kind() == "method_declaration" && routine.is_none() {
            routine = ids.get(&p.start_byte()).cloned();
            instance = !modifier(p, "static_modifier");
        }
        if class(p.kind()) {
            class_id = ids.get(&p.start_byte()).cloned();
            break;
        }
        if p.kind() == "object_creation_expression" {
            break;
        }
        n = p.parent();
    }
    (routine, class_id, instance)
}
pub(super) fn finish(root: Node, source: &str, id: &str, out: &mut IndexedDocument) {
    if !supported(&out.path) {
        return;
    }
    out.binding_limitations
        .retain(|s| s != "PHP_NAMESPACE_BINDING_UNMODELED");
    let mut facts = Facts {
        format: 6,
        complete: !out.parse_errors,
        scopes: vec![Scope {
            start: 0,
            end: source.len(),
            name: String::new(),
            imports: Vec::new(),
            complete: true,
        }],
        definitions: Vec::new(),
        uses: Vec::new(),
    };
    if source.len() > MAX_SOURCE {
        facts.complete = false;
        out.binding_limitations
            .push("PHP_NAME_CAPTURE_SOURCE_LIMIT".into());
        out.references.clear();
        for s in &mut out.symbols {
            s.qualified_name = format!("<php-name-unavailable@{}>::{}", s.start_byte, s.name);
        }
        out.php_names = Some(facts);
        return;
    }
    let top = children(root);
    for (i, n) in top
        .iter()
        .enumerate()
        .filter(|(_, n)| n.kind() == "namespace_definition")
    {
        let raw = n
            .child_by_field_name("name")
            .map(|n| text(n, source))
            .unwrap_or("");
        let name = if raw.is_empty() {
            Some(String::new())
        } else {
            plain(raw)
        };
        let end = n
            .child_by_field_name("body")
            .map(|n| n.end_byte())
            .unwrap_or_else(|| {
                top.iter()
                    .skip(i + 1)
                    .find(|n| n.kind() == "namespace_definition")
                    .map_or(source.len(), |n| n.start_byte())
            });
        facts.scopes.push(Scope {
            start: n.start_byte(),
            end,
            name: name.clone().unwrap_or_default(),
            imports: Vec::new(),
            complete: name.is_some(),
        });
    }
    if facts.scopes.len() > 128 {
        facts.complete = false;
        facts.scopes.truncate(128);
    }
    let scope_at = |at: usize, scopes: &[Scope]| {
        scopes
            .iter()
            .enumerate()
            .filter(|(_, s)| s.start <= at && at < s.end)
            .max_by_key(|(_, s)| s.start)
            .map_or(0, |(i, _)| i)
    };
    let mut nodes = Vec::new();
    let mut stack = vec![(root, 0)];
    while let Some((node, depth)) = stack.pop() {
        if nodes.len() >= MAX_NODES || depth >= 256 {
            facts.complete = false;
            break;
        }
        nodes.push(node);
        stack.extend(children(node).into_iter().rev().map(|n| (n, depth + 1)));
    }
    for n in &nodes {
        if n.kind() == "namespace_definition" && n.parent().is_some_and(|p| p.kind() != "program") {
            let i = scope_at(n.start_byte(), &facts.scopes);
            facts.scopes[i].complete = false;
        }
        if n.kind() == "namespace_use_declaration" {
            let i = scope_at(n.start_byte(), &facts.scopes);
            let top = n.parent().is_some_and(|p| {
                p.kind() == "program"
                    || p.kind() == "compound_statement"
                        && p.parent()
                            .is_some_and(|p| p.kind() == "namespace_definition")
            });
            if top {
                imports(*n, source, &mut facts.scopes[i]);
            } else {
                facts.scopes[i].complete = false;
            }
        }
    }
    let ids: HashMap<_, _> = out
        .symbols
        .iter()
        .map(|s| (s.start_byte, s.id.clone()))
        .collect();
    let by_id: HashMap<_, _> = out
        .symbols
        .iter()
        .map(|s| (s.id.clone(), s.name.clone()))
        .collect();
    let node_map: HashMap<_, _> = nodes
        .iter()
        .map(|n| ((n.start_byte(), n.kind()), *n))
        .collect();
    for symbol in &mut out.symbols {
        let Some(node) = node_map.get(&(symbol.start_byte, symbol.kind.as_str())) else {
            continue;
        };
        let i = scope_at(node.start_byte(), &facts.scopes);
        let namespace = &facts.scopes[i].name;
        let (_, owner, _) = owner(*node, &ids);
        let kind = if class(node.kind()) {
            "class"
        } else if node.kind() == "function_definition" {
            "function"
        } else if node.kind() == "method_declaration" {
            "method"
        } else {
            continue;
        };
        let fqn = if kind == "method" {
            let class = owner
                .as_ref()
                .and_then(|id| by_id.get(id))
                .cloned()
                .unwrap_or_else(|| format!("<anonymous@{}>", node.start_byte()));
            format!("{}::{}", qualify(namespace, &class), symbol.name)
        } else {
            qualify(namespace, &symbol.name)
        };
        symbol.qualified_name = fqn.clone();
        let visibility = children(*node)
            .into_iter()
            .find(|n| n.kind() == "visibility_modifier")
            .map(|n| text(n, source).to_ascii_lowercase())
            .unwrap_or_else(|| "public".into());
        let (test_attributes, test_attributes_complete) = super::phpunit::attributes(*node, source);
        facts.definitions.push(Definition {
            declaration_kind: node.kind().into(),
            scope: i,
            id: symbol.id.clone(),
            fqn,
            kind: kind.into(),
            owner: if kind == "method" { owner } else { None },
            conditional: conditional(*node),
            is_static: modifier(*node, "static_modifier"),
            visibility,
            has_body: node.child_by_field_name("body").is_some(),
            bases: children(*node)
                .into_iter()
                .filter(|n| matches!(n.kind(), "base_clause" | "class_interface_clause"))
                .flat_map(|clause| {
                    children(clause)
                        .into_iter()
                        .filter(|n| matches!(n.kind(), "name" | "qualified_name" | "relative_name"))
                        .map(move |name| Base {
                            expression: text(name, source).into(),
                            line: name.start_position().row + 1,
                            relation: if clause.kind() == "base_clause" {
                                "extends"
                            } else {
                                "implements"
                            }
                            .into(),
                        })
                })
                .collect(),
            uses_traits: node
                .child_by_field_name("body")
                .is_some_and(|body| children(body).iter().any(|n| n.kind() == "use_declaration")),
            is_final: modifier(*node, "final_modifier"),
            trait_uses: super::traits::capture(*node, source),
            is_abstract: modifier(*node, "abstract_modifier"),
            test_attributes,
            test_attributes_complete,
        });
    }
    for node in top
        .iter()
        .filter(|n| n.kind() == "namespace_definition")
        .take(128)
    {
        let name = if let Some(n) = node.child_by_field_name("name") {
            let Some(name) = plain(text(n, source)) else {
                continue;
            };
            name
        } else {
            "(global)".into()
        };
        out.symbols.push(Symbol {
            id: format!("{id}:php-namespace:{}", node.start_byte()),
            document_id: id.into(),
            path: out.path.clone(),
            name: name.clone(),
            qualified_name: name,
            kind: "namespace_definition".into(),
            signature: source[node.start_byte()
                ..node
                    .child_by_field_name("body")
                    .map_or(node.end_byte(), |n| n.start_byte())]
                .trim()
                .to_string(),
            documentation: None,
            documentation_examined: false,
            parameter_contracts: None,
            start_line: node.start_position().row + 1,
            end_line: node.end_position().row + 1,
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            revision: out.revision,
            content_hash: out.hash.clone(),
            build_context: None,
        });
    }
    for node in nodes {
        let type_nodes: Vec<_> = match node.kind() {
            "scoped_call_expression" => node
                .child_by_field_name("scope")
                .filter(|n| matches!(n.kind(), "name" | "qualified_name" | "relative_name"))
                .into_iter()
                .collect(),
            "class_constant_access_expression" => children(node)
                .first()
                .copied()
                .filter(|n| matches!(n.kind(), "name" | "qualified_name" | "relative_name"))
                .into_iter()
                .collect(),
            "named_type"
            | "attribute"
            | "base_clause"
            | "class_interface_clause"
            | "use_declaration" => children(node)
                .into_iter()
                .filter(|n| matches!(n.kind(), "name" | "qualified_name" | "relative_name"))
                .collect(),
            _ => Vec::new(),
        };
        for n in type_nodes {
            if facts.uses.len() >= MAX_USES {
                facts.complete = false;
                break;
            }
            if plain(text(n, source)).is_none() {
                continue;
            }
            let (caller, owner, instance) = owner(node, &ids);
            let target = text(n, source).to_string();
            facts.uses.push(Use {
                reference: Reference {
                    build_context: None,
                    start_byte: n.start_byte(),
                    document_id: id.into(),
                    path: out.path.clone(),
                    name: target.rsplit('\\').next().unwrap().into(),
                    expression: target.clone(),
                    line: n.start_position().row + 1,
                    column: n.start_position().column + 1,
                    caller,
                    is_call: false,
                    revision: out.revision,
                    content_hash: out.hash.clone(),
                },
                scope: scope_at(n.start_byte(), &facts.scopes),
                kind: "type".into(),
                target,
                owner,
                instance,
            });
        }
        if facts.uses.len() >= MAX_USES {
            facts.complete = false;
            break;
        }
        let (kind, target, name, expression, at) = match node.kind() {
            "function_call_expression" => {
                let Some(n) = node.child_by_field_name("function") else {
                    continue;
                };
                let valid = matches!(n.kind(), "name" | "qualified_name" | "relative_name");
                let target = if valid {
                    plain(text(n, source)).map(|_| text(n, source).to_string())
                } else {
                    None
                };
                let raw = target
                    .clone()
                    .unwrap_or_else(|| "<dynamic_php_function>".into());
                (
                    "function",
                    raw.clone(),
                    raw.rsplit('\\').next().unwrap().to_string(),
                    raw,
                    node,
                )
            }
            "scoped_call_expression" => {
                let Some(n) = node.child_by_field_name("name") else {
                    continue;
                };
                let Some(scope) = node.child_by_field_name("scope") else {
                    continue;
                };
                let name = if n.kind() == "name" {
                    plain(text(n, source))
                } else {
                    None
                };
                let target = if matches!(
                    scope.kind(),
                    "name" | "qualified_name" | "relative_name" | "relative_scope"
                ) {
                    plain(text(scope, source)).map(|_| text(scope, source).to_string())
                } else {
                    None
                };
                let name = name.unwrap_or_else(|| "<dynamic>".into());
                let target = target.unwrap_or_else(|| "<dynamic>".into());
                (
                    "static",
                    target.clone(),
                    name.clone(),
                    format!("{target}::{name}"),
                    node,
                )
            }
            "member_call_expression" | "nullsafe_member_call_expression" => {
                let Some(n) = node.child_by_field_name("name") else {
                    continue;
                };
                let obj = node.child_by_field_name("object");
                let target = if obj
                    .is_some_and(|n| n.kind() == "variable_name" && text(n, source) == "$this")
                {
                    "$this"
                } else {
                    "<dynamic>"
                };
                let name = if n.kind() == "name" {
                    plain(text(n, source))
                } else {
                    None
                };
                let name = name.unwrap_or_else(|| "<dynamic>".into());
                (
                    "member",
                    target.into(),
                    name.clone(),
                    format!("{target}->{name}"),
                    node,
                )
            }
            "object_creation_expression" => {
                let target = children(node)
                    .into_iter()
                    .find(|n| matches!(n.kind(), "name" | "qualified_name" | "relative_name"));
                let Some(n) = target else {
                    continue;
                };
                let Some(raw) = plain(text(n, source)).map(|_| text(n, source).to_string()) else {
                    continue;
                };
                (
                    "type",
                    raw.clone(),
                    raw.rsplit('\\').next().unwrap().into(),
                    raw,
                    n,
                )
            }
            _ => continue,
        };
        let (caller, owner, instance) = owner(node, &ids);
        let placeholder = node.child_by_field_name("arguments").is_some_and(|a| {
            children(a)
                .iter()
                .any(|n| n.kind() == "variadic_placeholder")
        });
        let unsupported_nullsafe = placeholder && node.kind() == "nullsafe_member_call_expression";
        let reference = Reference {
            build_context: None,
            start_byte: at.start_byte(),
            document_id: id.into(),
            path: out.path.clone(),
            name,
            expression,
            line: at.start_position().row + 1,
            column: at.start_position().column + 1,
            caller,
            is_call: kind != "type" && !placeholder,
            revision: out.revision,
            content_hash: out.hash.clone(),
        };
        if facts.uses.len() >= MAX_USES {
            facts.complete = false;
            break;
        }
        facts.uses.push(Use {
            reference,
            scope: scope_at(node.start_byte(), &facts.scopes),
            kind: if unsupported_nullsafe {
                "unsupported".into()
            } else {
                kind.into()
            },
            target,
            owner,
            instance,
        });
    }
    facts.uses.sort_by_key(|u| u.reference.start_byte);
    out.references = facts.uses.iter().map(|u| u.reference.clone()).collect();
    if !facts.complete || facts.scopes.iter().any(|s| !s.complete) {
        out.binding_limitations
            .push("PHP_NAME_CAPTURE_INCOMPLETE".into());
    }
    if !facts.complete {
        for s in &mut out.symbols {
            s.qualified_name = format!("<php-name-unavailable@{}>::{}", s.start_byte, s.name);
        }
    }
    out.php_names = Some(facts);
}
