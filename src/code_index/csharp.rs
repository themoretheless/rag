//! Captured C# declarations and member candidates, without compiler/build dispatch.
use super::*;
use anyhow::Context as _;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Facts {
    types: Vec<Type>,
    methods: Vec<Method>,
    imports: Vec<Import>,
    sites: BTreeMap<usize, Site>,
    locals: Vec<Local>,
    conditional: bool,
    unreliable_routines: BTreeSet<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Type {
    id: String,
    name: String,
    namespace: String,
    bases: Vec<(String, usize)>,
    kind: String,
    generic: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Method {
    id: String,
    owner: String,
    name: String,
    static_member: bool,
    arity: Option<usize>,
    local_scope: Option<(usize, usize)>,
    public: bool,
    explicit_interface: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Import {
    target: String,
    alias: Option<String>,
    start: usize,
    end: usize,
    global: bool,
    static_import: bool,
    namespace: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Local {
    name: String,
    ty: Option<String>,
    start: usize,
    end: usize,
    routine: Option<String>,
    writes: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Site {
    owner: String,
    namespace: String,
    receiver: Option<String>,
    name: String,
    argc: Option<usize>,
    static_context: bool,
    constructor: bool,
    type_use: bool,
    reliable: bool,
}
#[derive(Clone, Default)]
struct Context {
    namespace: String,
    owner: String,
    caller: Option<String>,
    static_context: bool,
    reliable: bool,
}
fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}
fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.into()
    } else {
        format!("{prefix}.{name}")
    }
}
fn name(node: Node<'_>, source: &str) -> Option<String> {
    if !matches!(
        node.kind(),
        "identifier" | "qualified_name" | "alias_qualified_name"
    ) {
        return None;
    }
    let raw = text(node, source).replace('@', "");
    let raw = raw.strip_prefix("global::").unwrap_or(&raw);
    (!raw.is_empty()
        && raw
            .split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || c == '_')))
    .then(|| {
        if text(node, source).starts_with("global::") {
            format!("global::{raw}")
        } else {
            raw.into()
        }
    })
}
fn scope(node: Node<'_>) -> (usize, usize) {
    let mut parent = node.parent();
    while let Some(p) = parent {
        if matches!(
            p.kind(),
            "block"
                | "switch_section"
                | "compilation_unit"
                | "declaration_list"
                | "for_statement"
                | "foreach_statement"
                | "catch_clause"
                | "lambda_expression"
                | "anonymous_method_expression"
        ) {
            return (p.start_byte(), p.end_byte());
        }
        parent = p.parent();
    }
    (node.start_byte(), node.end_byte())
}
fn modifiers(node: Node<'_>, source: &str) -> BTreeSet<String> {
    children(node)
        .into_iter()
        .filter(|n| n.kind() == "modifier")
        .map(|n| text(n, source).into())
        .collect()
}
fn symbol(
    doc: &str,
    node: Node<'_>,
    spelling: &str,
    qualified: String,
    source: &str,
    out: &mut IndexedDocument,
) -> String {
    let id = format!("{doc}:{}:{spelling}", node.start_byte());
    let body = node
        .child_by_field_name("body")
        .or_else(|| node.child_by_field_name("accessors"))
        .or_else(|| node.child_by_field_name("value"));
    let end = body.map_or(node.end_byte(), |b| b.start_byte());
    out.symbols.push(Symbol {
        id: id.clone(),
        document_id: doc.into(),
        path: out.path.clone(),
        name: spelling.into(),
        qualified_name: qualified,
        kind: node.kind().into(),
        signature: source[node.start_byte()..end]
            .trim()
            .chars()
            .take(512)
            .collect(),
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
    expression: String,
    site: Site,
    ctx: &Context,
    out: &mut IndexedDocument,
) {
    out.references.push(Reference {
        start_byte: node.start_byte(),
        document_id: doc.into(),
        path: out.path.clone(),
        name: site.name.clone(),
        expression,
        line: node.start_position().row + 1,
        column: node.start_position().column + 1,
        caller: ctx.caller.clone(),
        is_call: !site.type_use,
        revision: out.revision,
        content_hash: out.hash.clone(),
        build_context: None,
    });
    out.csharp
        .as_mut()
        .unwrap()
        .sites
        .insert(node.start_byte(), site);
}
fn type_reference(
    doc: &str,
    node: Node<'_>,
    source: &str,
    ctx: &Context,
    out: &mut IndexedDocument,
) {
    if let Some(ty) = name(node, source) {
        reference(
            doc,
            node,
            ty.clone(),
            Site {
                owner: ctx.owner.clone(),
                namespace: ctx.namespace.clone(),
                receiver: None,
                name: ty,
                argc: None,
                static_context: ctx.static_context,
                constructor: false,
                type_use: true,
                reliable: ctx.reliable,
            },
            ctx,
            out,
        );
    }
}
fn parameters(node: Node<'_>, source: &str) -> Option<usize> {
    let params = node.child_by_field_name("parameters")?;
    let mut count = 0;
    for p in children(params) {
        if p.kind() != "parameter"
            || children(p).iter().any(|n| n.kind() == "modifier")
            || p.child_by_field_name("type").is_none()
        {
            return None;
        }
        if text(p, source).contains('=') {
            return None;
        }
        // Optional/default parameters require overload applicability, not a length guess.
        if children(p).iter().any(|n| {
            !matches!(
                n.kind(),
                "identifier"
                    | "predefined_type"
                    | "qualified_name"
                    | "alias_qualified_name"
                    | "array_type"
                    | "nullable_type"
                    | "generic_name"
                    | "tuple_type"
                    | "pointer_type"
                    | "function_pointer_type"
            )
        }) {
            return None;
        }
        count += 1;
    }
    Some(count)
}
fn args(node: Node<'_>, source: &str) -> Option<usize> {
    let list = node.child_by_field_name("arguments")?;
    let values = children(list);
    if values.iter().any(|n| {
        n.kind() != "argument"
            || matches!(
                text(*n, source).split_whitespace().next(),
                Some("ref" | "out" | "in")
            )
            || n.child_by_field_name("name").is_some()
            || children(*n)
                .iter()
                .any(|c| matches!(c.kind(), "ref" | "out" | "in"))
    }) {
        return None;
    }
    Some(values.len())
}
fn local(
    node: Node<'_>,
    spelling: String,
    ty: Option<String>,
    ctx: &Context,
    out: &mut IndexedDocument,
) {
    let (start, end) = scope(node);
    out.csharp.as_mut().unwrap().locals.push(Local {
        name: spelling,
        ty,
        start,
        end,
        routine: ctx.caller.clone(),
        writes: 0,
    });
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
    if kind.starts_with("preproc_") {
        out.csharp.as_mut().unwrap().conditional = true;
    }
    if kind == "compilation_unit" {
        let mut context = ctx.clone();
        for child in children(node) {
            if child.kind() == "file_scoped_namespace_declaration" {
                if let Some(n) = child
                    .child_by_field_name("name")
                    .and_then(|n| name(n, source))
                {
                    context.namespace = n.clone();
                    symbol(doc, child, &n, n.clone(), source, out);
                }
            } else {
                walk(child, source, doc, &context, out, depth + 1);
            }
        }
        return;
    }
    if kind == "namespace_declaration" {
        let Some(n) = node
            .child_by_field_name("name")
            .and_then(|n| name(n, source))
        else {
            return;
        };
        let mut context = ctx.clone();
        context.namespace = join(&ctx.namespace, &n);
        symbol(doc, node, &n, context.namespace.clone(), source, out);
        if let Some(body) = node.child_by_field_name("body") {
            walk(body, source, doc, &context, out, depth + 1);
        }
        return;
    }
    if kind == "using_directive" {
        let raw = text(node, source).trim();
        let global = raw.starts_with("global ");
        let static_import = raw.contains("using static ");
        let alias = node
            .child_by_field_name("name")
            .and_then(|n| name(n, source));
        let target = children(node)
            .into_iter()
            .last()
            .and_then(|n| name(n, source))
            .or_else(|| alias.as_ref().map(|_| "<unsupported-type>".into()));
        if let Some(target) = target {
            let (start, end) = scope(node);
            out.csharp.as_mut().unwrap().imports.push(Import {
                target,
                alias: node
                    .child_by_field_name("name")
                    .and_then(|n| name(n, source)),
                start,
                end,
                global,
                static_import,
                namespace: ctx.namespace.clone(),
            });
        }
        return;
    }
    if matches!(
        kind,
        "class_declaration"
            | "struct_declaration"
            | "interface_declaration"
            | "record_declaration"
            | "enum_declaration"
            | "delegate_declaration"
    ) {
        let Some(n) = node
            .child_by_field_name("name")
            .and_then(|n| name(n, source))
        else {
            return;
        };
        let owner = join(
            if ctx.owner.is_empty() {
                &ctx.namespace
            } else {
                &ctx.owner
            },
            &n,
        );
        let id = symbol(doc, node, &n, owner.clone(), source, out);
        let bases = children(node)
            .into_iter()
            .filter(|n| n.kind() == "base_list")
            .flat_map(children)
            .filter_map(|n| name(n, source).map(|s| (s, n.start_position().row + 1)))
            .collect();
        out.csharp.as_mut().unwrap().types.push(Type {
            id,
            name: owner.clone(),
            namespace: ctx.namespace.clone(),
            bases,
            kind: kind.into(),
            generic: children(node)
                .iter()
                .any(|n| matches!(n.kind(), "type_parameter_list" | "parameter_list")),
        });
        let context = Context {
            owner,
            caller: None,
            reliable: ctx.reliable
                && !children(node)
                    .iter()
                    .any(|n| matches!(n.kind(), "type_parameter_list" | "parameter_list")),
            ..ctx.clone()
        };
        if let Some(body) = node.child_by_field_name("body") {
            walk(body, source, doc, &context, out, depth + 1);
        }
        return;
    }
    if matches!(
        kind,
        "method_declaration"
            | "constructor_declaration"
            | "destructor_declaration"
            | "operator_declaration"
            | "local_function_statement"
    ) {
        let n = node
            .child_by_field_name("name")
            .or_else(|| node.child_by_field_name("operator"));
        let Some(n) = n else {
            return;
        };
        let spelling = if kind == "operator_declaration" {
            format!("operator {}", text(n, source))
        } else if kind == "destructor_declaration" {
            format!("~{}", text(n, source))
        } else {
            text(n, source).trim_start_matches('@').into()
        };
        let id = symbol(
            doc,
            node,
            &spelling,
            join(&ctx.owner, &spelling),
            source,
            out,
        );
        let mods = modifiers(node, source);
        let explicit_interface = children(node)
            .iter()
            .any(|n| n.kind() == "explicit_interface_specifier");
        out.csharp.as_mut().unwrap().methods.push(Method {
            id: id.clone(),
            owner: ctx.owner.clone(),
            name: spelling,
            static_member: mods.contains("static"),
            arity: if node.child_by_field_name("type_parameters").is_some() {
                None
            } else {
                parameters(node, source)
            },
            local_scope: (kind == "local_function_statement").then(|| scope(node)),
            public: mods.contains("public"),
            explicit_interface,
        });
        let context = Context {
            caller: Some(id),
            static_context: mods.contains("static")
                || (kind == "local_function_statement" && ctx.static_context),
            reliable: ctx.reliable
                && !explicit_interface
                && node.child_by_field_name("type_parameters").is_none(),
            ..ctx.clone()
        };
        if let Some(params) = node.child_by_field_name("parameters") {
            for p in children(params) {
                if let Some(n) = p.child_by_field_name("name").and_then(|n| name(n, source)) {
                    let ty = p.child_by_field_name("type").and_then(|t| name(t, source));
                    out.csharp.as_mut().unwrap().locals.push(Local {
                        name: n,
                        ty,
                        start: node.start_byte(),
                        end: node.end_byte(),
                        routine: context.caller.clone(),
                        writes: 0,
                    });
                }
                if let Some(ty) = p.child_by_field_name("type") {
                    type_reference(doc, ty, source, &context, out);
                }
            }
        }
        if let Some(body) = node.child_by_field_name("body") {
            walk(body, source, doc, &context, out, depth + 1);
        }
        return;
    }
    if kind == "property_declaration" {
        if let Some(n) = node
            .child_by_field_name("name")
            .and_then(|n| name(n, source))
        {
            let id = symbol(doc, node, &n, join(&ctx.owner, &n), source, out);
            let context = Context {
                caller: Some(id),
                ..ctx.clone()
            };
            for field in ["accessors", "value"] {
                if let Some(n) = node.child_by_field_name(field) {
                    walk(n, source, doc, &context, out, depth + 1);
                }
            }
        }
        return;
    }
    if kind == "variable_declarator" {
        if let Some(n) = node
            .child_by_field_name("name")
            .and_then(|n| name(n, source))
        {
            let parent = node.parent();
            let field = parent.and_then(|p| p.parent()).is_some_and(|p| {
                p.kind() == "field_declaration" || p.kind() == "event_field_declaration"
            });
            if field {
                symbol(doc, node, &n, join(&ctx.owner, &n), source, out);
                let field = out.symbols.last_mut().unwrap();
                field.kind = "field_declaration".into();
                field.signature = n.clone();
            } else {
                let ty = parent
                    .and_then(|p| p.child_by_field_name("type"))
                    .and_then(|t| name(t, source));
                // `var` remains unknown; source constructor typing is a separate future fact.
                local(
                    node,
                    n,
                    ty.filter(|t| t != "var" && t != "dynamic"),
                    ctx,
                    out,
                );
            }
        }
        if node.child_by_field_name("name").is_none() {
            if let Some(caller) = &ctx.caller {
                out.csharp
                    .as_mut()
                    .unwrap()
                    .unreliable_routines
                    .insert(caller.clone());
            }
        }
        for child in children(node) {
            if node
                .child_by_field_name("name")
                .is_none_or(|n| n.id() != child.id())
            {
                walk(child, source, doc, ctx, out, depth + 1);
            }
        }
        return;
    }
    if kind == "invocation_expression" || kind == "object_creation_expression" {
        let constructor = kind == "object_creation_expression";
        let target = node.child_by_field_name(if constructor { "type" } else { "function" });
        if let Some(target) = target {
            let (spelling, receiver, reliable) = if constructor {
                (
                    name(target, source).unwrap_or_else(|| "<constructed-type>".into()),
                    None,
                    name(target, source).is_some(),
                )
            } else if target.kind() == "member_access_expression" {
                let member = target
                    .child_by_field_name("name")
                    .and_then(|n| name(n, source));
                let receiver: Option<String> = target.child_by_field_name("expression").map(|n| {
                    let raw = text(n, source);
                    if raw
                        .chars()
                        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '@' | ':'))
                    {
                        raw.into()
                    } else {
                        "<dynamic-receiver>".into()
                    }
                });
                (
                    member.unwrap_or_else(|| "<generic-member>".into()),
                    receiver,
                    member_simple(target, source),
                )
            } else {
                (
                    name(target, source).unwrap_or_else(|| "<dynamic-call>".into()),
                    None,
                    name(target, source).is_some(),
                )
            };
            let expression = if let Some(receiver) = &receiver {
                // Never copy nested invocation arguments into metadata.
                if receiver
                    .chars()
                    .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '@' | ':'))
                {
                    format!("{receiver}.{spelling}")
                } else {
                    format!("<receiver>.{spelling}")
                }
            } else {
                spelling.clone()
            };
            let anchor = if target.kind() == "member_access_expression" {
                target.child_by_field_name("name").unwrap_or(target)
            } else {
                target
            };
            reference(
                doc,
                anchor,
                expression,
                Site {
                    owner: ctx.owner.clone(),
                    namespace: ctx.namespace.clone(),
                    receiver,
                    name: spelling,
                    argc: args(node, source),
                    static_context: ctx.static_context,
                    constructor,
                    type_use: false,
                    reliable: ctx.reliable && reliable,
                },
                ctx,
                out,
            );
            // Inner calls/arguments remain source occurrences, without a duplicate target.
            for child in children(node) {
                if child.id() != target.id() {
                    walk(child, source, doc, ctx, out, depth + 1);
                } else if target.kind() == "member_access_expression" {
                    if let Some(receiver) = target.child_by_field_name("expression") {
                        if receiver.kind() != "identifier" {
                            walk(receiver, source, doc, ctx, out, depth + 1);
                        }
                    }
                }
            }
        }
        return;
    }
    if matches!(
        kind,
        "lambda_expression"
            | "anonymous_method_expression"
            | "foreach_statement"
            | "catch_declaration"
            | "declaration_pattern"
            | "declaration_expression"
    ) {
        if matches!(
            kind,
            "foreach_statement"
                | "catch_declaration"
                | "declaration_pattern"
                | "declaration_expression"
        ) {
            if let Some(caller) = &ctx.caller {
                out.csharp
                    .as_mut()
                    .unwrap()
                    .unreliable_routines
                    .insert(caller.clone());
            }
        }
        // These introduce shadowing/control constructs. Keep occurrences, but do
        // not claim a member owner or use an outer local's type inside them.
        let context = Context {
            reliable: false,
            ..ctx.clone()
        };
        for child in children(node) {
            walk(child, source, doc, &context, out, depth + 1);
        }
        return;
    }
    for child in children(node) {
        walk(child, source, doc, ctx, out, depth + 1);
    }
}
fn member_simple(node: Node<'_>, source: &str) -> bool {
    node.child_by_field_name("name")
        .and_then(|n| name(n, source))
        .is_some()
}
pub(super) fn parse_source(doc: &str, source: &str, out: &mut IndexedDocument) -> Result<()> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
        .map_err(anyhow::Error::from)?;
    let tree = parser
        .parse(source, None)
        .context("C# parser returned no tree")?;
    out.parse_errors = tree.root_node().has_error();
    out.csharp = Some(Facts::default());
    out.binding_limitations
        .push("CSHARP_BUILD_OVERLOAD_RUNTIME_SEMANTICS_UNVERIFIED".into());
    walk(
        tree.root_node(),
        source,
        doc,
        &Context {
            reliable: true,
            ..Default::default()
        },
        out,
        0,
    );
    // Writes anywhere in the routine invalidate parameter/local type candidates.
    let mut pending = vec![tree.root_node()];
    while let Some(n) = pending.pop() {
        if matches!(
            n.kind(),
            "assignment_expression" | "postfix_unary_expression" | "prefix_unary_expression"
        ) {
            let lhs = n.child_by_field_name("left").or_else(|| n.named_child(0));
            if let Some(lhs) = lhs.filter(|n| n.kind() == "identifier") {
                let spelling = text(lhs, source).trim_start_matches('@');
                for local in &mut out.csharp.as_mut().unwrap().locals {
                    if local.name == spelling
                        && local.start <= n.start_byte()
                        && n.end_byte() <= local.end
                    {
                        local.writes += 1;
                    }
                }
            }
        }
        pending.extend(children(n));
    }
    sql::finish(source, out);
    Ok(())
}

