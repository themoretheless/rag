//! Source-only Java binding. Arity narrows overload candidates; it does not run javac.
use super::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Facts {
    package: String,
    imports: Vec<Import>,
    types: Vec<Type>,
    methods: Vec<Method>,
    bindings: Vec<Binding>,
    sites: Vec<Site>,
    opaque: Vec<(usize, usize)>,
    type_parameters: Vec<(String, usize, usize)>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Import {
    name: String,
    wildcard: bool,
    static_: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Type {
    name: String,
    start: usize,
    end: usize,
    kind: String,
    bases: Vec<Base>,
    parent: Option<String>,
    local_scope: Option<(usize, usize)>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Base {
    name: String,
    relation: String,
    line: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Method {
    start: usize,
    owner: String,
    name: String,
    params: Vec<String>,
    varargs: bool,
    constructor: bool,
    static_: bool,
    private: bool,
    public: bool,
    protected: bool,
    test: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Binding {
    name: String,
    ty: Option<String>,
    owner: String,
    field: bool,
    start: usize,
    end: usize,
    declared: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Site {
    start: usize,
    owner: String,
    name: String,
    object: Option<String>,
    arity: Option<usize>,
    kind: String,
}
fn type_node(node: Node) -> bool {
    matches!(
        node.kind(),
        "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration"
            | "annotation_type_declaration"
    )
}
fn method_node(node: Node) -> bool {
    matches!(
        node.kind(),
        "method_declaration" | "constructor_declaration" | "compact_constructor_declaration"
    )
}
fn qualified(node: Node, source: &str) -> String {
    let mut current = Some(node);
    let mut names = vec![];
    while let Some(n) = current {
        if definition(n) {
            if let Some(name) = n.child_by_field_name("name") {
                names.push(text(name, source));
            }
        }
        current = n.parent();
    }
    names.reverse();
    names.join("::")
}
fn owner(node: Node, source: &str) -> String {
    let mut current = node.parent();
    while let Some(n) = current {
        if type_node(n) {
            return qualified(n, source);
        }
        current = n.parent();
    }
    String::new()
}
fn scope(node: Node, parameter: bool) -> (usize, usize) {
    let mut current = node.parent();
    while let Some(n) = current {
        if (!parameter
            && matches!(
                n.kind(),
                "block"
                    | "for_statement"
                    | "enhanced_for_statement"
                    | "try_with_resources_statement"
            ))
            || (parameter && method_node(n))
        {
            return (n.start_byte(), n.end_byte());
        }
        current = n.parent();
    }
    (0, usize::MAX)
}
fn qualified_path(node: Node, source: &str) -> Option<String> {
    let mut stack = vec![node];
    let mut parts = vec![];
    let mut budget = 256;
    while let Some(n) = stack.pop() {
        if budget == 0 {
            return None;
        }
        budget -= 1;
        match n.kind() {
            "identifier" | "type_identifier" | "this" | "super" => {
                parts.push((n.start_byte(), text(n, source)))
            }
            "scoped_identifier" | "scoped_type_identifier" | "field_access" => {
                let mut c = n.walk();
                stack.extend(n.named_children(&mut c));
            }
            "line_comment" | "block_comment" | "marker_annotation" | "annotation" => {}
            _ => return None,
        }
    }
    parts.sort_by_key(|p| p.0);
    (!parts.is_empty()).then(|| parts.into_iter().map(|p| p.1).collect::<Vec<_>>().join("."))
}
fn simple(node: Node, source: &str) -> Option<String> {
    if matches!(
        node.kind(),
        "type_identifier"
            | "scoped_type_identifier"
            | "identifier"
            | "scoped_identifier"
            | "integral_type"
            | "floating_point_type"
            | "boolean_type"
            | "void_type"
    ) {
        let value = qualified_path(node, source).unwrap_or_else(|| text(node, source).into());
        let s = value.as_str();
        if s.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '$' | '.'))
        {
            return Some(s.into());
        }
    }
    None
}
fn diag(out: &mut IndexedDocument, code: &str) {
    if !out.binding_limitations.iter().any(|s| s == code) {
        out.binding_limitations.push(code.into());
    }
}
fn opaque(node: Node) -> bool {
    let mut current = Some(node);
    while let Some(n) = current {
        if anonymous_body(n) || n.kind() == "lambda_expression" {
            return true;
        }
        current = n.parent();
    }
    false
}
pub(super) fn anonymous_body(node: Node) -> bool {
    node.kind() == "class_body"
        && node
            .parent()
            .is_some_and(|n| matches!(n.kind(), "object_creation_expression" | "enum_constant"))
}
fn modifiers(node: Node, source: &str) -> String {
    let mut c = node.walk();
    let result = node
        .named_children(&mut c)
        .find(|n| n.kind() == "modifiers")
        .map(|n| text(n, source).into())
        .unwrap_or_default();
    result
}
fn modifier(node: Node, name: &str) -> bool {
    let mut cursor = node.walk();
    let modifiers = node
        .named_children(&mut cursor)
        .find(|n| n.kind() == "modifiers");
    let Some(modifiers) = modifiers else {
        return false;
    };
    let mut cursor = modifiers.walk();
    let found = modifiers.children(&mut cursor).any(|n| n.kind() == name);
    found
}
fn args(node: Node) -> Option<usize> {
    let a = node.child_by_field_name("arguments")?;
    let mut c = a.walk();
    Some(
        a.named_children(&mut c)
            .filter(|n| !matches!(n.kind(), "line_comment" | "block_comment"))
            .count(),
    )
}
fn type_name(node: Node) -> Option<Node> {
    if node.kind() == "generic_type" {
        let mut c = node.walk();
        let base = node
            .named_children(&mut c)
            .find(|n| matches!(n.kind(), "type_identifier" | "scoped_type_identifier"))?;
        last_identifier(base)
    } else {
        last_identifier(node)
    }
}
fn site<'a>(node: Node<'a>, source: &str) -> Option<(Site, Node<'a>)> {
    let kind = node.kind();
    let (name, object) = match kind {
        "method_invocation" => (
            node.child_by_field_name("name")?,
            node.child_by_field_name("object")
                .map(|n| qualified_path(n, source).unwrap_or_else(|| text(n, source).into())),
        ),
        "object_creation_expression" => {
            let ty = node.child_by_field_name("type")?;
            let name = type_name(ty)?;
            (name, Some(text(ty, source).into()))
        }
        "explicit_constructor_invocation" => (node.child_by_field_name("constructor")?, None),
        "method_reference" => {
            let mut c = node.walk();
            let children: Vec<_> = node.named_children(&mut c).collect();
            let mut c = node.walk();
            let keyword = node.children(&mut c).find(|n| n.kind() == "new");
            let name = if let Some(keyword) = keyword {
                keyword
            } else {
                last_identifier(*children.last()?)?
            };
            (name, Some(text(*children.first()?, source).into()))
        }
        _ => return None,
    };
    Some((
        Site {
            start: node.start_byte(),
            owner: owner(node, source),
            name: text(name, source).into(),
            object,
            arity: args(node),
            kind: if kind == "method_reference" && text(name, source) == "new" {
                "constructor_reference"
            } else {
                kind
            }
            .into(),
        },
        name,
    ))
}
pub(super) fn collect(node: Node, source: &str, out: &mut IndexedDocument) {
    if node.kind() == "program" {
        diag(out, "JAVA_CLASSPATH_AND_RUNTIME_DISPATCH_UNVERIFIED");
    }
    if node.kind() == "package_declaration" {
        let mut c = node.walk();
        out.java.as_mut().unwrap().package = node
            .named_children(&mut c)
            .find(|n| matches!(n.kind(), "identifier" | "scoped_identifier"))
            .and_then(|n| qualified_path(n, source))
            .unwrap_or_default();
    }
    if node.kind() == "import_declaration" {
        let mut c = node.walk();
        let children: Vec<_> = node.named_children(&mut c).collect();
        let path = children
            .iter()
            .find(|n| matches!(n.kind(), "identifier" | "scoped_identifier"))
            .and_then(|n| qualified_path(*n, source));
        let mut c = node.walk();
        let static_ = node.children(&mut c).any(|n| n.kind() == "static");
        if let Some(name) = path {
            out.java.as_mut().unwrap().imports.push(Import {
                name,
                wildcard: children.iter().any(|n| n.kind() == "asterisk"),
                static_,
            });
        } else {
            diag(out, "JAVA_IMPORT_SYNTAX_UNVERIFIED");
        }
    }
    if anonymous_body(node) || node.kind() == "lambda_expression" {
        out.java
            .as_mut()
            .unwrap()
            .opaque
            .push((node.start_byte(), node.end_byte()));
        diag(out, "JAVA_ANONYMOUS_OR_LAMBDA_OWNER_UNMODELED");
    }
    if let Some((s, _)) = site(node, source) {
        out.java.as_mut().unwrap().sites.push(s);
    }
    if opaque(node) {
        return;
    }
    if type_node(node) {
        let mut bases = vec![];
        if let Some(base) = node
            .child_by_field_name("superclass")
            .and_then(|n| n.named_child(0))
        {
            bases.push(Base {
                name: simple(base, source).unwrap_or_else(|| text(base, source).into()),
                relation: "extends".into(),
                line: base.start_position().row + 1,
            });
        }
        let mut cursor = node.walk();
        for holder in node
            .named_children(&mut cursor)
            .filter(|n| matches!(n.kind(), "super_interfaces" | "extends_interfaces"))
        {
            if let Some(list) = holder.named_child(0) {
                let mut c = list.walk();
                for base in list.named_children(&mut c) {
                    bases.push(Base {
                        name: simple(base, source).unwrap_or_else(|| text(base, source).into()),
                        relation: if node.kind() == "interface_declaration" {
                            "extends"
                        } else {
                            "implements"
                        }
                        .into(),
                        line: base.start_position().row + 1,
                    });
                }
            }
        }
        let local_scope = match scope(node, false) {
            (0, _) => None,
            p => Some(p),
        };
        out.java.as_mut().unwrap().types.push(Type {
            name: qualified(node, source),
            start: node.start_byte(),
            end: node.end_byte(),
            kind: node.kind().into(),
            bases,
            parent: (!owner(node, source).is_empty()).then(|| owner(node, source)),
            local_scope,
        });
    }
    if method_node(node) {
        let raw = modifiers(node, source);
        let mut params = vec![];
        let mut varargs = false;
        if let Some(parameters) = node.child_by_field_name("parameters").or_else(|| {
            node.parent()
                .and_then(|p| p.parent())
                .filter(|p| p.kind() == "record_declaration")
                .and_then(|p| p.child_by_field_name("parameters"))
        }) {
            let mut c = parameters.walk();
            for p in parameters
                .named_children(&mut c)
                .filter(|n| matches!(n.kind(), "formal_parameter" | "spread_parameter"))
            {
                if p.kind() == "spread_parameter" {
                    varargs = true;
                }
                params.push(
                    p.child_by_field_name("type")
                        .and_then(|n| simple(n, source))
                        .unwrap_or_else(|| format!("<unverified@{}>", p.start_byte())),
                );
            }
        }
        let name = node
            .child_by_field_name("name")
            .map(|n| text(n, source))
            .unwrap_or("");
        out.java.as_mut().unwrap().methods.push(Method {
            start: node.start_byte(),
            owner: owner(node, source),
            name: name.into(),
            params,
            varargs,
            constructor: node.kind() != "method_declaration",
            static_: modifier(node, "static"),
            private: modifier(node, "private"),
            public: modifier(node, "public"),
            protected: modifier(node, "protected"),
            test: raw.contains("@Test") || raw.contains("@org.junit.Test"),
        });
    }
    if node.kind() == "type_parameter" {
        let mut c = node.walk();
        let type_name = node
            .named_children(&mut c)
            .find(|n| n.kind() == "type_identifier");
        if let Some(name) = type_name {
            let mut p = node.parent();
            while let Some(n) = p {
                if type_node(n) || method_node(n) {
                    out.java.as_mut().unwrap().type_parameters.push((
                        text(name, source).into(),
                        n.start_byte(),
                        n.end_byte(),
                    ));
                    break;
                }
                p = n.parent();
            }
        }
    }
    if matches!(
        node.kind(),
        "formal_parameter"
            | "variable_declarator"
            | "catch_formal_parameter"
            | "enhanced_for_statement"
            | "resource"
    ) {
        if let Some(name) = node.child_by_field_name("name") {
            let spread = node
                .parent()
                .is_some_and(|p| p.kind() == "spread_parameter");
            let parameter = node.kind() == "formal_parameter" || spread;
            let declaration = if spread {
                node.parent().unwrap()
            } else if parameter || node.kind() != "variable_declarator" {
                node
            } else {
                node.parent().unwrap_or(node)
            };
            let record_component = parameter
                && node
                    .parent()
                    .and_then(|p| p.parent())
                    .is_some_and(|p| p.kind() == "record_declaration");
            let field = declaration.kind() == "field_declaration" || record_component;
            let ty = declaration
                .child_by_field_name("type")
                .and_then(|n| simple(n, source))
                .filter(|t| t != "var")
                .or_else(|| {
                    node.child_by_field_name("value")
                        .filter(|v| v.kind() == "object_creation_expression")
                        .and_then(|v| v.child_by_field_name("type"))
                        .and_then(|n| simple(n, source))
                });
            let (start, end) = if node.kind() == "enhanced_for_statement" {
                (
                    node.child_by_field_name("body")
                        .map_or(node.end_byte(), |b| b.start_byte()),
                    node.end_byte(),
                )
            } else if field {
                (0, usize::MAX)
            } else {
                scope(node, parameter)
            };
            out.java.as_mut().unwrap().bindings.push(Binding {
                name: text(name, source).into(),
                ty,
                owner: owner(node, source),
                field,
                start,
                end,
                declared: if parameter || field {
                    start
                } else {
                    name.end_byte()
                },
            });
        }
    }
}
pub(super) fn push_call(
    node: Node,
    source: &str,
    doc_id: &str,
    out: &mut IndexedDocument,
    caller: Option<&str>,
) {
    if let Some((site, name)) = site(node, source) {
        if matches!(
            site.kind.as_str(),
            "method_reference" | "constructor_reference"
        ) {
            return;
        }
        let expression = if site.kind == "object_creation_expression" {
            format!("new {}", site.object.as_deref().unwrap_or(""))
        } else if let Some(object) = &site.object {
            format!("{object}.{}", site.name)
        } else {
            site.name.clone()
        };
        out.references.push(Reference {
            start_byte: node.start_byte(),
            document_id: doc_id.into(),
            path: out.path.clone(),
            name: site.name,
            expression,
            line: name.start_position().row + 1,
            column: name.start_position().column + 1,
            caller: caller.map(str::to_owned),
            is_call: true,
            revision: out.revision,
            content_hash: out.hash.clone(),
        });
    }
}
pub(super) fn reference_occurrence<'a>(node: Node<'a>, source: &str) -> Option<(Node<'a>, String)> {
    site(node, source).map(|(_, name)| (name, text(node, source).into()))
}
pub(super) fn non_usage(node: Node) -> bool {
    let mut current = node.parent();
    while let Some(p) = current {
        if matches!(p.kind(), "package_declaration" | "import_declaration") {
            return true;
        }
        if matches!(
            p.kind(),
            "formal_parameter"
                | "variable_declarator"
                | "method_invocation"
                | "catch_formal_parameter"
                | "enhanced_for_statement"
                | "resource"
        ) && p
            .child_by_field_name("name")
            .is_some_and(|n| n.id() == node.id())
        {
            return true;
        }
        if p.kind() == "method_reference" && last_identifier(p).is_some_and(|n| n.id() == node.id())
        {
            let mut cursor = p.walk();
            let constructor = p.children(&mut cursor).any(|n| n.kind() == "new");
            if !constructor {
                return true;
            }
        }
        current = p.parent();
    }
    false
}
#[derive(Clone, Copy)]
struct Target<'a> {
    doc: &'a IndexedDocument,
    ty: &'a Type,
}
fn same_doc(a: &IndexedDocument, b: &IndexedDocument) -> bool {
    a.symbols
        .first()
        .zip(b.symbols.first())
        .is_some_and(|(a, b)| a.document_id == b.document_id)
}
fn fqn(target: Target) -> String {
    let name = target.ty.name.replace("::", ".");
    if target.doc.java.as_ref().unwrap().package.is_empty() {
        name
    } else {
        format!("{}.{name}", target.doc.java.as_ref().unwrap().package)
    }
}
fn all_types<'a>(
    docs: &'a HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Vec<Target<'a>> {
    docs.values()
        .filter(|d| d.project == doc.project && !d.parse_errors)
        .filter_map(|d| d.java.as_ref().map(|j| (d, j)))
        .flat_map(|(d, j)| j.types.iter().map(move |t| Target { doc: d, ty: t }))
        .collect()
}
fn type_lookup<'a>(
    docs: &'a HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
    name: &str,
    at: usize,
    owner: &str,
) -> Vec<Target<'a>> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '$' | '.'))
    {
        return vec![];
    }
    let facts = doc.java.as_ref().unwrap();
    if facts
        .type_parameters
        .iter()
        .any(|(n, s, e)| n == name && *s <= at && at < *e)
    {
        return vec![];
    }
    let all = all_types(docs, doc);
    if name.contains('.') {
        let exact: Vec<_> = all
            .iter()
            .copied()
            .filter(|t| t.ty.local_scope.is_none() && fqn(*t) == name)
            .collect();
        if !exact.is_empty() {
            return exact;
        }
    }
    let local: Vec<_> = all
        .iter()
        .copied()
        .filter(|t| {
            same_doc(t.doc, doc)
                && t.ty.name.rsplit("::").next() == Some(name)
                && t.ty
                    .local_scope
                    .is_some_and(|(s, e)| s <= at && at < e && t.ty.start <= at)
        })
        .collect();
    if !local.is_empty() {
        let nearest = local
            .iter()
            .map(|t| t.ty.local_scope.unwrap().0)
            .max()
            .unwrap();
        return local
            .into_iter()
            .filter(|t| t.ty.local_scope.unwrap().0 == nearest)
            .collect();
    }
    let mut prefix = Some(owner);
    while let Some(p) = prefix {
        for q in [format!("{p}::{name}"), p.to_owned()] {
            let found: Vec<_> = all
                .iter()
                .copied()
                .filter(|t| {
                    same_doc(t.doc, doc)
                        && t.ty.name == q
                        && (q.ends_with(&format!("::{name}")) || q == name)
                })
                .collect();
            if !found.is_empty() {
                return found;
            }
        }
        prefix = p.rsplit_once("::").map(|p| p.0);
    }
    let head = name.split('.').next().unwrap();
    let imports: Vec<_> = facts
        .imports
        .iter()
        .filter(|i| !i.static_ && !i.wildcard && i.name.rsplit('.').next() == Some(head))
        .collect();
    if !imports.is_empty() {
        return imports
            .into_iter()
            .flat_map(|i| {
                let full = format!("{}{}", i.name, &name[head.len()..]);
                all.iter()
                    .copied()
                    .filter(move |t| t.ty.local_scope.is_none() && fqn(*t) == full)
            })
            .collect();
    }
    let full = if facts.package.is_empty() {
        name.to_owned()
    } else {
        format!("{}.{name}", facts.package)
    };
    let package: Vec<_> = all
        .iter()
        .copied()
        .filter(|t| t.ty.local_scope.is_none() && fqn(*t) == full)
        .collect();
    if !package.is_empty() {
        return package;
    }
    facts
        .imports
        .iter()
        .filter(|i| !i.static_ && i.wildcard)
        .map(|i| i.name.as_str())
        .chain(std::iter::once("java.lang"))
        .flat_map(|package| {
            let full = format!("{package}.{name}");
            all.iter()
                .copied()
                .filter(move |t| t.ty.local_scope.is_none() && fqn(*t) == full)
        })
        .collect()
}
fn symbol<'a>(symbols: &'a [Symbol], target: Target) -> Option<&'a Symbol> {
    target
        .doc
        .symbols
        .iter()
        .find(|s| s.start_byte == target.ty.start)
        .and_then(|s| symbols.iter().find(|item| item.id == s.id))
}
fn method_symbol<'a>(
    symbols: &'a [Symbol],
    doc: &IndexedDocument,
    m: &Method,
) -> Option<&'a Symbol> {
    doc.symbols
        .iter()
        .find(|s| s.start_byte == m.start)
        .and_then(|s| symbols.iter().find(|item| item.id == s.id))
}
fn owner_target<'a>(
    docs: &'a HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
    owner: &str,
) -> Option<Target<'a>> {
    all_types(docs, doc)
        .into_iter()
        .find(|t| same_doc(t.doc, doc) && t.ty.name == owner)
}
fn arity(m: &Method, n: Option<usize>) -> bool {
    n.is_none_or(|n| {
        if m.varargs {
            n >= m.params.len().saturating_sub(1)
        } else {
            n == m.params.len()
        }
    })
}
fn signature(docs: &HashMap<String, IndexedDocument>, doc: &IndexedDocument, m: &Method) -> String {
    m.params
        .iter()
        .map(|p| {
            if matches!(
                p.as_str(),
                "byte" | "short" | "int" | "long" | "float" | "double" | "char" | "boolean"
            ) {
                p.clone()
            } else {
                let types = type_lookup(docs, doc, p, m.start, &m.owner);
                if types.len() == 1 {
                    fqn(types[0])
                } else {
                    format!("<unknown@{}:{}:{p}>", doc.path, m.start)
                }
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}
struct Methods<'a> {
    values: Vec<(&'a IndexedDocument, &'a Method)>,
    complete: bool,
}
fn methods<'a>(
    docs: &'a HashMap<String, IndexedDocument>,
    ty: Target<'a>,
    name: &str,
    n: Option<usize>,
    constructor: bool,
    static_only: bool,
    stack: &mut HashSet<String>,
    budget: &mut usize,
) -> Methods<'a> {
    let key = format!("{}:{}", ty.doc.path, ty.ty.start);
    if *budget == 0 || !stack.insert(key.clone()) {
        return Methods {
            values: vec![],
            complete: false,
        };
    }
    *budget -= 1;
    let own: Vec<_> = ty
        .doc
        .java
        .as_ref()
        .unwrap()
        .methods
        .iter()
        .filter(|m| {
            m.owner == ty.ty.name
                && m.name == name
                && m.constructor == constructor
                && arity(m, n)
                && (!static_only || m.static_)
        })
        .map(|m| (ty.doc, m))
        .collect();
    let mut hidden: HashSet<_> = own.iter().map(|(d, m)| signature(docs, d, m)).collect();
    let mut out = Methods {
        values: own,
        complete: true,
    };
    if !constructor {
        for base in &ty.ty.bases {
            let bases = type_lookup(docs, ty.doc, &base.name, ty.ty.start, &ty.ty.name);
            if bases.is_empty() && matches!(base.name.as_str(), "Object" | "java.lang.Object") {
                continue;
            }
            if bases.len() != 1 {
                out.complete = false;
                continue;
            }
            let inherited = methods(docs, bases[0], name, n, false, static_only, stack, budget);
            out.complete &= inherited.complete;
            for (d, m) in inherited.values {
                if m.private || m.static_ && bases[0].ty.kind == "interface_declaration" {
                    continue;
                }
                let sig = signature(docs, d, m);
                if !hidden.contains(&sig) {
                    if bases[0].ty.kind != "interface_declaration" {
                        hidden.insert(sig);
                    }
                    out.values.push((d, m));
                }
            }
        }
    }
    // Keep unrelated interface declarations ambiguous; only this type's declarations hide bases.
    stack.remove(&key);
    out
}
fn binding<'a>(
    doc: &'a IndexedDocument,
    name: &str,
    at: usize,
    owner: &str,
    fields_only: bool,
) -> Option<&'a Binding> {
    doc.java
        .as_ref()?
        .bindings
        .iter()
        .filter(|b| {
            b.name == name
                && b.start <= at
                && at < b.end
                && b.declared <= at
                && (!b.field || b.owner == owner)
                && (!fields_only || b.field)
        })
        .max_by_key(|b| (!b.field, b.start, b.declared))
}
fn current_static(doc: &IndexedDocument, r: &Reference) -> bool {
    doc.java
        .as_ref()
        .unwrap()
        .methods
        .iter()
        .find(|m| {
            doc.symbols
                .iter()
                .any(|s| s.start_byte == m.start && Some(&s.id) == r.caller.as_ref())
        })
        .is_some_and(|m| m.static_)
}
fn accessible(
    doc: &IndexedDocument,
    owner: &str,
    target_doc: &IndexedDocument,
    m: &Method,
) -> bool {
    if m.private {
        return doc.java.as_ref().unwrap().package == target_doc.java.as_ref().unwrap().package
            && owner.split("::").next() == m.owner.split("::").next();
    }
    m.public
        || m.protected
        || doc.java.as_ref().unwrap().package == target_doc.java.as_ref().unwrap().package
        || target_doc
            .java
            .as_ref()
            .unwrap()
            .types
            .iter()
            .any(|t| t.name == m.owner && t.kind == "interface_declaration")
}
pub(super) fn resolve(
    symbols: &[Symbol],
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
    r: &Reference,
) -> CallEdge {
    let facts = doc.java.as_ref().unwrap();
    let mut found = vec![];
    let mut complete = true;
    let mut reason = "java_unresolved";
    if doc.parse_errors
        || facts
            .opaque
            .iter()
            .any(|(s, e)| *s <= r.start_byte && r.start_byte < *e)
    {
        reason = "java_parse_or_anonymous_owner_unverified";
    } else if let Some(site) = facts
        .sites
        .iter()
        .find(|s| s.start == r.start_byte && (r.is_call || r.expression.contains("::")))
    {
        let mut targets = vec![];
        let mut static_only = false;
        let mut constructor = false;
        let mut name = site.name.clone();
        if matches!(
            site.kind.as_str(),
            "object_creation_expression" | "constructor_reference"
        ) {
            constructor = true;
            targets = type_lookup(
                docs,
                doc,
                site.object.as_deref().unwrap_or(""),
                r.start_byte,
                &site.owner,
            );
            reason = "java_constructor";
        } else if site.kind == "explicit_constructor_invocation" {
            constructor = true;
            if let Some(current) = owner_target(docs, doc, &site.owner) {
                if site.name == "this" {
                    targets.push(current);
                } else {
                    for base in current.ty.bases.iter().filter(|b| b.relation == "extends") {
                        targets.extend(type_lookup(
                            docs,
                            doc,
                            &base.name,
                            r.start_byte,
                            &site.owner,
                        ));
                    }
                }
            }
            reason = "java_constructor_delegation";
        } else {
            match site.object.as_deref() {
                None | Some("this") => {
                    if let Some(current) = owner_target(docs, doc, &site.owner) {
                        targets.push(current);
                    }
                    static_only = current_static(doc, r);
                    reason = "java_lexical_method";
                }
                Some("super") => {
                    if !current_static(doc, r) {
                        if let Some(current) = owner_target(docs, doc, &site.owner) {
                            for b in current.ty.bases.iter().filter(|b| b.relation == "extends") {
                                targets.extend(type_lookup(
                                    docs,
                                    doc,
                                    &b.name,
                                    r.start_byte,
                                    &site.owner,
                                ));
                            }
                        }
                    }
                    reason = "java_super_method";
                }
                Some(object) => {
                    let field = object.strip_prefix("this.");
                    if let Some(b) = binding(
                        doc,
                        field.unwrap_or(object),
                        r.start_byte,
                        &site.owner,
                        field.is_some(),
                    ) {
                        if let Some(ty) = &b.ty {
                            targets = type_lookup(docs, doc, ty, r.start_byte, &site.owner);
                        }
                        reason = "java_receiver_type";
                    } else {
                        targets = type_lookup(docs, doc, object, r.start_byte, &site.owner);
                        static_only = site.kind != "method_reference";
                        reason = "java_static_type";
                    }
                }
            }
        }
        if targets.len() > 1 {
            complete = false;
        }
        for target in targets {
            if !constructor && target.ty.kind == "interface_declaration" {
                reason = "java_interface_contract";
            }
            if constructor {
                name = target.ty.name.rsplit("::").next().unwrap_or("").into();
            }
            let selected = methods(
                docs,
                target,
                &name,
                site.arity,
                constructor,
                static_only,
                &mut HashSet::new(),
                &mut 256,
            );
            complete &= selected.complete;
            for (d, m) in &selected.values {
                if accessible(doc, &site.owner, d, m) {
                    if let Some(s) = method_symbol(symbols, d, m) {
                        found.push(s);
                    }
                }
            }
            if constructor
                && selected.values.is_empty()
                && (site.arity == Some(0) || site.kind == "constructor_reference")
                && target.ty.kind == "class_declaration"
                && !target
                    .doc
                    .java
                    .as_ref()
                    .unwrap()
                    .methods
                    .iter()
                    .any(|m| m.owner == target.ty.name && m.constructor)
            {
                if let Some(s) = symbol(symbols, target) {
                    found.push(s);
                    reason = "java_implicit_constructor_type";
                }
            }
        }
        if found.is_empty()
            && complete
            && site.object.is_none()
            && !constructor
            && !matches!(
                site.name.as_str(),
                "equals"
                    | "hashCode"
                    | "toString"
                    | "getClass"
                    | "clone"
                    | "finalize"
                    | "notify"
                    | "notifyAll"
                    | "wait"
            )
        {
            for import in facts.imports.iter().filter(|i| i.static_) {
                let (class, member) = if import.wildcard {
                    (import.name.as_str(), site.name.as_str())
                } else {
                    import.name.rsplit_once('.').unwrap_or(("", ""))
                };
                if member != site.name {
                    continue;
                }
                for target in type_lookup(docs, doc, class, r.start_byte, &site.owner) {
                    let selected = methods(
                        docs,
                        target,
                        member,
                        site.arity,
                        false,
                        true,
                        &mut HashSet::new(),
                        &mut 256,
                    );
                    complete &= selected.complete;
                    for (d, m) in selected.values {
                        if accessible(doc, &site.owner, d, m) {
                            if let Some(s) = method_symbol(symbols, d, m) {
                                found.push(s);
                                reason = "java_static_import";
                            }
                        }
                    }
                }
            }
        }
    } else {
        let owner = doc
            .symbols
            .iter()
            .find(|s| Some(&s.id) == r.caller.as_ref())
            .map(|s| {
                s.qualified_name
                    .rsplit_once("::")
                    .map_or(s.qualified_name.as_str(), |p| p.0)
            })
            .unwrap_or("");
        if binding(doc, &r.expression, r.start_byte, owner, false).is_none() {
            for ty in type_lookup(docs, doc, &r.expression, r.start_byte, owner) {
                if let Some(s) = symbol(symbols, ty) {
                    found.push(s);
                }
            }
            reason = "java_type_reference";
        }
    }
    found.sort_by_key(|s| &s.id);
    found.dedup_by_key(|s| &s.id);
    CallEdge {
        reference: r.clone(),
        candidates: found.iter().map(|s| s.id.clone()).collect(),
        resolution: match found.len() {
            0 => "unresolved",
            1 if complete => "resolved_static",
            _ => "ambiguous",
        }
        .into(),
        provenance: "derived",
        resolution_basis: if !complete {
            "java_incomplete_type_or_hierarchy_candidates".into()
        } else {
            reason.into()
        },
    }
}
fn valid_hierarchy(
    docs: &HashMap<String, IndexedDocument>,
    ty: Target,
    stack: &mut HashSet<String>,
    budget: &mut usize,
) -> bool {
    let key = format!("{}:{}", ty.doc.path, ty.ty.start);
    if *budget == 0 || !stack.insert(key.clone()) {
        return false;
    }
    *budget -= 1;
    for b in &ty.ty.bases {
        let base = type_lookup(docs, ty.doc, &b.name, ty.ty.start, &ty.ty.name);
        if base.is_empty() && matches!(b.name.as_str(), "Object" | "java.lang.Object") {
            continue;
        }
        if base.len() != 1 || !valid_hierarchy(docs, base[0], stack, budget) {
            return false;
        }
    }
    stack.remove(&key);
    true
}
pub(super) fn edges(
    symbols: &[Symbol],
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Vec<inheritance::InheritanceEdge> {
    let mut out = vec![];
    for ty in &doc.java.as_ref().unwrap().types {
        let target = Target { doc, ty };
        let Some(derived) = symbol(symbols, target) else {
            continue;
        };
        let valid = valid_hierarchy(docs, target, &mut HashSet::new(), &mut 256);
        for b in &ty.bases {
            let targets = type_lookup(docs, doc, &b.name, ty.start, &ty.name);
            let ids: Vec<_> = targets
                .into_iter()
                .filter_map(|t| symbol(symbols, t).map(|s| s.id.clone()))
                .collect();
            out.push(inheritance::InheritanceEdge {
                derived: derived.id.clone(),
                resolution: match ids.len() {
                    0 => "unresolved",
                    1 => "resolved_static",
                    _ => "ambiguous",
                }
                .into(),
                candidates: ids,
                base_expression: b.name.clone(),
                path: doc.path.clone(),
                line: b.line,
                relation: b.relation.clone(),
                hierarchy_status: if valid {
                    "complete_static"
                } else {
                    "incomplete_or_invalid"
                }
                .into(),
                provenance: "derived",
            });
        }
    }
    out
}
pub(super) fn entrypoint(doc: &IndexedDocument, s: &Symbol) -> Option<&'static str> {
    let m = doc
        .java
        .as_ref()?
        .methods
        .iter()
        .find(|m| m.start == s.start_byte)?;
    if m.test {
        Some("java_test_candidate")
    } else if m.name == "main" && m.public && m.static_ {
        Some("conventional_main_candidate")
    } else if !m.constructor
        && (m.public
            || !m.private
                && doc
                    .java
                    .as_ref()?
                    .types
                    .iter()
                    .any(|t| t.name == m.owner && t.kind == "interface_declaration"))
    {
        Some("public_api")
    } else {
        None
    }
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
            out.symbols.extend(d.symbols.clone());
            out.references.extend(d.references.clone());
            docs.insert(i.to_string(), d);
        }
        out.bound_references = resolve::references(&out.symbols, &docs);
        out.calls = out
            .bound_references
            .iter()
            .filter(|r| r.reference.is_call)
            .cloned()
            .collect();
        out.inheritance = inheritance::edges(&out.symbols, &docs);
        out.entrypoints = analysis::entrypoints(&out.symbols, &docs);
        out
    }
    fn calls<'a>(s: &'a CodeSnapshot, expression: &str) -> Vec<&'a CallEdge> {
        s.calls
            .iter()
            .filter(|r| r.reference.expression == expression)
            .collect()
    }
    fn names<'a>(s: &'a CodeSnapshot, c: &CallEdge) -> Vec<&'a str> {
        c.candidates
            .iter()
            .map(|id| {
                s.symbols
                    .iter()
                    .find(|s| s.id == *id)
                    .unwrap()
                    .qualified_name
                    .as_str()
            })
            .collect()
    }
    #[test]
    fn imports_typed_receivers_constructor_and_arity_preserve_overload_ambiguity() {
        let s=snapshot(&[
            ("/p/api/Service.java","p","package api; public class Service { public Service(){} public void run(){} public void run(int x){} public void ambiguous(int x){} public void ambiguous(String s){} }"),
            ("/p/app/Main.java","p","package app; import api.Service; class Main { void use(Service s){s.run();s.run(1);s.ambiguous(1);new Service();} }"),
        ]);
        let run = calls(&s, "s.run");
        assert_eq!(run.len(), 2);
        assert!(run.iter().all(|c| c.candidates.len() == 1), "{run:?}");
        assert_ne!(run[0].candidates, run[1].candidates);
        assert_eq!(calls(&s, "s.ambiguous")[0].resolution, "ambiguous");
        assert_eq!(calls(&s, "s.ambiguous")[0].candidates.len(), 2);
        assert_eq!(
            names(&s, calls(&s, "new Service")[0]),
            vec!["Service::Service"]
        );
        let refs = s.query(
            "references",
            &CodeQuery {
                symbol: Some(run[0].candidates[0].clone()),
                ..Default::default()
            },
        );
        assert_eq!(refs["total"], 1, "{refs}");
        assert!(s.calls.iter().all(|r| !r.reference.document_id.is_empty()));
    }
    #[test]
    fn inherited_overloads_overrides_super_and_private_methods_are_separate() {
        let s=snapshot(&[
            ("/p/Base.java","p","package p; class Base { public void work(){} public void work(int x){} private void secret(){} }"),
            ("/p/Child.java","p","package p; class Child extends Base {public void work(){} void use(){work();work(1);super.work();secret();}}"),
        ]);
        let work = calls(&s, "work");
        assert_eq!(names(&s, work[0]), vec!["Child::work"]);
        assert_eq!(names(&s, work[1]), vec!["Base::work"]);
        assert_eq!(names(&s, calls(&s, "super.work")[0]), vec!["Base::work"]);
        assert!(calls(&s, "secret")[0].candidates.is_empty());
        assert_eq!(s.inheritance.len(), 1);
        assert_eq!(s.inheritance[0].hierarchy_status, "complete_static");
    }
    #[test]
    fn typed_fields_this_local_shadowing_static_receivers_and_interface_contracts() {
        let s=snapshot(&[("/p/A.java","p","class Service {static void stat(){} void run(){}} interface Api {void read();} class Main { Service svc; void use(Service other,Api api){this.svc.run();other.run();api.read();Service.stat();Service.run();} void shadow(Object Service){Service.stat();}}")]);
        for expr in ["this.svc.run", "other.run", "api.read"] {
            assert_eq!(
                calls(&s, expr)[0].candidates.len(),
                1,
                "{expr}: {:?}",
                s.calls
            );
        }
        assert!(calls(&s, "Service.run")[0].candidates.is_empty());
        let stat = calls(&s, "Service.stat");
        assert_eq!(stat[0].candidates.len(), 1);
        assert!(stat[1].candidates.is_empty());
    }
    #[test]
    fn static_and_wildcard_imports_do_not_fall_back_to_unrelated_types_or_projects() {
        let s=snapshot(&[
            ("/p/a/Util.java","p","package a; public class Util { public static void work(){} }"),
            ("/p/a/Service.java","p","package a; public class Service {public void run(){}}"),
            ("/p/b/Service.java","p","package b; public class Service {public void run(){}}"),
            ("/p/Main.java","p","import static a.Util.work; import a.*; import b.*; class Main {void use(Service s){work();s.run();}}"),
            ("/p/Foreign.java","other","package a; public class Util {public static void work(){}}"),
        ]);
        assert_eq!(calls(&s, "work")[0].candidates.len(), 1);
        assert_eq!(calls(&s, "s.run")[0].candidates.len(), 2);
        assert_eq!(calls(&s, "s.run")[0].resolution, "ambiguous");
        let s=snapshot(&[("/p/A.java","p","import absent.Service; class Service {static void run(){}} class Main {void use(){Service.run();}}")]);
        assert!(calls(&s, "Service.run")[0].candidates.is_empty());
    }
    #[test]
    fn cycles_missing_bases_and_unindexed_object_methods_do_not_prove_unique_dispatch() {
        let s = snapshot(&[(
            "/p/A.java",
            "p",
            "class A extends B {void run(){} void use(){run();}} class B extends A {}",
        )]);
        assert_eq!(calls(&s, "run")[0].resolution, "ambiguous");
        assert!(s
            .inheritance
            .iter()
            .all(|e| e.hierarchy_status == "incomplete_or_invalid"));
        let s=snapshot(&[("/p/A.java","p","import static A.toString; class A { static void toString(int x){} } class Main {void use(){toString();}}")]);
        assert!(calls(&s, "toString")[0].candidates.is_empty());
    }
    #[test]
    fn method_references_records_and_opaque_anonymous_owners_keep_source_truth() {
        let s=snapshot(&[("/p/A.java","p","class Service {void run(){} void run(int x){}} record R(int x) {R {}} enum E {ONE} @interface Tag {} class Main {void use(Service s){Object cb=s::run;new R(1);Runnable r=()->s.run();}}")]);
        let refs = s.query(
            "references",
            &CodeQuery {
                symbol: Some("Service::run".into()),
                ..Default::default()
            },
        );
        assert!(
            refs["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["expression"] == "s::run" && r["resolution"] == "ambiguous"),
            "{refs}"
        );
        assert_eq!(names(&s, calls(&s, "new R")[0]), vec!["R::R"]);
        assert!(calls(&s, "s.run")[0].candidates.is_empty());
        for kind in [
            "record_declaration",
            "enum_declaration",
            "annotation_type_declaration",
        ] {
            assert!(s.symbols.iter().any(|s| s.kind == kind), "{kind}");
        }
    }
    #[test]
    fn class_members_take_precedence_over_interface_declarations() {
        let s=snapshot(&[("/p/A.java","p","interface Api {default void work(){}} class Base {public void work(){}} class Child extends Base implements Api {void use(){work();}}")]);
        assert_eq!(names(&s, calls(&s, "work")[0]), vec!["Base::work"]);
        assert_eq!(calls(&s, "work")[0].resolution, "resolved_static");
    }
    #[test]
    fn constructor_and_unbound_instance_references_do_not_claim_sam_arity() {
        let s=snapshot(&[("/p/A.java","p","class Service {Service(){} Service(int x){} void run(){}} class Main {void use(){Object ctor=Service::new;Object method=Service::run;}}")]);
        let ctor = s.query(
            "references",
            &CodeQuery {
                symbol: Some("Service::Service".into()),
                ..Default::default()
            },
        );
        assert!(
            ctor["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["expression"] == "Service::new" && r["resolution"] == "ambiguous"),
            "{ctor}"
        );
        let method = s.query(
            "references",
            &CodeQuery {
                symbol: Some("Service::run".into()),
                ..Default::default()
            },
        );
        assert_eq!(method["total"], 1, "{method}");
        assert_eq!(method["items"][0]["expression"], "Service::run");
        let ty = s
            .symbols
            .iter()
            .find(|s| s.qualified_name == "Service")
            .unwrap();
        assert!(
            s.query(
                "references",
                &CodeQuery {
                    symbol: Some(ty.id.clone()),
                    ..Default::default()
                }
            )["total"]
                .as_u64()
                .unwrap()
                >= 2
        );
    }
    #[test]
    fn comments_in_imports_qualified_types_and_receivers_do_not_change_binding() {
        let s=snapshot(&[
            ("/p/S.java","p","package api /* comment */ . inner; public class Service { public void run(){} public static void work(){} }"),
            ("/p/M.java","p","import /* comment */ api.inner.Service; import static /* comment */ api.inner.Service.work; class Main { void use(Service s){s /*comment*/ .run();work();api.inner.Service.work();}}"),
        ]);
        assert_eq!(calls(&s, "s.run")[0].candidates.len(), 1);
        assert_eq!(calls(&s, "work")[0].candidates.len(), 1);
        assert_eq!(calls(&s, "api.inner.Service.work")[0].candidates.len(), 1);
    }
    #[test]
    fn enhanced_for_headers_use_outer_fields_and_record_components_keep_their_owner() {
        let s=snapshot(&[("/p/A.java","p","class Service {void run(){}} class Bag {Service[] items(){return null;}} class Main {Bag value;void use(){for(Service value:value.items()){value.run();}}} record R(Service service){} class service {static void run(){}} class Other {void use(){service.run();}}")]);
        assert_eq!(names(&s, calls(&s, "value.items")[0]), vec!["Bag::items"]);
        assert_eq!(names(&s, calls(&s, "value.run")[0]), vec!["Service::run"]);
        assert_eq!(names(&s, calls(&s, "service.run")[0]), vec!["service::run"]);
    }
    #[test]
    fn identical_paths_in_other_projects_do_not_change_java_targets() {
        let s = snapshot(&[
            (
                "/p/Service.java",
                "p",
                "package api;public class Service {public void run(){}}",
            ),
            (
                "/p/Service.java",
                "other",
                "package api;public class Service {public void run(){}}",
            ),
            (
                "/p/Main.java",
                "p",
                "import api.Service;class Main {void use(Service s){s.run();}}",
            ),
        ]);
        let target = calls(&s, "s.run")[0];
        assert_eq!(target.candidates.len(), 1);
        assert!(target.candidates[0].starts_with("0:"), "{target:?}");
    }
    #[test]
    fn java_sql_text_blocks_do_not_publish_unmodeled_table_identifiers() {
        let source = r#"class Queries { void use(){
            String plain = "SELECT * FROM users";
            String block = """
                SELECT * FROM "multiline
                  identifier"
                """;
        }}"#;
        let parsed = parse("j", "/p/Queries.java", source, 1).unwrap();
        assert!(!parsed.parse_errors);
        let sql = serde_json::to_value(&parsed.sql).unwrap();
        assert_eq!(sql["uses"].as_array().unwrap().len(), 1);
        assert!(sql["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "JAVA_SQL_TEXT_BLOCK_VALUE_UNVERIFIED"));
    }
    #[test]
    fn annotation_values_cannot_spoof_static_or_private_modifiers() {
        let s=snapshot(&[
            ("/p/Host.java","p","package api; @interface Label {String value();} public class Host {@Label(\"static private\") public void run(){} @Label(\"static\") public void use(){run();}}"),
            ("/p/Main.java","p","import api.Host; class Main {void use(Host h){h.run();}}"),
        ]);
        assert_eq!(names(&s, calls(&s, "run")[0]), vec!["Host::run"]);
        assert_eq!(names(&s, calls(&s, "h.run")[0]), vec!["Host::run"]);
    }
}
