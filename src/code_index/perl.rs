//! Perl package, lexical declaration and captured call candidates. No interpreter.
use super::*;
use anyhow::Context as _;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Facts {
    packages: Vec<Package>,
    functions: Vec<Function>,
    variables: Vec<Variable>,
    parents: Vec<Parent>,
    sites: BTreeMap<String, Site>,
    dynamic_packages: BTreeSet<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Package {
    id: String,
    name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Function {
    id: String,
    package: String,
    name: String,
    lexical: Option<(usize, usize)>,
    visible: usize,
    body: bool,
    method: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Variable {
    id: String,
    name: String,
    package: String,
    start: usize,
    end: usize,
    visible: usize,
    package_var: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Parent {
    package: String,
    base: String,
    line: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Target {
    Function(String),
    Method {
        package: Option<String>,
        name: String,
    },
    Variable(String),
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Site {
    package: String,
    target: Target,
    reliable: bool,
}
#[derive(Clone)]
struct Context {
    package: String,
    caller: Option<String>,
    reliable: bool,
}
fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut c = node.walk();
    node.named_children(&mut c).collect()
}
fn ident(raw: &str) -> Option<String> {
    let raw = raw.trim();
    (!raw.is_empty()
        && raw
            .split("::")
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_alphanumeric() || c == '_')))
    .then(|| raw.into())
}
fn literal(node: Node<'_>, source: &str) -> Option<String> {
    if node.kind() != "string_literal" {
        return None;
    }
    let raw = text(node, source);
    let quote = *raw.as_bytes().first()?;
    if !matches!(quote, b'\'' | b'"')
        || raw.as_bytes().last() != Some(&quote)
        || raw.contains(['\\', '$', '@'])
    {
        return None;
    }
    raw.get(1..raw.len().checked_sub(1)?).map(str::to_string)
}
fn scope(node: Node<'_>) -> (usize, usize) {
    let mut parent = node.parent();
    while let Some(p) = parent {
        if matches!(
            p.kind(),
            "block" | "block_statement" | "source_file" | "for_statement" | "cstyle_for_statement"
        ) {
            return (p.start_byte(), p.end_byte());
        }
        parent = p.parent();
    }
    (node.start_byte(), node.end_byte())
}
fn visible_after(node: Node<'_>) -> usize {
    let mut parent = node.parent();
    while let Some(p) = parent {
        if p.kind() == "expression_statement" {
            return p.end_byte();
        }
        if matches!(
            p.kind(),
            "block" | "block_statement" | "source_file" | "for_statement" | "cstyle_for_statement"
        ) {
            break;
        }
        parent = p.parent();
    }
    node.end_byte()
}
fn symbol(
    doc: &str,
    node: Node<'_>,
    name: &str,
    qualified: String,
    signature: String,
    out: &mut IndexedDocument,
) -> String {
    let id = format!("{doc}:{}:{name}", node.start_byte());
    out.symbols.push(Symbol {
        id: id.clone(),
        document_id: doc.into(),
        path: out.path.clone(),
        name: name.into(),
        qualified_name: qualified,
        kind: node.kind().into(),
        signature: signature.chars().take(512).collect(),
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
    id
}
fn reference(
    doc: &str,
    node: Node<'_>,
    occurrence: (String, String, Target, bool),
    ctx: &Context,
    out: &mut IndexedDocument,
) {
    let (name, expression, target, is_call) = occurrence;
    out.references.push(Reference {
        start_byte: node.start_byte(),
        document_id: doc.into(),
        path: out.path.clone(),
        name,
        expression,
        line: node.start_position().row + 1,
        column: node.start_position().column + 1,
        caller: ctx.caller.clone(),
        is_call,
        revision: out.revision,
        content_hash: out.hash.clone(),
        build_context: None,
    });
    out.perl.as_mut().unwrap().sites.insert(
        format!("{}:{is_call}", node.start_byte()),
        Site {
            package: ctx.package.clone(),
            target,
            reliable: ctx.reliable,
        },
    );
}
fn variables(node: Node<'_>) -> Vec<Node<'_>> {
    if matches!(node.kind(), "scalar" | "array" | "hash") {
        return vec![node];
    }
    if matches!(
        node.kind(),
        "variable_declaration"
            | "variable_group"
            | "signature"
            | "mandatory_parameter"
            | "optional_parameter"
            | "named_parameter"
            | "slurpy_parameter"
    ) {
        return children(node).into_iter().flat_map(variables).collect();
    }
    vec![]
}
fn declaration(
    doc: &str,
    node: Node<'_>,
    scope_node: Node<'_>,
    source: &str,
    ctx: &Context,
    out: &mut IndexedDocument,
    options: (bool, usize),
) {
    let (package_var, visible) = options;
    let raw = text(node, source);
    let Some(sigil) = raw.chars().next() else {
        return;
    };
    if !matches!(sigil, '$' | '@' | '%') || ident(&raw[sigil.len_utf8()..]).is_none() {
        return;
    }
    let signature = format!("{} {raw}", if package_var { "our" } else { "my" });
    let id = symbol(
        doc,
        node,
        raw,
        format!("{}::{raw}", ctx.package),
        signature,
        out,
    );
    out.symbols.last_mut().unwrap().kind = "perl_variable".into();
    let (start, end) = if scope_node.id() == node.id() {
        scope(node)
    } else {
        (scope_node.start_byte(), scope_node.end_byte())
    };
    out.perl.as_mut().unwrap().variables.push(Variable {
        id,
        name: raw.into(),
        package: ctx.package.clone(),
        start,
        end,
        visible,
        package_var,
    });
}
fn imported(node: Node<'_>, source: &str, ctx: &Context, out: &mut IndexedDocument) {
    let Some(module) = node
        .child_by_field_name("module")
        .and_then(|n| ident(text(n, source)))
    else {
        return;
    };
    let is_no = text(node, source).trim_start().starts_with("no ");
    if matches!(module.as_str(), "parent" | "base") && !is_no {
        let mut bases = vec![];
        for child in children(node) {
            if let Some(base) = literal(child, source).and_then(|v| ident(&v)) {
                bases.push(base);
            }
            if child.kind() == "quoted_word_list" {
                let raw = text(child, source);
                let inner = raw.get(3..raw.len().saturating_sub(1)).unwrap_or("");
                if !raw.contains('\\') {
                    bases.extend(inner.split_whitespace().filter_map(ident));
                }
            }
        }
        if bases.is_empty() {
            out.perl
                .as_mut()
                .unwrap()
                .dynamic_packages
                .insert(ctx.package.clone());
        }
        for base in bases {
            out.perl.as_mut().unwrap().parents.push(Parent {
                package: ctx.package.clone(),
                base: base.clone(),
                line: node.start_position().row + 1,
            });
            file_imports::perl_import(node, &base, "perl_parent_package_candidate", out);
        }
        return;
    }
    if matches!(
        module.as_str(),
        "strict" | "warnings" | "utf8" | "feature" | "integer" | "bytes" | "v5"
    ) {
        return;
    }
    file_imports::perl_import(node, &module, "perl_use_package_candidate", out);
    let raw = text(node, source).trim_end_matches(';').trim();
    let empty = raw.ends_with("()") || raw.ends_with("( )");
    if is_no || !empty {
        out.perl
            .as_mut()
            .unwrap()
            .dynamic_packages
            .insert(ctx.package.clone());
    }
}
fn walk_list(
    node: Node<'_>,
    source: &str,
    doc: &str,
    ctx: &Context,
    out: &mut IndexedDocument,
    depth: usize,
) {
    let mut context = ctx.clone();
    for child in children(node) {
        walk(child, source, doc, &context, out, depth + 1);
        if matches!(child.kind(), "package_statement" | "class_statement")
            && !children(child).iter().any(|n| n.kind() == "block")
        {
            if let Some(name) = child
                .child_by_field_name("name")
                .and_then(|n| ident(text(n, source)))
            {
                context.package = name;
            }
        }
    }
}
fn walk(
    node: Node<'_>,
    source: &str,
    doc: &str,
    ctx: &Context,
    out: &mut IndexedDocument,
    depth: usize,
) {
    if depth >= 256 {
        out.parse_errors = true;
        return;
    }
    let kind = node.kind();
    sql::collect(node, source, out, ctx.caller.as_deref());
    if matches!(kind, "source_file" | "block" | "block_statement") {
        walk_list(node, source, doc, ctx, out, depth);
        return;
    }
    if matches!(kind, "package_statement" | "class_statement") {
        let Some(name) = node
            .child_by_field_name("name")
            .and_then(|n| ident(text(n, source)))
        else {
            return;
        };
        let body = children(node).into_iter().find(|n| n.kind() == "block");
        let signature = source[node.start_byte()..body.map_or(node.end_byte(), |b| b.start_byte())]
            .trim()
            .into();
        let id = symbol(doc, node, &name, name.clone(), signature, out);
        out.perl.as_mut().unwrap().packages.push(Package {
            id,
            name: name.clone(),
        });
        if let Some(body) = body {
            walk(
                body,
                source,
                doc,
                &Context {
                    package: name,
                    ..ctx.clone()
                },
                out,
                depth + 1,
            );
        }
        return;
    }
    if matches!(
        kind,
        "subroutine_declaration_statement"
            | "method_declaration_statement"
            | "anonymous_subroutine_expression"
            | "anonymous_method_expression"
            | "phaser_statement"
    ) {
        let body = node
            .child_by_field_name("body")
            .or_else(|| children(node).into_iter().find(|n| n.kind() == "block"));
        let named = node
            .child_by_field_name("name")
            .and_then(|n| ident(text(n, source)));
        let spelling = named
            .clone()
            .unwrap_or_else(|| format!("<{}@{}>", kind, node.start_byte()));
        let qualified = if spelling.contains("::") {
            spelling.clone()
        } else {
            format!("{}::{spelling}", ctx.package)
        };
        let signature = source[node.start_byte()..body.map_or(node.end_byte(), |b| b.start_byte())]
            .trim()
            .into();
        let id = symbol(doc, node, &spelling, qualified.clone(), signature, out);
        let lexical = node.child_by_field_name("lexical").map(|_| scope(node));
        if named.is_some() {
            let (package, name) = qualified.rsplit_once("::").unwrap();
            out.perl.as_mut().unwrap().functions.push(Function {
                id: id.clone(),
                package: package.into(),
                name: name.into(),
                lexical,
                visible: body.map_or(node.end_byte(), |b| b.start_byte()),
                body: body.is_some(),
                method: kind == "method_declaration_statement",
            });
        }
        let nested = Context {
            caller: Some(id),
            ..ctx.clone()
        };
        if let Some(body) = body {
            for signature in children(node)
                .into_iter()
                .filter(|n| n.kind() == "signature")
            {
                for param in variables(signature) {
                    declaration(
                        doc,
                        param,
                        body,
                        source,
                        &nested,
                        out,
                        (false, body.start_byte()),
                    );
                }
            }
            walk(body, source, doc, &nested, out, depth + 1);
        }
        return;
    }
    if kind == "variable_declaration" {
        let package_var = text(node, source).split_whitespace().next() == Some("our");
        for var in variables(node) {
            declaration(
                doc,
                var,
                var,
                source,
                ctx,
                out,
                (package_var, visible_after(node)),
            );
        }
        return;
    }
    if kind == "use_statement" {
        imported(node, source, ctx, out);
        return;
    }
    if kind == "require_expression" {
        if let Some(module) = children(node)
            .into_iter()
            .next()
            .and_then(|n| ident(text(n, source)))
        {
            file_imports::perl_import(node, &module, "perl_require_package_candidate", out);
        } else {
            out.perl
                .as_mut()
                .unwrap()
                .dynamic_packages
                .insert(ctx.package.clone());
        }
        return;
    }
    if matches!(
        kind,
        "assignment_expression" | "localization_expression" | "eval_expression" | "glob"
    ) && (kind != "assignment_expression"
        || node.child_by_field_name("left").is_some_and(|n| {
            ["ISA", "EXPORT"]
                .iter()
                .any(|part| text(n, source).contains(part))
        }))
    {
        out.perl
            .as_mut()
            .unwrap()
            .dynamic_packages
            .insert(ctx.package.clone());
    }
    if kind == "function_call_expression" || kind == "ambiguous_function_call_expression" {
        if let Some(target) = node
            .child_by_field_name("function")
            .or_else(|| children(node).into_iter().find(|n| n.kind() == "function"))
        {
            let raw = text(target, source).trim_start_matches('&');
            let name = ident(raw);
            reference(
                doc,
                target,
                (
                    name.clone().unwrap_or_else(|| "<dynamic-call>".into()),
                    name.clone().unwrap_or_else(|| "<dynamic-call>".into()),
                    if kind == "function_call_expression" {
                        name.map_or(Target::Unknown, Target::Function)
                    } else {
                        Target::Unknown
                    },
                    true,
                ),
                ctx,
                out,
            );
            for child in children(node) {
                if child.id() != target.id() {
                    walk(child, source, doc, ctx, out, depth + 1);
                }
            }
        } else {
            for child in children(node) {
                walk(child, source, doc, ctx, out, depth + 1);
            }
        }
        return;
    }
    if kind == "method_call_expression" {
        if let Some(method) = node.child_by_field_name("method") {
            let name = ident(text(method, source));
            let receiver = node.child_by_field_name("invocant");
            let package = receiver.and_then(|r| {
                if matches!(r.kind(), "bareword" | "package" | "autoquoted_bareword") {
                    ident(text(r, source))
                } else {
                    literal(r, source).and_then(|s| ident(&s))
                }
            });
            let spelling = name.clone().unwrap_or_else(|| "<dynamic-method>".into());
            let expression = format!("{}->{spelling}", package.as_deref().unwrap_or("<receiver>"));
            reference(
                doc,
                method,
                (
                    spelling.clone(),
                    expression,
                    Target::Method {
                        package,
                        name: spelling,
                    },
                    true,
                ),
                ctx,
                out,
            );
            for child in children(node) {
                if child.id() != method.id() {
                    walk(child, source, doc, ctx, out, depth + 1);
                }
            }
        }
        return;
    }
    if kind == "coderef_call_expression" {
        reference(
            doc,
            node,
            (
                "<coderef>".into(),
                "<coderef>".into(),
                Target::Unknown,
                true,
            ),
            ctx,
            out,
        );
        for child in children(node) {
            walk(child, source, doc, ctx, out, depth + 1);
        }
        return;
    }
    if kind == "function" {
        if let Some(name) = ident(text(node, source).trim_start_matches('&')) {
            reference(
                doc,
                node,
                (name.clone(), name.clone(), Target::Function(name), false),
                ctx,
                out,
            );
        }
        return;
    }
    if matches!(kind, "scalar" | "array" | "hash" | "container_variable") {
        let raw = text(node, source);
        let mut name = raw.to_string();
        if kind == "container_variable" {
            if let Some(p) = node.parent() {
                if p.kind() == "hash_element_expression" {
                    name = format!("%{}", raw.trim_start_matches(['$', '@', '%']));
                } else if p.kind() == "array_element_expression" {
                    name = format!("@{}", raw.trim_start_matches(['$', '@', '%']));
                }
            }
        }
        if name
            .chars()
            .next()
            .is_some_and(|s| matches!(s, '$' | '@' | '%'))
            && ident(&name[1..]).is_some()
        {
            reference(
                doc,
                node,
                (name.clone(), raw.into(), Target::Variable(name), false),
                ctx,
                out,
            );
        }
        return;
    }
    for child in children(node) {
        walk(child, source, doc, ctx, out, depth + 1);
    }
}
pub(super) fn parse_source(doc: &str, source: &str, out: &mut IndexedDocument) -> Result<()> {
    let mut parser = Parser::new();
    parser
        .set_language(&ts_parser_perl::LANGUAGE.into())
        .map_err(anyhow::Error::from)?;
    let tree = parser
        .parse(source, None)
        .context("Perl parser returned no tree")?;
    out.parse_errors = tree.root_node().has_error();
    out.perl = Some(Facts::default());
    out.binding_limitations
        .push("PERL_INC_EXPORTS_SYMBOL_TABLE_MRO_AND_RUNTIME_UNVERIFIED".into());
    walk(
        tree.root_node(),
        source,
        doc,
        &Context {
            package: "main".into(),
            caller: None,
            reliable: true,
        },
        out,
        0,
    );
    sql::finish(source, out);
    Ok(())
}

pub(super) fn package_matches(doc: &IndexedDocument, name: &str) -> bool {
    doc.perl
        .as_ref()
        .is_some_and(|f| f.packages.iter().any(|p| p.name == name))
}
struct Resolver<'a> {
    doc: &'a IndexedDocument,
    docs: Vec<&'a IndexedDocument>,
}
impl<'a> Resolver<'a> {
    fn new(doc: &'a IndexedDocument, docs: &'a HashMap<String, IndexedDocument>) -> Self {
        let mut selected: Vec<_> = docs
            .values()
            .filter(|d| d.perl.is_some() && !d.parse_errors && d.project == doc.project)
            .collect();
        selected.sort_by(|a, b| (&a.path, &a.hash).cmp(&(&b.path, &b.hash)));
        Self {
            doc,
            docs: selected,
        }
    }
    fn dynamic(&self, package: &str) -> bool {
        self.docs.len() > 4096
            || self
                .docs
                .iter()
                .any(|d| d.perl.as_ref().unwrap().dynamic_packages.contains(package))
    }
    fn functions(&self, package: &str, name: &str, method: bool, depth: usize) -> Vec<String> {
        if depth >= 32 || self.dynamic(package) {
            return vec![];
        }
        let found: Vec<_> = self
            .docs
            .iter()
            .flat_map(|d| {
                d.perl
                    .as_ref()
                    .unwrap()
                    .functions
                    .iter()
                    .filter(move |f| {
                        f.package == package
                            && f.name == name
                            && f.lexical.is_none()
                            && f.body
                            && !f.method
                    })
                    .map(|f| f.id.clone())
            })
            .collect();
        if !found.is_empty() || !method {
            return found;
        }
        let bases: BTreeSet<_> = self
            .docs
            .iter()
            .flat_map(|d| {
                d.perl
                    .as_ref()
                    .unwrap()
                    .parents
                    .iter()
                    .filter(move |p| p.package == package)
                    .map(|p| p.base.as_str())
            })
            .collect();
        if bases.len() != 1 {
            return vec![];
        }
        self.functions(bases.into_iter().next().unwrap(), name, true, depth + 1)
    }
    fn variable(&self, name: &str, package: &str, pos: usize) -> Vec<String> {
        let (sigil, raw) = name.split_at(1);
        if let Some((package, variable)) = raw.rsplit_once("::") {
            if self.dynamic(package) {
                return vec![];
            }
            let name = format!("{sigil}{variable}");
            return self
                .docs
                .iter()
                .flat_map(|d| {
                    d.perl
                        .as_ref()
                        .unwrap()
                        .variables
                        .iter()
                        .filter(|v| v.package_var && v.package == package && v.name == name)
                        .map(|v| v.id.clone())
                })
                .collect();
        }
        let mut locals: Vec<_> = self
            .doc
            .perl
            .as_ref()
            .unwrap()
            .variables
            .iter()
            .filter(|v| v.name == name && v.start <= pos && pos < v.end && v.visible <= pos)
            .collect();
        locals.sort_by_key(|v| (v.start, v.visible));
        if let Some(local) = locals.last() {
            if !local.package_var {
                return vec![local.id.clone()];
            }
            if self.dynamic(&local.package) {
                return vec![];
            }
            return self
                .docs
                .iter()
                .flat_map(|d| {
                    d.perl
                        .as_ref()
                        .unwrap()
                        .variables
                        .iter()
                        .filter(|v| v.package_var && v.package == local.package && v.name == name)
                        .map(|v| v.id.clone())
                })
                .collect();
        }
        if self.dynamic(package) {
            return vec![];
        }
        self.docs
            .iter()
            .flat_map(|d| {
                d.perl
                    .as_ref()
                    .unwrap()
                    .variables
                    .iter()
                    .filter(|v| v.package_var && v.package == package && v.name == name)
                    .map(|v| v.id.clone())
            })
            .collect()
    }
    fn candidates(&self, r: &Reference, site: &Site) -> Vec<String> {
        if self.doc.parse_errors || !site.reliable {
            return vec![];
        }
        match &site.target {
            Target::Function(name) => {
                if let Some((package, name)) = name.rsplit_once("::") {
                    return self.functions(package, name, false, 0);
                }
                let lexical: Vec<_> = self
                    .doc
                    .perl
                    .as_ref()
                    .unwrap()
                    .functions
                    .iter()
                    .filter(|f| {
                        f.name == *name
                            && f.lexical
                                .is_some_and(|(a, b)| a <= r.start_byte && r.start_byte < b)
                            && f.visible <= r.start_byte
                            && f.body
                    })
                    .collect();
                if !lexical.is_empty() {
                    return lexical.into_iter().map(|f| f.id.clone()).collect();
                }
                self.functions(&site.package, name, false, 0)
            }
            Target::Method {
                package: Some(package),
                name,
            } if !name.contains("::") => self.functions(package, name, true, 0),
            Target::Variable(name) => self.variable(name, &site.package, r.start_byte),
            _ => vec![],
        }
    }
}
pub(super) fn resolve_document(
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Vec<CallEdge> {
    let resolver = Resolver::new(doc, docs);
    doc.references.iter().map(|r|{
        let mut candidates=doc.perl.as_ref().unwrap().sites.get(&format!("{}:{}",r.start_byte,r.is_call)).map_or_else(Vec::new,|s|resolver.candidates(r,s));candidates.sort();candidates.dedup();
        CallEdge {reference:r.clone(),resolution:match candidates.len(){0=>"unresolved",1=>"resolved_static",_=>"ambiguous"}.into(),candidates,provenance:"derived",resolution_basis:"perl_captured_package_or_lexical_candidates;INC_exports_symbol_table_runtime_unverified".into()}
    }).collect()
}
pub(super) fn edges(
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Vec<inheritance::InheritanceEdge> {
    let resolver = Resolver::new(doc, docs);
    let mut result = vec![];
    for parent in &doc.perl.as_ref().unwrap().parents {
        for package in doc
            .perl
            .as_ref()
            .unwrap()
            .packages
            .iter()
            .filter(|p| p.name == parent.package)
        {
            let mut candidates: Vec<_> = if doc.parse_errors || resolver.dynamic(&parent.package) {
                vec![]
            } else {
                resolver
                    .docs
                    .iter()
                    .flat_map(|d| {
                        d.perl
                            .as_ref()
                            .unwrap()
                            .packages
                            .iter()
                            .filter(|p| p.name == parent.base)
                            .map(|p| p.id.clone())
                    })
                    .collect()
            };
            candidates.sort();
            candidates.dedup();
            result.push(inheritance::InheritanceEdge {
                derived: package.id.clone(),
                resolution: match candidates.len() {
                    0 => "unresolved",
                    1 => "resolved_static",
                    _ => "ambiguous",
                }
                .into(),
                candidates,
                base_expression: parent.base.clone(),
                path: doc.path.clone(),
                line: parent.line,
                relation: "perl_parent_package_candidate".into(),
                hierarchy_status: "partial_INC_ISA_MRO_runtime_unverified".into(),
                provenance: "derived",
            });
        }
    }
    result
}
pub(super) fn entrypoint(doc: &IndexedDocument, symbol: &Symbol) -> Option<&'static str> {
    if symbol.kind == "phaser_statement" {
        Some("perl_phaser_candidate")
    } else if symbol.name == "main" && symbol.qualified_name == "main::main" {
        Some("perl_conventional_main_candidate")
    } else if symbol.name == "DESTROY" {
        Some("perl_destructor_candidate")
    } else if doc.path.ends_with(".t") && analysis::callable(symbol) {
        Some("perl_test_file_routine_candidate")
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