struct Resolver<'a> {
    doc: &'a IndexedDocument,
    documents: Vec<&'a IndexedDocument>,
}
impl<'a> Resolver<'a> {
    fn new(doc: &'a IndexedDocument, docs: &'a HashMap<String, IndexedDocument>) -> Self {
        let mut documents: Vec<_> = docs
            .values()
            .filter(|d| d.csharp.is_some() && !d.parse_errors && d.project == doc.project)
            .collect();
        documents.sort_by(|a, b| (&a.path, &a.hash).cmp(&(&b.path, &b.hash)));
        Self { doc, documents }
    }
    fn exact(&self, qualified: &str) -> Vec<(&'a IndexedDocument, &'a Type)> {
        if self.documents.len() > 4096 {
            return vec![];
        }
        self.documents
            .iter()
            .flat_map(|d| {
                d.csharp
                    .as_ref()
                    .unwrap()
                    .types
                    .iter()
                    .filter(move |t| t.name == qualified && !t.generic)
                    .map(move |t| (*d, t))
            })
            .collect()
    }
    fn import_target(&self, import: &Import) -> String {
        if let Some(absolute) = import.target.strip_prefix("global::") {
            return absolute.into();
        }
        let mut namespace = import.namespace.as_str();
        loop {
            let candidate = join(namespace, &import.target);
            let prefix = format!("{candidate}.");
            if self.documents.iter().any(|d| {
                d.csharp
                    .as_ref()
                    .unwrap()
                    .types
                    .iter()
                    .any(|t| t.name == candidate || t.name.starts_with(&prefix))
            }) {
                return candidate;
            }
            if namespace.is_empty() {
                return import.target.clone();
            }
            namespace = namespace.rsplit_once('.').map_or("", |(p, _)| p);
        }
    }
    fn types(
        &self,
        raw: &str,
        namespace: &str,
        owner: &str,
        position: usize,
    ) -> Vec<(&'a IndexedDocument, &'a Type)> {
        if self.documents.iter().any(|d| {
            d.csharp.as_ref().unwrap().conditional
                && d.csharp.as_ref().unwrap().imports.iter().any(|i| i.global)
        }) {
            return vec![];
        }
        if let Some(absolute) = raw.strip_prefix("global::") {
            return self.exact(absolute);
        }
        if raw.contains(['<', '>', '[', ']', '?', '(', ')', ':']) {
            return vec![];
        }
        let imports: Vec<_> = self
            .documents
            .iter()
            .flat_map(|d| {
                d.csharp.as_ref().unwrap().imports.iter().filter(move |i| {
                    i.global
                        || (std::ptr::eq(*d, self.doc) && i.start <= position && position < i.end)
                })
            })
            .collect();
        let first = raw.split('.').next().unwrap_or(raw);
        let aliases: Vec<_> = imports
            .iter()
            .filter(|i| i.alias.as_deref() == Some(first))
            .collect();
        if !aliases.is_empty() {
            if aliases.len() != 1 {
                return vec![];
            }
            let tail = &raw[first.len()..];
            return self.exact(&format!("{}{tail}", self.import_target(aliases[0])));
        }
        let mut prefix = owner;
        while !prefix.is_empty() && prefix != namespace {
            let found = self.exact(&join(prefix, raw));
            if !found.is_empty() {
                return found;
            }
            prefix = prefix.rsplit_once('.').map_or("", |(p, _)| p);
        }
        let mut prefix = namespace;
        loop {
            let found = self.exact(&join(prefix, raw));
            if !found.is_empty() {
                return found;
            }
            let mut using = vec![];
            for import in imports
                .iter()
                .filter(|i| i.alias.is_none() && !i.static_import && i.namespace == prefix)
            {
                using.extend(self.exact(&join(&self.import_target(import), raw)));
            }
            using.sort_by(|a, b| a.1.id.cmp(&b.1.id));
            using.dedup_by(|a, b| a.1.id == b.1.id);
            if !using.is_empty() {
                return using;
            }
            if prefix.is_empty() {
                break;
            }
            prefix = prefix.rsplit_once('.').map_or("", |(p, _)| p);
        }
        vec![]
    }
    fn members(
        &self,
        doc: &'a IndexedDocument,
        ty: &'a Type,
        member: &str,
        statics: Option<bool>,
        argc: Option<usize>,
        depth: usize,
    ) -> Vec<String> {
        if depth >= 32 || doc.csharp.as_ref().unwrap().conditional || ty.generic {
            return vec![];
        }
        if self.exact(&ty.name).len() != 1 {
            return vec![];
        }
        let facts = doc.csharp.as_ref().unwrap();
        let named: Vec<_> = facts
            .methods
            .iter()
            .filter(|m| m.owner == ty.name && m.name == member && m.local_scope.is_none())
            .collect();
        if !named.is_empty() {
            if named
                .iter()
                .any(|m| m.arity.is_none() || m.explicit_interface)
            {
                return vec![];
            }
            return named
                .into_iter()
                .filter(|m| statics.is_none_or(|s| m.static_member == s) && argc == m.arity)
                .map(|m| m.id.clone())
                .collect();
        }
        // Only one captured class base is followed. Interface lists, generic and
        // unknown external bases cannot prove a complete member search.
        if ty.bases.len() != 1 || ty.kind == "interface_declaration" {
            return vec![];
        }
        let (base, _) = &ty.bases[0];
        let parent_resolver = Resolver {
            doc,
            documents: self.documents.clone(),
        };
        let position = doc
            .symbols
            .iter()
            .find(|s| s.id == ty.id)
            .map_or(0, |s| s.start_byte);
        let parents = parent_resolver.types(base, &ty.namespace, &ty.name, position);
        if parents.len() != 1 || parents[0].1.kind != "class_declaration" {
            return vec![];
        }
        self.members(parents[0].0, parents[0].1, member, statics, argc, depth + 1)
    }
    fn value_shadow(&self, owner: &str, spelling: &str, depth: usize) -> bool {
        if depth >= 32 {
            return true;
        }
        let types = self.exact(owner);
        if types.len() != 1 {
            return !owner.is_empty();
        }
        let (doc, ty) = types[0];
        if doc.symbols.iter().any(|s| {
            s.qualified_name == join(owner, spelling)
                && matches!(
                    s.kind.as_str(),
                    "property_declaration" | "field_declaration"
                )
        }) {
            return true;
        }
        if ty.bases.is_empty() {
            return false;
        }
        if ty.bases.len() != 1 {
            return true;
        }
        let resolver = Resolver {
            doc,
            documents: self.documents.clone(),
        };
        let position = doc
            .symbols
            .iter()
            .find(|s| s.id == ty.id)
            .map_or(0, |s| s.start_byte);
        let parents = resolver.types(&ty.bases[0].0, &ty.namespace, &ty.name, position);
        if parents.len() != 1 {
            return true;
        }
        self.value_shadow(&parents[0].1.name, spelling, depth + 1)
    }
    fn candidates(&self, r: &Reference, site: &Site) -> Vec<String> {
        let facts = self.doc.csharp.as_ref().unwrap();
        if !site.reliable
            || facts.conditional
            || self.doc.parse_errors
            || r.caller
                .as_ref()
                .is_some_and(|c| facts.unreliable_routines.contains(c))
            || self.documents.iter().any(|d| {
                d.csharp.as_ref().unwrap().conditional
                    && d.csharp.as_ref().unwrap().imports.iter().any(|i| i.global)
            })
        {
            return vec![];
        }
        if site.type_use {
            return self
                .types(&site.name, &site.namespace, &site.owner, r.start_byte)
                .into_iter()
                .map(|(_, t)| t.id.clone())
                .collect();
        }
        if site.argc.is_none() {
            return vec![];
        }
        if site.constructor {
            let types = self.types(&site.name, &site.namespace, &site.owner, r.start_byte);
            if types.len() != 1 {
                return vec![];
            }
            let (doc, ty) = types[0];
            let member = ty.name.rsplit('.').next().unwrap_or(&ty.name);
            return self.members(doc, ty, member, Some(false), site.argc, 0);
        }
        let local_name = site
            .receiver
            .as_deref()
            .unwrap_or(&site.name)
            .split('.')
            .next()
            .unwrap_or("")
            .trim_start_matches('@');
        if site
            .receiver
            .as_deref()
            .is_none_or(|r| !r.starts_with("global::"))
            && self.value_shadow(&site.owner, local_name, 0)
        {
            return vec![];
        }
        let locals: Vec<_> = facts
            .locals
            .iter()
            .filter(|l| l.name == local_name && l.start <= r.start_byte && r.start_byte < l.end)
            .collect();
        if !locals.is_empty() {
            if site.receiver.is_none()
                || site.receiver.as_deref().is_some_and(|s| s.contains('.'))
                || locals.len() != 1
                || locals[0].writes > 0
            {
                return vec![];
            }
            let Some(ty) = &locals[0].ty else {
                return vec![];
            };
            let types = self.types(ty, &site.namespace, &site.owner, r.start_byte);
            if types.len() != 1 {
                return vec![];
            }
            return self.members(
                types[0].0,
                types[0].1,
                &site.name,
                Some(false),
                site.argc,
                0,
            );
        }
        if site.receiver.is_none() {
            let locals: Vec<_> = facts
                .methods
                .iter()
                .filter(|m| {
                    m.name == site.name
                        && m.local_scope
                            .is_some_and(|(a, b)| a <= r.start_byte && r.start_byte < b)
                })
                .collect();
            if !locals.is_empty() {
                if locals.iter().any(|m| m.arity.is_none()) {
                    return vec![];
                }
                return locals
                    .into_iter()
                    .filter(|m| m.arity == site.argc)
                    .map(|m| m.id.clone())
                    .collect();
            }
        }
        let (types, statics) = match site.receiver.as_deref() {
            None => (self.exact(&site.owner), site.static_context.then_some(true)),
            Some("this") if !site.static_context => (self.exact(&site.owner), Some(false)),
            Some("base") if !site.static_context => {
                let own = self.exact(&site.owner);
                if own.len() != 1 || own[0].1.bases.len() != 1 {
                    return vec![];
                }
                (
                    self.types(
                        &own[0].1.bases[0].0,
                        &site.namespace,
                        &site.owner,
                        r.start_byte,
                    ),
                    Some(false),
                )
            }
            Some(receiver) => (
                self.types(receiver, &site.namespace, &site.owner, r.start_byte),
                Some(true),
            ),
        };
        if types.len() != 1 {
            return vec![];
        }
        self.members(types[0].0, types[0].1, &site.name, statics, site.argc, 0)
    }
}
pub(super) fn resolve_document(
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Vec<CallEdge> {
    let resolver = Resolver::new(doc, docs);
    doc.references.iter().map(|r|{
        let mut candidates=doc.csharp.as_ref().unwrap().sites.get(&r.start_byte).map_or_else(Vec::new,|site|resolver.candidates(r,site));
        candidates.sort();candidates.dedup();
        CallEdge {reference:r.clone(),resolution:match candidates.len(){0=>"unresolved",1=>"resolved_static",_=>"ambiguous"}.into(),candidates,provenance:"derived",resolution_basis:"csharp_captured_namespace_owner_type_arity_candidates;build_overload_visibility_runtime_unverified".into()}
    }).collect()
}
pub(super) fn edges(
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Vec<inheritance::InheritanceEdge> {
    let resolver = Resolver::new(doc, docs);
    let mut result = vec![];
    for ty in &doc.csharp.as_ref().unwrap().types {
        let pos = doc
            .symbols
            .iter()
            .find(|s| s.id == ty.id)
            .map_or(0, |s| s.start_byte);
        for (base, line) in &ty.bases {
            let mut candidates: Vec<_> =
                if doc.parse_errors || doc.csharp.as_ref().unwrap().conditional {
                    vec![]
                } else {
                    resolver
                        .types(base, &ty.namespace, &ty.name, pos)
                        .into_iter()
                        .map(|(_, t)| t.id.clone())
                        .collect()
                };
            candidates.sort();
            candidates.dedup();
            result.push(inheritance::InheritanceEdge {
                derived: ty.id.clone(),
                resolution: match candidates.len() {
                    0 => "unresolved",
                    1 => "resolved_static",
                    _ => "ambiguous",
                }
                .into(),
                candidates,
                base_expression: base.clone(),
                path: doc.path.clone(),
                line: *line,
                relation: "csharp_base_type_candidate".into(),
                hierarchy_status: "partial_compiler_build_unverified".into(),
                provenance: "derived",
            });
        }
    }
    result
}
pub(super) fn entrypoint(doc: &IndexedDocument, symbol: &Symbol) -> Option<&'static str> {
    let method = doc
        .csharp
        .as_ref()?
        .methods
        .iter()
        .find(|m| m.id == symbol.id)?;
    if method.name == "Main" && method.static_member && method.local_scope.is_none() {
        Some("csharp_conventional_main_candidate")
    } else if symbol.kind == "constructor_declaration" || symbol.kind == "destructor_declaration" {
        Some("csharp_lifecycle_candidate")
    } else if method.public && method.local_scope.is_none() {
        Some("csharp_public_api_candidate")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestSnapshot {
        symbols: Vec<Symbol>,
        calls: Vec<CallEdge>,
        documents: HashMap<String, IndexedDocument>,
    }
    fn snapshot(files: &[(&str, &str, &str)]) -> TestSnapshot {
        let mut documents = HashMap::new();
        for (i, (path, project, source)) in files.iter().enumerate() {
            let id = format!("d{i}");
            let mut doc = parse(&id, path, source, 1).unwrap();
            doc.project = Some((*project).into());
            assert!(
                !doc.parse_errors,
                "{}: {:?}",
                path,
                doc.symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
            );
            documents.insert(id, doc);
        }
        let mut symbols: Vec<_> = documents.values().flat_map(|d| d.symbols.clone()).collect();
        symbols.sort_by(|a, b| a.id.cmp(&b.id));
        let references = resolve::references(&symbols, &documents);
        let calls = references
            .iter()
            .filter(|e| e.reference.is_call)
            .cloned()
            .collect();
        TestSnapshot {
            symbols,
            calls,
            documents,
        }
    }
    fn candidates(s: &TestSnapshot, caller: &str, expression: &str) -> Vec<String> {
        let owner = s
            .symbols
            .iter()
            .find(|s| s.qualified_name == caller)
            .unwrap();
        let edges: Vec<_> = s
            .calls
            .iter()
            .filter(|e| {
                e.reference.caller.as_deref() == Some(&owner.id)
                    && e.reference.expression == expression
            })
            .collect();
        assert_eq!(edges.len(), 1, "{caller} {expression}: {:?}", s.calls);
        edges[0]
            .candidates
            .iter()
            .map(|id| {
                s.symbols
                    .iter()
                    .find(|s| s.id == *id)
                    .unwrap()
                    .qualified_name
                    .clone()
            })
            .collect()
    }
    #[test]
    fn csharp_declarations_cover_reference_profile_and_exact_unicode_spans() {
        let source="// café\nnamespace N { interface I { void Run(); } struct S {} record R(int X); record struct RS(int X); enum E { A } class C { int field=42; public int P { get { return 1; } } C() {} ~C() {} public static C operator +(C a,C b)=>a; void Run() { void Local() {} Local(); } } }";
        let s = snapshot(&[("/p/all.cs", "p", source)]);
        for expected in [
            "N",
            "N.I",
            "N.I.Run",
            "N.S",
            "N.R",
            "N.RS",
            "N.E",
            "N.C",
            "N.C.field",
            "N.C.P",
            "N.C.C",
            "N.C.~C",
            "N.C.operator +",
            "N.C.Local",
        ] {
            assert!(
                s.symbols.iter().any(|s| s.qualified_name == expected),
                "missing {expected}"
            );
        }
        let field = s
            .symbols
            .iter()
            .find(|s| s.qualified_name == "N.C.field")
            .unwrap();
        assert!(!analysis::callable(field));
        assert_eq!(field.signature, "field");
        let local = s
            .symbols
            .iter()
            .find(|s| s.qualified_name == "N.C.Local")
            .unwrap();
        assert_eq!(&source[local.start_byte..local.end_byte], "void Local() {}");
        assert_eq!(candidates(&s, "N.C.Run", "Local"), ["N.C.Local"]);
    }
    #[test]
    fn csharp_namespace_usings_aliases_and_arity_preserve_ambiguity() {
        let s=snapshot(&[("/p/a.cs","p","namespace A; public class C { public static void Run(){} public static void Run(int x){} public static void Run(string x){} }"),("/p/b.cs","p","using Alias = A.C; namespace B; class MainClass { static void Main(){ Alias.Run(); Alias.Run(1); } }")]);
        let mut calls: Vec<_> = s
            .calls
            .iter()
            .filter(|e| e.reference.expression == "Alias.Run")
            .collect();
        calls.sort_by_key(|e| e.reference.start_byte);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].candidates.len(), 1);
        assert_eq!(calls[1].candidates.len(), 2);
        assert_eq!(calls[1].resolution, "ambiguous");
    }
    #[test]
    fn csharp_type_receivers_inherited_members_and_project_scopes() {
        let s=snapshot(&[("/p/base.cs","p","namespace Lib; class Base { public void Work(){} public static void Api(){} }"),("/p/client.cs","p","using Lib; namespace App; class Child : Base { void Run(Base b) { this.Work(); base.Work(); b.Work(); Base.Api(); } }"),("/foreign/base.cs","other","namespace Lib; class Base { public void Work(){} }")]);
        for expr in ["this.Work", "base.Work", "b.Work"] {
            assert_eq!(candidates(&s, "App.Child.Run", expr), ["Lib.Base.Work"]);
        }
        assert_eq!(
            candidates(&s, "App.Child.Run", "Base.Api"),
            ["Lib.Base.Api"]
        );
        let edges = edges(
            &s.documents,
            s.documents
                .values()
                .find(|d| d.path == "/p/client.cs")
                .unwrap(),
        );
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].candidates.len(), 1);
        assert_ne!(edges[0].hierarchy_status, "complete_static");
    }
    #[test]
    fn csharp_unknown_shadowed_mutated_optional_generic_and_conditional_calls_fail_closed() {
        let s=snapshot(&[("/p/a.cs","p","class C { public static void Api() {} public void Work(){} void Optional(int x=1){} void Generic<T>(){} void Run(C c, System.Action Api) { c = other; c.Work(); Api(); Optional(); Generic<int>(); System.Action f=()=>c.Work(); } }")]);
        let run = s
            .symbols
            .iter()
            .find(|s| s.qualified_name == "C.Run")
            .unwrap();
        assert!(
            s.calls
                .iter()
                .filter(|e| e.reference.caller.as_deref() == Some(&run.id))
                .all(|e| e.candidates.is_empty()),
            "{:?}",
            s.calls
        );
        let c=snapshot(&[("/p/config.cs","p","class C { static void Api() {} static void Main() { Api(); } }\n#if DEBUG\nclass D {}\n#endif")]);
        assert!(c.calls.iter().all(|e| e.candidates.is_empty()));
    }
    #[test]
    fn csharp_property_field_and_local_namespace_shadows_do_not_become_static_calls() {
        let s=snapshot(&[("/p/a.cs","p","class Target { public static void Api(){} } class C { System.Action Target; void Run(){ Target.Api(); } void Local(int Target){Target.Api();} }")]);
        assert!(s.calls.iter().all(|e| e.candidates.is_empty()));
    }
    #[test]
    fn csharp_unknown_receivers_do_not_copy_argument_bodies_and_nested_calls_remain() {
        let s=snapshot(&[("/p/a.cs","p","class C { C Factory(string text)=>this; void Work(){} void Run(){ Factory(\"PRIVATE_ARGUMENT\").Work(); } }")]);
        assert!(!serde_json::to_string(&s.calls)
            .unwrap()
            .contains("PRIVATE_ARGUMENT"));
        let facts = &s.documents.values().next().unwrap().csharp;
        assert!(!serde_json::to_string(facts)
            .unwrap()
            .contains("PRIVATE_ARGUMENT"));
        assert_eq!(candidates(&s, "C.Run", "Factory"), ["C.Factory"]);
        assert!(candidates(&s, "C.Run", "<receiver>.Work").is_empty());
    }
    #[test]
    fn csharp_namespace_relative_imports_precede_outer_types_and_absolute_aliases() {
        let s=snapshot(&[("/p/a.cs","p","class C { public static void Ping(){} } namespace N { namespace Sub { class C { public static void Ping(){} } } namespace App { using N.Sub; using Alias = global::C; class Entry { static void Main(){ C.Ping(); Alias.Ping(); } } } }")]);
        assert_eq!(
            candidates(&s, "N.App.Entry.Main", "C.Ping"),
            ["N.Sub.C.Ping"]
        );
        assert_eq!(candidates(&s, "N.App.Entry.Main", "Alias.Ping"), ["C.Ping"]);
        let s=snapshot(&[("/p/a.cs","p","namespace N { namespace Sub { class C { public static void Ping(){} } } namespace App { using Sub; class Entry { static void Main(){ C.Ping(); } } } }")]);
        assert_eq!(
            candidates(&s, "N.App.Entry.Main", "C.Ping"),
            ["N.Sub.C.Ping"]
        );
    }
    #[test]
    fn csharp_duplicate_types_and_using_collisions_remain_unresolved_or_ambiguous() {
        let s=snapshot(&[("/p/a.cs","p","namespace A { class C { public static void Ping(){} } } namespace B { class C { public static void Ping(){} } }"),("/p/main.cs","p","using A; using B; class Entry { static void Main(){C.Ping();} }")]);
        assert!(candidates(&s, "Entry.Main", "C.Ping").is_empty());
        let s = snapshot(&[
            ("/p/a.cs", "p", "partial class C { static void Ping(){} }"),
            (
                "/p/b.cs",
                "p",
                "partial class C { static void Main(){Ping();} }",
            ),
        ]);
        assert!(candidates(&s, "C.Main", "Ping").is_empty());
    }
    #[test]
    fn csharp_global_using_and_explicit_constructor_links_are_captured() {
        let s = snapshot(&[
            ("/p/usings.cs", "p", "global using Lib;"),
            (
                "/p/lib.cs",
                "p",
                "namespace Lib; class C { public C(){} public void Work(){} }",
            ),
            (
                "/p/main.cs",
                "p",
                "class Entry { static void Main(){ C c = new C(); c.Work(); } }",
            ),
        ]);
        assert_eq!(candidates(&s, "Entry.Main", "C"), ["Lib.C.C"]);
        assert_eq!(candidates(&s, "Entry.Main", "c.Work"), ["Lib.C.Work"]);
    }
    #[test]
    fn csharp_store_freshness_history_body_and_full_resolution_equivalence() {
        use crate::models::Document;
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("csharp.duckdb")).unwrap();
        let doc = |id: &str, path: &str, source: &str| Document {
            id: id.into(),
            uri: format!("file://{id}"),
            source_file: Some(path.into()),
            wing: Some("p".into()),
            title: path.into(),
            content: source.into(),
            ..Default::default()
        };
        let base = doc(
            "base",
            "/p/base.cs",
            "namespace Lib; class Base { public void Work(){ string q=\"SELECT * FROM users\"; } }",
        );
        let mut client = doc(
            "client",
            "/p/client.cs",
            "using Lib; class Child : Base { void Run(){Work();} }",
        );
        store.upsert_document(&base).unwrap();
        store.upsert_document(&client).unwrap();
        assert_eq!(store.index_code().unwrap().indexed, 2);
        let q = CodeQuery {
            symbol: Some("Lib.Base.Work".into()),
            ..Default::default()
        };
        let before = store.code_snapshot().unwrap().query("callers", &q);
        assert_eq!(before["items"].as_array().unwrap().len(), 1, "{before}");
        assert!(store
            .code_body(&q)
            .unwrap()
            .to_string()
            .contains("SELECT * FROM users"));
        assert!(!before.to_string().contains("SELECT * FROM users"));
        let full = store
            .index_code_with_options(&resolution_cache::IndexOptions {
                force_full_resolution: true,
            })
            .unwrap();
        assert!(!full.revision_created);
        assert_eq!(full.resolution.unwrap().equivalent_to_existing, Some(true));
        client.content = "class Child { void Run(){} }".into();
        store.upsert_document(&client).unwrap();
        assert!(store.code_snapshot().unwrap().query("callers", &q)["items"]
            .as_array()
            .unwrap()
            .is_empty());
        store.index_code().unwrap();
        let query = history::DiffQuery {
            before: 1,
            after: Some(2),
            project: Some("p".into()),
            limit: None,
        };
        let diff = store.code_index_diff(&query).unwrap();
        assert_eq!(diff["csharp_relations_available"], true);
        assert!(
            diff["relations_removed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["relation"] == "csharp_base_type_candidate"),
            "{diff}"
        );
        client.content = "class Later {}".into();
        store.upsert_document(&client).unwrap();
        store.index_code().unwrap();
        assert_eq!(store.code_index_diff(&query).unwrap(), diff);
        let conn = store.lock().unwrap();
        let raw: String = conn
            .query_row(
                "SELECT graph_json FROM code_index_revisions WHERE id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut graph: serde_json::Value = serde_json::from_str(&raw).unwrap();
        graph
            .as_object_mut()
            .unwrap()
            .remove("csharp_syntax_format");
        conn.execute(
            "UPDATE code_index_revisions SET graph_json=? WHERE id=1",
            [graph.to_string()],
        )
        .unwrap();
        drop(conn);
        let legacy = store.code_index_diff(&query).unwrap();
        assert_eq!(legacy["csharp_relations_available"], false);
        assert!(!legacy["relations_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["relation"] == "csharp_base_type_candidate"));
    }
    #[test]
    fn csharp_generic_and_unsupported_binding_shapes_cannot_fall_through_to_global_types() {
        let s=snapshot(&[("/p/a.cs","p","class C { public static void Api(){} } class Generic<C> { void Run(){C.Api();} } class Primary(int C) {void Run(){C.Api();}} class Entry { void Generic<C>(){C.Api();} void Catch(){try {} catch(System.Exception C){C.Api();}} void Pattern(object x){if(x is int C){C.Api();}} void Tuple(){var (C,x)=(1,2);C.Api();} }")]);
        assert!(
            s.calls.iter().all(|e| e.candidates.is_empty()),
            "{:?}",
            s.calls
        );
        let s=snapshot(&[("/p/a.cs","p","using C=System.Collections.Generic.List<int>; class C { public static void Api(){} } class Entry { static void Main(){C.Api();} }")]);
        assert!(s.calls.iter().all(|e| e.candidates.is_empty()));
    }
    #[test]
    fn csharp_nested_local_function_preserves_captured_value_shadow() {
        let s=snapshot(&[("/p/a.cs","p","class C { public static void Api(){} } class Entry { void Outer(int C){void Inner(){C.Api();} Inner();} }")]);
        assert!(candidates(&s, "Entry.Inner", "C.Api").is_empty());
    }
    #[test]
    fn csharp_inherited_fields_and_unknown_bases_prevent_type_receiver_guesses() {
        let s=snapshot(&[("/p/a.cs","p","class Target { public static void Api(){} } class Base { protected int Target; } class Child : Base { void Run(){Target.Api();} } class Unknown : External { void Run(){Target.Api();} }")]);
        assert!(
            s.calls.iter().all(|e| e.candidates.is_empty()),
            "{:?}",
            s.calls
        );
    }
}
