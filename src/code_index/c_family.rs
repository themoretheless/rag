//! Source C/C++ declarations and translation-unit candidates; never invokes a preprocessor/linker.
use super::*;
use std::path::{Component, Path, PathBuf};
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Facts {
    pub cpp: bool,
    functions: Vec<Function>,
    types: Vec<Type>,
    includes: Vec<Include>,
    bindings: Vec<Binding>,
    macros: HashSet<String>,
    sites: Vec<Site>,
    opaque: Vec<(usize, usize)>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Function {
    id: String,
    name: String,
    qname: String,
    definition: bool,
    internal: bool,
    conditional: bool,
    arity: Option<usize>,
    variadic: bool,
    min_arity: usize,
    empty_parameters: bool,
    start: usize,
    end: usize,
    declared: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Type {
    id: String,
    qname: String,
    bases: Vec<String>,
    complete: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Include {
    start: usize,
    path: Option<String>,
    conditional: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Binding {
    name: String,
    ty: Option<String>,
    start: usize,
    end: usize,
    declared: usize,
    owner: String,
    field: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Site {
    start: usize,
    arity: usize,
}
impl Facts {
    pub(super) fn new(path: &str) -> Self {
        Self {
            cpp: !matches!(path.rsplit('.').next(), Some("c" | "h")),
            ..Default::default()
        }
    }
}
pub(super) fn supported(path: &str) -> bool {
    matches!(
        path.rsplit('.').next(),
        Some("c" | "h" | "C" | "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx")
    )
}
fn header(path: &str) -> bool {
    matches!(path.rsplit('.').next(), Some("h" | "hpp" | "hh" | "hxx"))
}
fn diag(out: &mut IndexedDocument, code: &str) {
    if !out.binding_limitations.iter().any(|c| c == code) {
        out.binding_limitations.push(code.into());
    }
}
fn type_node(n: Node) -> bool {
    matches!(
        n.kind(),
        "class_specifier" | "struct_specifier" | "union_specifier" | "enum_specifier"
    )
}
/// Follow declarators, not their parameter/initializer subtrees.
fn declared_name(mut node: Node) -> Option<Node> {
    for _ in 0..64 {
        if matches!(
            node.kind(),
            "identifier"
                | "field_identifier"
                | "type_identifier"
                | "qualified_identifier"
                | "operator_name"
                | "destructor_name"
        ) {
            return Some(node);
        }
        if let Some(next) = node.child_by_field_name("declarator") {
            node = next;
            continue;
        }
        if matches!(
            node.kind(),
            "parenthesized_declarator"
                | "reference_declarator"
                | "abstract_parenthesized_declarator"
        ) {
            node = node.named_child(0)?;
            continue;
        }
        return None;
    }
    None
}
pub(super) fn terminal_name(node: Node) -> Option<Node> {
    if matches!(node.kind(), "operator_name" | "destructor_name") {
        return Some(node);
    }
    if node.kind() == "field_expression" {
        return node.child_by_field_name("field").and_then(terminal_name);
    }
    if matches!(
        node.kind(),
        "template_function" | "template_method" | "template_type"
    ) {
        return node.child_by_field_name("name").and_then(terminal_name);
    }
    if node.kind() == "qualified_identifier" {
        return node.child_by_field_name("name").and_then(terminal_name);
    }
    last_identifier(node)
}
fn function_declarator(decl: Node) -> Option<(Node, Node)> {
    let name = declared_name(decl)?;
    let mut parent = name.parent();
    while let Some(n) = parent {
        if n.id() == decl.id() && n.kind() != "function_declarator" {
            return None;
        }
        if matches!(
            n.kind(),
            "pointer_declarator" | "reference_declarator" | "array_declarator"
        ) {
            return None;
        }
        if n.kind() == "function_declarator" {
            return Some((n, name));
        }
        parent = n.parent();
    }
    None
}
fn full_name(node: Node, source: &str) -> Option<String> {
    let s = text(node, source).trim();
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '~'))
    {
        Some(s.trim_start_matches("::").into())
    } else {
        None
    }
}
fn scope_name(node: Node, source: &str) -> String {
    let mut nodes = vec![];
    let mut p = node.parent();
    while let Some(n) = p {
        if n.kind() == "namespace_definition" {
            nodes.push(
                n.child_by_field_name("name")
                    .map(|n| text(n, source).into())
                    .unwrap_or_else(|| format!("<anonymous@{}>", n.start_byte())),
            );
        } else if type_node(n) {
            nodes.push(
                n.child_by_field_name("name")
                    .map(|name| text(name, source).to_owned())
                    .unwrap_or_else(|| format!("<anonymous-type@{}>", n.start_byte())),
            );
        }
        p = n.parent();
    }
    nodes.reverse();
    nodes.join("::")
}
fn qualify(scope: &str, name: &str) -> String {
    if scope.is_empty() {
        name.into()
    } else {
        format!("{scope}::{name}")
    }
}
fn storage(node: Node, source: &str, value: &str) -> bool {
    let mut c = node.walk();
    let found = node
        .named_children(&mut c)
        .any(|n| n.kind() == "storage_class_specifier" && text(n, source) == value);
    found
}
fn guard(node: Node, source: &str, path: &str) -> bool {
    if !header(path)
        || node.kind() != "preproc_ifdef"
        || !text(node, source).trim_start().starts_with("#ifndef")
        || node.child_by_field_name("alternative").is_some()
    {
        return false;
    }
    if !node
        .parent()
        .is_some_and(|p| p.kind() == "translation_unit")
    {
        return false;
    }
    let Some(name) = node.child_by_field_name("name") else {
        return false;
    };
    let mut c = node.walk();
    let first = node
        .named_children(&mut c)
        .find(|n| n.id() != name.id() && n.kind() != "comment");
    first.is_some_and(|n| {
        n.kind() == "preproc_def"
            && n.child_by_field_name("name")
                .is_some_and(|d| text(d, source) == text(name, source))
    })
}
fn conditional(node: Node, source: &str, path: &str) -> bool {
    let mut p = node.parent();
    while let Some(n) = p {
        if matches!(
            n.kind(),
            "preproc_if" | "preproc_ifdef" | "preproc_else" | "preproc_elif"
        ) && !guard(n, source, path)
        {
            return true;
        }
        p = n.parent();
    }
    false
}
fn class_member(node: Node) -> bool {
    let mut p = node.parent();
    while let Some(n) = p {
        if type_node(n) {
            return true;
        }
        if n.kind() == "function_definition" {
            return false;
        }
        p = n.parent();
    }
    false
}
fn unsupported(node: Node) -> bool {
    let mut p = Some(node);
    while let Some(n) = p {
        if matches!(n.kind(), "template_declaration" | "lambda_expression") {
            return true;
        }
        p = n.parent();
    }
    false
}
fn add_symbol(
    node: Node,
    name: Node,
    qname: String,
    kind: &str,
    source: &str,
    doc: &str,
    out: &mut IndexedDocument,
) -> String {
    let terminal = terminal_name(name).unwrap_or(name);
    let short = if matches!(name.kind(), "destructor_name" | "operator_name") {
        text(name, source)
    } else {
        text(terminal, source)
    };
    let id = format!("{}:{}:{}", doc, node.start_byte(), short);
    let body = node.child_by_field_name("body");
    out.symbols.push(Symbol {
        id: id.clone(),
        document_id: doc.into(),
        path: out.path.clone(),
        name: short.into(),
        qualified_name: qname,
        kind: kind.into(),
        signature: source[node.start_byte()..body.map_or(node.end_byte(), |n| n.start_byte())]
            .trim()
            .into(),
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        revision: out.revision,
        content_hash: out.hash.clone(),
    });
    id
}
fn scope(node: Node, parameter: bool) -> (usize, usize) {
    let mut p = node.parent();
    while let Some(n) = p {
        if n.kind() == "compound_statement" || parameter && n.kind() == "function_definition" {
            return (n.start_byte(), n.end_byte());
        }
        p = n.parent();
    }
    (0, usize::MAX)
}
fn simple_type(node: Node, source: &str) -> Option<String> {
    if type_node(node) {
        return node
            .child_by_field_name("name")
            .and_then(|n| full_name(n, source));
    }
    if matches!(
        node.kind(),
        "type_identifier" | "qualified_identifier" | "primitive_type"
    ) {
        full_name(node, source)
    } else {
        None
    }
}
fn arity(node: Node, source: &str, cpp: bool) -> (Option<usize>, bool) {
    let Some(p) = node.child_by_field_name("parameters") else {
        return (None, false);
    };
    let mut c = p.walk();
    let children: Vec<_> = p
        .named_children(&mut c)
        .filter(|n| n.kind() != "comment")
        .collect();
    if children.len() == 1
        && children[0].kind() == "parameter_declaration"
        && children[0].child_by_field_name("declarator").is_none()
        && children[0]
            .child_by_field_name("type")
            .is_some_and(|n| text(n, source) == "void")
    {
        return (Some(0), false);
    }
    let variadic = children.iter().any(|n| {
        matches!(
            n.kind(),
            "variadic_parameter" | "variadic_parameter_declaration"
        )
    });
    if children.is_empty() && !cpp {
        return (None, false);
    }
    if children.iter().any(|n| {
        !matches!(
            n.kind(),
            "parameter_declaration"
                | "optional_parameter_declaration"
                | "variadic_parameter"
                | "variadic_parameter_declaration"
        )
    }) {
        return (None, variadic);
    }
    (
        Some(
            children
                .iter()
                .filter(|n| {
                    !matches!(
                        n.kind(),
                        "variadic_parameter" | "variadic_parameter_declaration"
                    )
                })
                .count(),
        ),
        variadic,
    )
}
pub(super) fn collect(
    node: Node,
    source: &str,
    doc: &str,
    out: &mut IndexedDocument,
) -> Option<String> {
    if node.kind() == "translation_unit" {
        diag(out, "C_FAMILY_PREPROCESSING_BUILD_AND_LINKAGE_UNVERIFIED");
    }
    if matches!(
        node.kind(),
        "lambda_expression" | "optional_parameter_declaration"
    ) {
        if node.kind() == "lambda_expression" {
            out.c_family
                .as_mut()
                .unwrap()
                .opaque
                .push((node.start_byte(), node.end_byte()));
            diag(out, "CXX_LAMBDA_OWNER_UNMODELED");
        } else if let Some(value) = node.child_by_field_name("default_value") {
            out.c_family
                .as_mut()
                .unwrap()
                .opaque
                .push((value.start_byte(), value.end_byte()));
        }
    }
    if matches!(node.kind(), "preproc_def" | "preproc_function_def") {
        if let Some(n) = node.child_by_field_name("name") {
            out.c_family
                .as_mut()
                .unwrap()
                .macros
                .insert(text(n, source).into());
        }
    }
    if node.kind() == "preproc_include" {
        let path = node
            .child_by_field_name("path")
            .filter(|n| n.kind() == "string_literal")
            .and_then(|n| serde_json::from_str::<String>(text(n, source)).ok());
        if path.is_none() {
            diag(out, "C_INCLUDE_SEARCH_OR_MACRO_PATH_UNVERIFIED");
        }
        out.c_family.as_mut().unwrap().includes.push(Include {
            start: node.start_byte(),
            path,
            conditional: conditional(node, source, &out.path),
        });
    }
    if node.kind() == "call_expression" || node.kind() == "new_expression" {
        let arity = node
            .child_by_field_name("arguments")
            .map(|args| {
                let mut c = args.walk();
                args.named_children(&mut c)
                    .filter(|n| n.kind() != "comment")
                    .count()
            })
            .unwrap_or(0);
        out.c_family.as_mut().unwrap().sites.push(Site {
            start: node.start_byte(),
            arity,
        });
    }
    let scope_name = scope_name(node, source);
    let mut owner = None;
    if node.kind() == "namespace_definition" {
        if let Some(name) = node.child_by_field_name("name") {
            owner = Some(add_symbol(
                node,
                name,
                qualify(&scope_name, text(name, source)),
                "namespace_definition",
                source,
                doc,
                out,
            ));
        }
    }
    if type_node(node) {
        if let Some(name) = node.child_by_field_name("name") {
            if node.child_by_field_name("body").is_some() {
                let qname = qualify(&scope_name, text(name, source));
                let id = add_symbol(node, name, qname.clone(), node.kind(), source, doc, out);
                let mut bases = vec![];
                let mut c = node.walk();
                for base in node
                    .named_children(&mut c)
                    .filter(|n| n.kind() == "base_class_clause")
                {
                    let mut c = base.walk();
                    for ty in base.named_children(&mut c).filter(|n| {
                        matches!(
                            n.kind(),
                            "type_identifier" | "qualified_identifier" | "template_type"
                        )
                    }) {
                        bases.push(text(ty, source).into());
                    }
                }
                out.c_family.as_mut().unwrap().types.push(Type {
                    id: id.clone(),
                    qname,
                    bases,
                    complete: !conditional(node, source, &out.path) && !unsupported(node),
                });
                owner = Some(id);
            }
        }
    }
    let declarations = if node.kind() == "function_definition" {
        node.child_by_field_name("declarator")
            .into_iter()
            .collect::<Vec<_>>()
    } else if matches!(node.kind(), "declaration" | "field_declaration") {
        let mut c = node.walk();
        node.children_by_field_name("declarator", &mut c).collect()
    } else {
        vec![]
    };
    for declaration in declarations {
        if let Some((function, name)) = function_declarator(declaration) {
            let full = full_name(name, source).unwrap_or_else(|| text(name, source).trim().into());
            if full.contains("operator") {
                diag(out, "CXX_OPERATOR_BINDING_UNVERIFIED");
            }
            let qname = qualify(&scope_name, &full);
            let definition =
                node.kind() == "function_definition" && node.child_by_field_name("body").is_some();
            let short = full.rsplit("::").next().unwrap_or(&full).to_owned();
            let id = add_symbol(
                node,
                name,
                qname.clone(),
                if definition {
                    "function_definition"
                } else {
                    "function_prototype"
                },
                source,
                doc,
                out,
            );
            let (arity, variadic) = arity(function, source, out.c_family.as_ref().unwrap().cpp);
            let (start, end) = scope(node, false);
            let mut c = function.child_by_field_name("parameters").unwrap().walk();
            let parameters = function.child_by_field_name("parameters").unwrap();
            let empty_parameters = parameters
                .named_children(&mut c)
                .all(|n| n.kind() == "comment");
            let mut c = parameters.walk();
            let defaults = parameters
                .named_children(&mut c)
                .filter(|n| n.kind() == "optional_parameter_declaration")
                .count();
            out.c_family.as_mut().unwrap().functions.push(Function {
                id: id.clone(),
                name: short,
                qname,
                definition,
                internal: storage(node, source, "static") && !class_member(node)
                    || scope_name.contains("<anonymous")
                    || start != 0,
                conditional: conditional(node, source, &out.path) || unsupported(node),
                arity,
                variadic,
                min_arity: arity.unwrap_or(0).saturating_sub(defaults),
                empty_parameters,
                start,
                end,
                declared: node.start_byte(),
            });
            if definition {
                owner = Some(id);
            }
        } else if let Some(name) = declared_name(declaration) {
            let parameter = false;
            let (start, end) = scope(node, parameter);
            let field = node.kind() == "field_declaration";
            let ty = if declaration.kind() == "function_declarator" {
                None
            } else {
                node.child_by_field_name("type")
                    .and_then(|n| simple_type(n, source))
            };
            out.c_family.as_mut().unwrap().bindings.push(Binding {
                name: text(name, source).into(),
                ty,
                start,
                end,
                declared: if field { 0 } else { name.end_byte() },
                owner: scope_name.clone(),
                field,
            });
        }
    }
    if matches!(
        node.kind(),
        "parameter_declaration" | "optional_parameter_declaration"
    ) {
        if let Some(decl) = node.child_by_field_name("declarator") {
            if let Some(name) = declared_name(decl) {
                let (start, end) = scope(node, true);
                out.c_family.as_mut().unwrap().bindings.push(Binding {
                    name: text(name, source).into(),
                    ty: node
                        .child_by_field_name("type")
                        .and_then(|n| simple_type(n, source)),
                    start,
                    end,
                    declared: start,
                    owner: scope_name,
                    field: false,
                });
            }
        }
    }
    owner
}
pub(super) fn non_usage(node: Node) -> bool {
    let mut p = node.parent();
    while let Some(n) = p {
        if matches!(
            n.kind(),
            "preproc_include" | "preproc_def" | "preproc_function_def" | "preproc_call"
        ) {
            return true;
        }
        if matches!(n.kind(), "preproc_if" | "preproc_ifdef" | "preproc_elif") {
            for field in ["name", "condition"] {
                if n.child_by_field_name(field).is_some_and(|c| {
                    c.start_byte() <= node.start_byte() && node.end_byte() <= c.end_byte()
                }) {
                    return true;
                }
            }
        }
        if matches!(
            n.kind(),
            "declaration"
                | "field_declaration"
                | "function_definition"
                | "parameter_declaration"
                | "optional_parameter_declaration"
        ) {
            let mut c = n.walk();
            if n.children_by_field_name("declarator", &mut c)
                .filter_map(declared_name)
                .any(|name| {
                    name.id() == node.id()
                        || terminal_name(name).is_some_and(|n| n.id() == node.id())
                })
            {
                return true;
            }
        }
        if (type_node(n) || n.kind() == "namespace_definition")
            && n.child_by_field_name("name")
                .is_some_and(|name| name.id() == node.id())
            && n.child_by_field_name("body").is_some()
        {
            return true;
        }
        p = n.parent();
    }
    false
}
pub(super) fn push_new(
    node: Node,
    source: &str,
    doc: &str,
    out: &mut IndexedDocument,
    caller: Option<&str>,
) {
    if node.kind() != "new_expression" {
        return;
    }
    let Some(ty) = node.child_by_field_name("type") else {
        return;
    };
    let Some(name) = terminal_name(ty) else {
        return;
    };
    out.references.push(Reference {
        start_byte: node.start_byte(),
        document_id: doc.into(),
        path: out.path.clone(),
        name: text(name, source).into(),
        expression: format!("new {}", text(ty, source)),
        line: name.start_position().row + 1,
        column: name.start_position().column + 1,
        caller: caller.map(str::to_owned),
        is_call: true,
        revision: out.revision,
        content_hash: out.hash.clone(),
    });
}
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for p in path.components() {
        match p {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            _ => out.push(p.as_os_str()),
        }
    }
    out
}
fn same_doc(a: &IndexedDocument, b: &IndexedDocument) -> bool {
    std::ptr::eq(a, b)
        || a.symbols
            .first()
            .zip(b.symbols.first())
            .is_some_and(|(a, b)| a.document_id == b.document_id)
}
fn visible<'a>(
    docs: &'a HashMap<String, IndexedDocument>,
    current: &IndexedDocument,
    at: Option<usize>,
) -> (Vec<&'a IndexedDocument>, bool) {
    let mut result = vec![];
    let mut todo = vec![current.path.clone()];
    let mut seen = HashSet::new();
    let mut complete = true;
    while let Some(path) = todo.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        if seen.len() > 128 {
            complete = false;
            break;
        }
        let found: Vec<_> = docs
            .values()
            .filter(|d| {
                d.path == path
                    && d.project == current.project
                    && d.c_family.is_some()
                    && !d.parse_errors
            })
            .collect();
        if found.len() != 1 {
            complete = false;
            continue;
        }
        let doc = found[0];
        result.push(doc);
        for inc in &doc.c_family.as_ref().unwrap().includes {
            if same_doc(doc, current) && at.is_some_and(|at| inc.start > at) {
                continue;
            }
            if inc.conditional {
                complete = false;
                continue;
            }
            if let Some(path) = &inc.path {
                let path = normalized(
                    &Path::new(&doc.path)
                        .parent()
                        .unwrap_or(Path::new(""))
                        .join(path),
                )
                .to_string_lossy()
                .into_owned();
                todo.push(path);
            } else {
                complete = false;
            }
        }
    }
    (result, complete)
}
fn names(scope: &str, name: &str) -> Vec<String> {
    if name.starts_with("::") {
        return vec![name.trim_start_matches("::").into()];
    }
    let mut result = vec![];
    let mut prefix = Some(scope);
    while let Some(p) = prefix {
        result.push(qualify(p, name));
        prefix = p
            .rsplit_once("::")
            .map(|p| p.0)
            .or_else(|| (!p.is_empty()).then_some(""));
    }
    result.dedup();
    result
}
fn type_lookup<'a>(
    visible: &[&'a IndexedDocument],
    scope: &str,
    name: &str,
) -> Vec<(&'a IndexedDocument, &'a Type)> {
    for q in names(scope, name) {
        let q = &q;
        let result: Vec<_> = visible
            .iter()
            .flat_map(|d| {
                d.c_family
                    .as_ref()
                    .unwrap()
                    .types
                    .iter()
                    .filter(move |t| t.qname == *q)
                    .map(move |t| (*d, t))
            })
            .collect();
        if !result.is_empty() {
            return result;
        }
    }
    vec![]
}
fn effective_arity(f: &Function, owner: &IndexedDocument, cpp: bool) -> Option<usize> {
    if cpp && header(&owner.path) && f.empty_parameters {
        Some(0)
    } else {
        f.arity
    }
}
fn accepts(f: &Function, owner: &IndexedDocument, cpp: bool, n: Option<usize>) -> bool {
    n.is_none_or(|n| {
        effective_arity(f, owner, cpp)
            .is_none_or(|arity| n >= f.min_arity && (f.variadic || n <= arity))
    })
}
fn functions<'a>(
    visible: &[&'a IndexedDocument],
    scope: &str,
    name: &str,
) -> Vec<(&'a IndexedDocument, &'a Function)> {
    for q in names(scope, name) {
        let q = &q;
        let result: Vec<_> = visible
            .iter()
            .flat_map(|d| {
                d.c_family
                    .as_ref()
                    .unwrap()
                    .functions
                    .iter()
                    .filter(move |f| f.qname == *q)
                    .map(move |f| (*d, f))
            })
            .collect();
        if !result.is_empty() {
            return result;
        }
    }
    vec![]
}
fn members<'a>(
    visible: &[&'a IndexedDocument],
    ty: (&'a IndexedDocument, &'a Type),
    name: &str,
    stack: &mut HashSet<String>,
    budget: &mut usize,
) -> (Vec<(&'a IndexedDocument, &'a Function)>, bool) {
    if *budget == 0 || !stack.insert(ty.1.id.clone()) {
        return (vec![], false);
    }
    *budget -= 1;
    let own = functions(visible, "", &format!("{}::{name}", ty.1.qname));
    let mut complete = ty.1.complete;
    let mut result = own;
    if result.is_empty()
        && !visible.iter().any(|d| {
            d.c_family
                .as_ref()
                .unwrap()
                .bindings
                .iter()
                .any(|b| b.field && b.owner == ty.1.qname && b.name == name)
        })
    {
        if ty.1.bases.len() > 1 {
            complete = false;
        }
        for base in &ty.1.bases {
            let bases = type_lookup(
                visible,
                ty.1.qname.rsplit_once("::").map_or("", |p| p.0),
                base,
            );
            if bases.len() != 1 {
                complete = false;
                continue;
            }
            let (inherited, valid) = members(visible, bases[0], name, stack, budget);
            complete &= valid;
            result.extend(inherited);
        }
    }
    stack.remove(&ty.1.id);
    (result, complete)
}
pub(super) fn resolve(
    symbols: &[Symbol],
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
    r: &Reference,
) -> CallEdge {
    let facts = doc.c_family.as_ref().unwrap();
    let (visible, _includes_complete) = visible(docs, doc, Some(r.start_byte));
    let caller = facts
        .functions
        .iter()
        .find(|f| Some(&f.id) == r.caller.as_ref());
    let scope = caller
        .and_then(|f| f.qname.rsplit_once("::").map(|p| p.0))
        .unwrap_or("");
    let expression = r.expression.replace("->", ".");
    let new_type = expression.strip_prefix("new ");
    let raw = new_type.unwrap_or(&expression);
    let mut found = vec![];
    let mut complete = true;
    let mut reason = "c_source_scope";
    let arity = if r.is_call {
        facts
            .sites
            .iter()
            .find(|s| s.start == r.start_byte)
            .map(|s| s.arity)
    } else {
        None
    };
    if doc.parse_errors
        || facts
            .opaque
            .iter()
            .any(|(s, e)| *s <= r.start_byte && r.start_byte < *e)
        || !raw
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '.' | '~'))
    {
        reason = "c_dynamic_template_or_anonymous_expression";
    } else if visible
        .iter()
        .any(|d| d.c_family.as_ref().unwrap().macros.contains(&r.name))
    {
        reason = "c_macro_expansion_unmodeled";
    } else {
        let mut candidates = vec![];
        if let Some((receiver, member)) = raw.rsplit_once('.') {
            let field = receiver.strip_prefix("this.");
            let binding = facts
                .bindings
                .iter()
                .filter(|b| {
                    b.name == field.unwrap_or(receiver)
                        && b.start <= r.start_byte
                        && r.start_byte < b.end
                        && b.declared <= r.start_byte
                        && (!b.field || b.owner == scope)
                        && (field.is_none() || b.field)
                })
                .max_by_key(|b| (!b.field, b.start, b.declared));
            let ty = if receiver == "this" {
                Some(scope)
            } else {
                binding.and_then(|b| b.ty.as_deref())
            };
            if let Some(ty) = ty {
                let types = type_lookup(&visible, scope, ty);
                if types.len() != 1 {
                    complete = false;
                }
                for ty in types {
                    let (methods, valid) =
                        members(&visible, ty, member, &mut HashSet::new(), &mut 128);
                    candidates.extend(methods);
                    complete &= valid;
                }
            }
            reason = "cxx_receiver_type";
        } else if facts.bindings.iter().any(|b| {
            !b.field
                && b.name == raw
                && b.start <= r.start_byte
                && r.start_byte < b.end
                && b.declared <= r.start_byte
        }) {
            reason = "c_shadowed_or_function_pointer";
        } else {
            if new_type.is_none() {
                let classes = type_lookup(&visible, "", scope);
                if !raw.contains("::") && !classes.is_empty() {
                    for ty in classes {
                        let (members, valid) =
                            members(&visible, ty, raw, &mut HashSet::new(), &mut 128);
                        complete &= valid;
                        candidates.extend(members);
                    }
                    let field = visible.iter().any(|d| {
                        d.c_family
                            .as_ref()
                            .unwrap()
                            .bindings
                            .iter()
                            .any(|b| b.field && b.owner == scope && b.name == raw)
                    });
                    if candidates.is_empty() && complete && !field {
                        candidates =
                            functions(&visible, scope.rsplit_once("::").map_or("", |p| p.0), raw);
                    }
                } else {
                    candidates = functions(&visible, scope, raw);
                }
            }
            if candidates.is_empty() {
                for ty in type_lookup(&visible, scope, raw) {
                    let ctor = ty.1.qname.rsplit("::").next().unwrap();
                    let constructors = functions(&visible, "", &format!("{}::{ctor}", ty.1.qname));
                    if constructors.is_empty() && r.is_call && arity == Some(0) {
                        found.push(ty.1.id.clone());
                        reason = "cxx_type_construction_without_body";
                    } else {
                        candidates.extend(constructors);
                    }
                }
            }
        }
        for (owner, f) in candidates
            .into_iter()
            .filter(|(owner, f)| accepts(f, owner, facts.cpp, arity))
        {
            if same_doc(owner, doc) {
                if !(f.start <= r.start_byte && r.start_byte < f.end) {
                    continue;
                }
                let member = f
                    .qname
                    .rsplit_once("::")
                    .is_some_and(|(scope, _)| !type_lookup(&visible, "", scope).is_empty());
                if !member && f.declared > r.start_byte {
                    continue;
                }
            }
            let count = effective_arity(f, owner, facts.cpp);
            complete &= !f.conditional && count.is_some();
            let mut definitions: Vec<_> = visible
                .iter()
                .flat_map(|d| {
                    d.c_family
                        .as_ref()
                        .unwrap()
                        .functions
                        .iter()
                        .filter(move |other| {
                            other.definition
                                && other.qname == f.qname
                                && effective_arity(other, d, facts.cpp) == count
                        })
                        .map(move |f| (*d, f))
                })
                .collect();
            if definitions.is_empty() && !f.internal {
                definitions = docs
                    .values()
                    .filter(|d| d.project == doc.project && !d.parse_errors && d.c_family.is_some())
                    .flat_map(|d| {
                        d.c_family
                            .as_ref()
                            .unwrap()
                            .functions
                            .iter()
                            .filter(move |other| {
                                other.definition
                                    && !other.internal
                                    && other.qname == f.qname
                                    && effective_arity(other, d, facts.cpp) == count
                            })
                            .map(move |f| (d, f))
                    })
                    .collect();
            }
            if definitions.is_empty() {
                found.push(f.id.clone());
                reason = "c_visible_declaration_without_body";
            } else {
                for (d, definition) in definitions {
                    let _ = d;
                    complete &= !definition.conditional;
                    found.push(definition.id.clone());
                }
            }
            let _ = owner;
        }
        if !r.is_call && found.is_empty() {
            for (_, ty) in type_lookup(&visible, scope, raw) {
                found.push(ty.id.clone());
            }
        }
    }
    found.retain(|id| symbols.iter().any(|s| s.id == *id));
    found.sort();
    found.dedup();
    CallEdge {
        reference: r.clone(),
        resolution: match found.len() {
            0 => "unresolved",
            1 if complete => "resolved_static",
            _ => "ambiguous",
        }
        .into(),
        candidates: found,
        provenance: "derived",
        resolution_basis: if !complete {
            "c_incomplete_conditional_or_overload_candidates".into()
        } else {
            reason.into()
        },
    }
}
pub(super) fn entrypoint(doc: &IndexedDocument, symbol: &Symbol) -> Option<&'static str> {
    let f = doc
        .c_family
        .as_ref()?
        .functions
        .iter()
        .find(|f| f.id == symbol.id)?;
    if !f.definition {
        return None;
    }
    if f.qname == "main" {
        Some("conventional_main_candidate")
    } else if !f.internal {
        Some("external_linkage_candidate")
    } else {
        None
    }
}
pub(super) fn edges(
    symbols: &[Symbol],
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Vec<inheritance::InheritanceEdge> {
    let (visible, _) = visible(docs, doc, None);
    let mut out = vec![];
    for ty in &doc.c_family.as_ref().unwrap().types {
        for base in &ty.bases {
            let targets = type_lookup(
                &visible,
                ty.qname.rsplit_once("::").map_or("", |p| p.0),
                base,
            );
            let ids: Vec<_> = targets
                .into_iter()
                .map(|(_, t)| t.id.clone())
                .filter(|id| symbols.iter().any(|s| s.id == *id))
                .collect();
            out.push(inheritance::InheritanceEdge {
                derived: ty.id.clone(),
                resolution: match ids.len() {
                    0 => "unresolved",
                    1 => "resolved_static",
                    _ => "ambiguous",
                }
                .into(),
                candidates: ids,
                base_expression: base.clone(),
                path: doc.path.clone(),
                line: doc
                    .symbols
                    .iter()
                    .find(|s| s.id == ty.id)
                    .map_or(1, |s| s.start_line),
                relation: "extends".into(),
                hierarchy_status: "structural_bases_only".into(),
                provenance: "derived",
            });
        }
    }
    out
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
    fn c_declarators_distinguish_functions_function_pointer_variables_and_values() {
        let s=snapshot(&[("/p/main.c","p","static void local(void){} static void caller(void){local();} int main(void){caller(); int (*cb)(void)=0; cb(); void (*value)(void)=local;}")]);
        assert_eq!(
            s.symbols
                .iter()
                .filter(|s| s.kind == "function_definition")
                .count(),
            3
        );
        assert!(!s
            .symbols
            .iter()
            .any(|s| s.name == "cb" || s.name == "value"));
        assert_eq!(names(&s, calls(&s, "local")[0]), vec!["local"]);
        assert!(calls(&s, "cb")[0].candidates.is_empty());
        let r = s.query(
            "references",
            &CodeQuery {
                symbol: Some("local".into()),
                ..Default::default()
            },
        );
        assert!(
            r["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["is_call"] == false),
            "{r}"
        );
    }
    #[test]
    fn c_visible_header_prototype_can_link_external_definition_without_global_name_guessing() {
        let s = snapshot(&[
            (
                "/p/api.h",
                "p",
                "#ifndef API_H\n#define API_H\nint work(int x);\n#endif\n",
            ),
            (
                "/p/api.c",
                "p",
                "#include \"api.h\"\nint work(int x){return x;}",
            ),
            (
                "/p/main.c",
                "p",
                "#include \"api.h\"\nint main(void){return work(1);}",
            ),
            ("/p/unknown.c", "p", "int use(void){return work(1);}"),
        ]);
        let work = calls(&s, "work");
        assert_eq!(work.len(), 2);
        assert_eq!(work[0].resolution, "resolved_static", "{work:?}");
        assert!(work[1].candidates.is_empty(), "{work:?}");
        assert_eq!(
            s.symbols
                .iter()
                .find(|s| s.id == work[0].candidates[0])
                .unwrap()
                .path,
            "/p/api.c"
        );
    }
    #[test]
    fn static_definitions_in_other_translation_units_or_projects_are_not_external_targets() {
        let s = snapshot(&[
            (
                "/p/main.c",
                "p",
                "int work(void); int main(void){return work();}",
            ),
            ("/p/other.c", "p", "static int work(void){return 1;}"),
            ("/p/other.c", "foreign", "int work(void){return 2;}"),
        ]);
        let c = calls(&s, "work")[0];
        assert_eq!(c.resolution_basis, "c_visible_declaration_without_body");
        assert_eq!(c.candidates.len(), 1);
        assert_eq!(
            s.symbols
                .iter()
                .find(|s| s.id == c.candidates[0])
                .unwrap()
                .kind,
            "function_prototype"
        );
    }
    #[test]
    fn cpp_header_methods_inheritance_and_overload_arity_link_to_source_bodies() {
        let s=snapshot(&[
            ("/p/api.h","p","#pragma once\nnamespace api {struct Base {void work();void work(int x);};struct Worker:Base {void run();};void over(int x);void over(double x);}"),
            ("/p/api.cpp","p","#include \"api.h\"\nvoid api::Base::work(){} void api::Base::work(int x){} void api::Worker::run(){} void api::over(int x){} void api::over(double x){}"),
            ("/p/main.cpp","p","#include \"api.h\"\nvoid use(api::Worker &w){w.run();w.work();api::over(1);}"),
        ]);
        assert_eq!(names(&s, calls(&s, "w.run")[0]), vec!["api::Worker::run"]);
        assert_eq!(names(&s, calls(&s, "w.work")[0]), vec!["api::Base::work"]);
        assert_eq!(calls(&s, "api::over")[0].candidates.len(), 2);
        assert_eq!(calls(&s, "api::over")[0].resolution, "ambiguous");
        assert!(s
            .inheritance
            .iter()
            .any(|e| e.base_expression == "Base" && !e.candidates.is_empty()));
    }
    #[test]
    fn cpp_member_name_hiding_precedes_namespace_functions_and_base_overloads() {
        let s=snapshot(&[("/p/main.cpp","p","void work(int x){} struct Base{void work(int x){}};struct Child:Base{void work(){}void use(){work(1);}};")]);
        assert!(calls(&s, "work")[0].candidates.is_empty());
        let s = snapshot(&[(
            "/p/main.cpp",
            "p",
            "void work(){} struct Base{void work(){}};struct Child:Base{void use(){work();}};",
        )]);
        assert_eq!(names(&s, calls(&s, "work")[0]), vec!["Base::work"]);
    }
    #[test]
    fn macros_conditional_branches_and_unknown_include_paths_do_not_get_unique_targets() {
        let s=snapshot(&[("/p/main.c","p","#define work() other()\nvoid work(void){}\n#if ENABLE\nvoid chosen(void){}\n#else\nvoid chosen(void){}\n#endif\nvoid use(void){work();chosen();}")]);
        assert!(calls(&s, "work")[0].candidates.is_empty());
        assert_eq!(calls(&s, "chosen")[0].resolution, "ambiguous");
        let s = snapshot(&[
            (
                "/p/main.c",
                "p",
                "#include <api.h>\nint main(void){return work();}",
            ),
            ("/p/api.h", "p", "int work(void);"),
            ("/p/api.c", "p", "int work(void){return 1;}"),
        ]);
        assert!(calls(&s, "work")[0].candidates.is_empty());
    }
    #[test]
    fn c_unspecified_parameter_lists_are_not_cpp_zero_argument_prototypes() {
        let c = snapshot(&[(
            "/p/a.c",
            "p",
            "int work(){return 1;} int use(void){return work(3);}",
        )]);
        assert_eq!(calls(&c, "work")[0].resolution, "ambiguous");
        let cpp = snapshot(&[(
            "/p/a.cpp",
            "p",
            "int work(){return 1;} int use(){return work(3);}",
        )]);
        assert!(calls(&cpp, "work")[0].candidates.is_empty());
    }
    #[test]
    fn default_argument_declarations_and_namespaces_preserve_definition_identity() {
        let s = snapshot(&[(
            "/p/a.cpp",
            "p",
            "namespace n {int work(int x=1);int work(int x){return x;}int use(){return work();}}",
        )]);
        assert_eq!(names(&s, calls(&s, "work")[0]), vec!["n::work"]);
        assert_eq!(
            s.symbols
                .iter()
                .find(|symbol| symbol.id == calls(&s, "work")[0].candidates[0])
                .unwrap()
                .kind,
            "function_definition"
        );
    }
    #[test]
    fn declarations_and_includes_after_a_call_do_not_supply_prior_visibility() {
        let s = snapshot(&[
            ("/p/api.h", "p", "void work(void);"),
            ("/p/api.c", "p", "void work(void){}"),
            (
                "/p/main.c",
                "p",
                "void use(void){work();}\n#include \"api.h\"\n",
            ),
        ]);
        assert!(calls(&s, "work")[0].candidates.is_empty());
        let s = snapshot(&[("/p/a.c", "p", "void use(void){work();}void work(void){}")]);
        assert!(calls(&s, "work")[0].candidates.is_empty());
    }
    #[test]
    fn cpp_operator_declarations_fields_and_raw_sql_keep_source_evidence() {
        let source = r##"struct Service{void run(){}};struct Owner {void use(){field.run();} Service field;};
struct Value{Value operator+(const Value&)const {return Value{};}};
void query(){const char* sql=R"SQL(SELECT *
FROM users WHERE path='c:\work')SQL";}
"##;
        let d = parse("c", "/p/a.cpp", source, 1).unwrap();
        assert!(!d.parse_errors);
        assert!(
            d.symbols.iter().any(|s| s.name == "operator+"),
            "{:?}",
            d.symbols
        );
        let sql = serde_json::to_value(&d.sql).unwrap();
        let uses = sql["uses"].as_array().unwrap();
        assert_eq!(uses.len(), 1, "{sql}");
        assert_eq!(uses[0]["table"], "users");
        assert_eq!(uses[0]["line"], 4, "{sql}");
        let s = snapshot(&[("/p/a.cpp", "p", source)]);
        assert_eq!(names(&s, calls(&s, "field.run")[0]), vec!["Service::run"]);
    }
    #[test]
    fn template_and_anonymous_type_methods_never_collapse_into_free_functions() {
        let source="template<class T>struct Box;template<class T>struct Box<T*>{void run(){}};struct {void hidden(){}} object;void use(){run();hidden();}";
        let s = snapshot(&[("/p/a.cpp", "p", source)]);
        assert!(!s
            .symbols
            .iter()
            .any(|s| s.qualified_name == "run" || s.qualified_name == "hidden"));
        assert!(calls(&s, "run")[0].candidates.is_empty());
        assert!(calls(&s, "hidden")[0].candidates.is_empty());
    }
}
