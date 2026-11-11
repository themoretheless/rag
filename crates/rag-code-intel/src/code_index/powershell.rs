//! Captured PowerShell syntax candidates; no runspace, command invocation or load.
use super::*;
use anyhow::Context as _;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Facts {
    functions: Vec<Function>,
    classes: Vec<Class>,
    methods: Vec<Method>,
    loads: Vec<Load>,
    sites: BTreeMap<usize, Site>,
    dynamic: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Function {
    id: String,
    name: String,
    scope_start: usize,
    scope_end: usize,
    reliable: bool,
    root: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Class {
    id: String,
    name: String,
    bases: Vec<String>,
    line: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Method {
    id: String,
    owner: String,
    name: String,
    statics: bool,
    arity: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Load {
    path: String,
    kind: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Target {
    Command(String),
    Member {
        owner: Option<String>,
        name: String,
        statics: bool,
        argc: usize,
    },
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Site {
    target: Target,
    reliable: bool,
}
#[derive(Clone)]
struct Context {
    caller: Option<String>,
    owner: String,
    statics: bool,
    scope_start: usize,
    scope_end: usize,
    reliable: bool,
    root: bool,
}
fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut c = node.walk();
    node.named_children(&mut c).collect()
}
fn child<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    children(node).into_iter().find(|n| n.kind() == kind)
}
fn ident(raw: &str) -> Option<String> {
    let raw = raw.trim();
    (!raw.is_empty()
        && raw.is_ascii()
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && !raw.starts_with('-'))
    .then(|| raw.to_ascii_lowercase())
}
fn unquote(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let value = if raw.starts_with(['\'', '"']) {
        let quote = raw.as_bytes()[0];
        if raw.as_bytes().last() != Some(&quote) {
            return None;
        }
        raw.get(1..raw.len().checked_sub(1)?)?
    } else {
        raw
    };
    (!value.contains(['`', '\n', '\r', '\0', ';', '(', ')'])).then(|| value.into())
}
fn path(source_path: &str, raw: &str, using: bool) -> Option<String> {
    let value = unquote(raw)?;
    let lower = value.to_ascii_lowercase();
    let relative = if lower.starts_with("$psscriptroot/") || lower.starts_with("$psscriptroot\\") {
        &value[14..]
    } else if using
        && (value.starts_with("./")
            || value.starts_with(".\\")
            || value.starts_with("../")
            || value.starts_with("..\\"))
    {
        &value
    } else {
        return None;
    };
    if relative.is_empty() || relative.contains(['$', '`', '*', '?', '[', ']', ':']) {
        return None;
    }
    let parent = std::path::Path::new(source_path).parent()?;
    Some(resolve::normalized(
        &parent.join(relative.replace('\\', "/")),
    ))
}
fn symbol(
    doc: &str,
    node: Node<'_>,
    name: &str,
    qualified: String,
    source: &str,
    out: &mut IndexedDocument,
) -> String {
    let id = format!("{doc}:{}:{name}", node.start_byte());
    let end = text(node, source)
        .find('{')
        .map_or(node.end_byte(), |i| node.start_byte() + i);
    out.symbols.push(Symbol {
        id: id.clone(),
        document_id: doc.into(),
        path: out.path.clone(),
        name: name.into(),
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
    name: &str,
    expression: String,
    target: Target,
    ctx: &Context,
    out: &mut IndexedDocument,
) {
    out.references.push(Reference {
        start_byte: node.start_byte(),
        document_id: doc.into(),
        path: out.path.clone(),
        name: name.into(),
        expression,
        line: node.start_position().row + 1,
        column: node.start_position().column + 1,
        caller: ctx.caller.clone(),
        is_call: true,
        revision: out.revision,
        content_hash: out.hash.clone(),
        build_context: None,
    });
    out.powershell.as_mut().unwrap().sites.insert(
        node.start_byte(),
        Site {
            target,
            reliable: ctx.reliable,
        },
    );
}
fn load(node: Node<'_>, raw: &str, kind: &str, using: bool, out: &mut IndexedDocument) {
    if let Some(path) = path(&out.path, raw, using) {
        out.powershell.as_mut().unwrap().loads.push(Load {
            path: path.clone(),
            kind: kind.into(),
        });
        file_imports::powershell_import(node, &path, kind, out);
    } else {
        file_imports::powershell_import(node, "<unresolved-loader>", kind, out);
        out.powershell.as_mut().unwrap().dynamic = true;
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
    if kind == "using_statement" {
        let raw = text(node, source);
        let rest = raw.get(5..).unwrap_or("").trim_start();
        let split = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let target = &rest[..split];
        let value = rest[split..].trim();
        if target.eq_ignore_ascii_case("module") {
            load(node, value, "powershell_using_module_candidate", true, out);
        } else {
            file_imports::powershell_import(
                node,
                "<unresolved-namespace-or-assembly>",
                "powershell_using_directive_unverified",
                out,
            );
        }
        return;
    }
    if kind == "function_statement" {
        let Some(n) = child(node, "function_name") else {
            return;
        };
        let spelling = text(n, source);
        let name = ident(spelling);
        let id = symbol(doc, node, spelling, spelling.into(), source, out);
        if name.is_none() {
            out.powershell.as_mut().unwrap().dynamic = true;
        }
        out.powershell.as_mut().unwrap().functions.push(Function {
            id: id.clone(),
            name: name
                .clone()
                .unwrap_or_else(|| spelling.to_ascii_lowercase()),
            scope_start: ctx.scope_start,
            scope_end: ctx.scope_end,
            reliable: ctx.reliable && name.is_some(),
            root: ctx.root,
        });
        if let Some(body) = child(node, "script_block") {
            walk(
                body,
                source,
                doc,
                &Context {
                    caller: Some(id),
                    scope_start: body.start_byte(),
                    scope_end: body.end_byte(),
                    root: false,
                    ..ctx.clone()
                },
                out,
                depth + 1,
            );
        }
        return;
    }
    if kind == "class_statement" || kind == "enum_statement" {
        let Some(n) = child(node, "simple_name") else {
            return;
        };
        let spelling = text(n, source);
        let id = symbol(doc, node, spelling, spelling.into(), source, out);
        if kind == "class_statement" {
            let bases = children(node)
                .into_iter()
                .filter(|n| n.kind() == "type_spec")
                .map(|n| text(n, source).to_ascii_lowercase())
                .collect();
            out.powershell.as_mut().unwrap().classes.push(Class {
                id,
                name: spelling.to_ascii_lowercase(),
                bases,
                line: node.start_position().row + 1,
            });
            let nested = Context {
                owner: spelling.to_ascii_lowercase(),
                caller: None,
                ..ctx.clone()
            };
            for child in children(node).into_iter().filter(|n| {
                matches!(
                    n.kind(),
                    "class_method_definition" | "class_property_definition"
                )
            }) {
                walk(child, source, doc, &nested, out, depth + 1);
            }
        }
        return;
    }
    if kind == "class_method_definition" {
        let Some(n) = child(node, "simple_name") else {
            return;
        };
        let spelling = text(n, source);
        let id = symbol(
            doc,
            node,
            spelling,
            format!("{}::{spelling}", ctx.owner),
            source,
            out,
        );
        let statics = children(node).iter().any(|n| {
            n.kind() == "class_attribute" && text(*n, source).eq_ignore_ascii_case("static")
        });
        let arity = child(node, "class_method_parameter_list").map_or(0, |n| children(n).len());
        out.powershell.as_mut().unwrap().methods.push(Method {
            id: id.clone(),
            owner: ctx.owner.clone(),
            name: spelling.to_ascii_lowercase(),
            statics,
            arity,
        });
        if let Some(body) = child(node, "script_block") {
            walk(
                body,
                source,
                doc,
                &Context {
                    caller: Some(id),
                    statics,
                    scope_start: body.start_byte(),
                    scope_end: body.end_byte(),
                    root: false,
                    ..ctx.clone()
                },
                out,
                depth + 1,
            );
        }
        return;
    }
    if kind == "class_property_definition" {
        if let Some(n) = child(node, "variable") {
            let spelling = text(n, source);
            symbol(
                doc,
                node,
                spelling,
                format!("{}::{spelling}", ctx.owner),
                source,
                out,
            );
            out.symbols.last_mut().unwrap().signature = spelling.into();
        }
        return;
    }
    if kind == "command" {
        let Some(target) = node.child_by_field_name("command_name") else {
            return;
        };
        let raw = text(target, source);
        let op = child(node, "command_invocation_operator").map(|n| text(n, source));
        if op == Some(".") {
            if !ctx.root {
                out.powershell.as_mut().unwrap().dynamic = true;
            }
            load(node, raw, "powershell_dot_source_candidate", false, out);
        }
        let simple = unquote(raw).and_then(|s| ident(&s));
        if simple.as_deref().is_some_and(|s| {
            matches!(
                s,
                "set-alias"
                    | "new-alias"
                    | "sal"
                    | "nal"
                    | "import-module"
                    | "ipmo"
                    | "invoke-expression"
                    | "iex"
                    | "set-item"
                    | "new-item"
                    | "remove-item"
                    | "export-modulemember"
            )
        }) {
            out.powershell.as_mut().unwrap().dynamic = true;
        }
        let target_kind = if op == Some(".") {
            Target::Unknown
        } else {
            simple.clone().map_or(Target::Unknown, Target::Command)
        };
        let expression = simple
            .clone()
            .unwrap_or_else(|| "<dynamic-command-or-path>".into());
        reference(
            doc,
            target,
            &simple.unwrap_or_else(|| "<dynamic-command>".into()),
            expression,
            target_kind,
            ctx,
            out,
        );
        for child in children(node) {
            if child.id() != target.id() {
                walk(child, source, doc, ctx, out, depth + 1);
            } else if target.kind() == "command_name_expr" {
                walk(
                    child,
                    source,
                    doc,
                    &Context {
                        reliable: false,
                        ..ctx.clone()
                    },
                    out,
                    depth + 1,
                );
            }
        }
        return;
    }
    if kind == "invocation_expression" {
        let nodes = children(node);
        let member = nodes.iter().find(|n| n.kind() == "member_name").copied();
        let receiver = nodes.first().copied();
        if let Some(member) = member {
            let name = ident(text(member, source));
            let statics =
                receiver.is_some_and(|r| source[r.end_byte()..member.start_byte()].contains("::"));
            let owner = receiver.and_then(|r| {
                if r.kind() == "type_literal" && statics {
                    ident(
                        text(r, source)
                            .trim_start_matches('[')
                            .trim_end_matches(']'),
                    )
                } else if text(r, source).eq_ignore_ascii_case("$this") && !ctx.statics && !statics
                {
                    Some(ctx.owner.clone())
                } else {
                    None
                }
            });
            let argc = child(node, "argument_list")
                .and_then(|n| n.child_by_field_name("argument_expression_list"))
                .map_or(0, |n| children(n).len());
            let spelling = name.clone().unwrap_or_else(|| "<dynamic-member>".into());
            let expression = format!(
                "{}{}{}",
                owner.as_deref().unwrap_or("<receiver>"),
                if statics { "::" } else { "." },
                spelling
            );
            reference(
                doc,
                member,
                &spelling,
                expression,
                Target::Member {
                    owner,
                    name: spelling.clone(),
                    statics,
                    argc,
                },
                ctx,
                out,
            );
            for child in nodes {
                if child.id() != member.id() {
                    walk(child, source, doc, ctx, out, depth + 1);
                }
            }
        }
        return;
    }
    // Deferred script blocks and conditional function definitions are inventoried
    // without treating their execution/scope as established.
    let mut nested = ctx.clone();
    if kind == "script_block_expression"
        || kind.ends_with("statement")
            && matches!(
                kind,
                "if_statement"
                    | "switch_statement"
                    | "try_statement"
                    | "foreach_statement"
                    | "for_statement"
                    | "while_statement"
            )
    {
        nested.reliable = false;
    }
    if kind == "assignment_expression"
        && text(node, source)
            .to_ascii_lowercase()
            .contains("function:")
    {
        out.powershell.as_mut().unwrap().dynamic = true;
    }
    for child in children(node) {
        walk(child, source, doc, &nested, out, depth + 1);
    }
}

// Upstream accepts only single-quoted literal using-module paths. Convert only
// pure double-quoted header literals, preserving every byte and original anchors.
fn normalize_using_quotes(source: &str) -> Option<String> {
    let mut bytes = source.as_bytes().to_vec();
    let mut offset = 0;
    let mut changed = false;
    let mut block_comment = false;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        if block_comment {
            if let Some(end) = trimmed.find("#>") {
                block_comment = false;
                if !trimmed[end + 2..].trim().is_empty() {
                    break;
                }
            }
            offset += line.len();
            continue;
        }
        if trimmed.starts_with("<#") {
            if let Some(end) = trimmed.find("#>") {
                if !trimmed[end + 2..].trim().is_empty() {
                    break;
                }
            } else {
                block_comment = true;
            }
            offset += line.len();
            continue;
        }
        if trimmed.trim().is_empty() || trimmed.starts_with('#') {
            offset += line.len();
            continue;
        }
        if !lower.starts_with("using")
            || !trimmed
                .get(5..)
                .and_then(|s| s.chars().next())
                .is_some_and(char::is_whitespace)
        {
            break;
        }
        let rest = trimmed[5..].trim_start();
        let split = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let directive = &rest[..split];
        let value = rest[split..].trim_start();
        if directive.eq_ignore_ascii_case("module") && value.starts_with('"') {
            if let Some(end) = value[1..].find('"').map(|i| i + 1) {
                let content = &value[1..end];
                let tail = value[end + 1..].trim();
                if !content
                    .chars()
                    .any(|c| matches!(c, '$' | '\'' | '\r' | '\n') || c as u32 == 96)
                    && (tail.is_empty() || tail.starts_with('#'))
                {
                    let start = offset + line.len() - value.len();
                    bytes[start] = b'\'';
                    bytes[start + end] = b'\'';
                    changed = true;
                }
            }
        }
        offset += line.len();
    }
    changed.then(|| String::from_utf8(bytes).expect("ASCII quote replacement preserves UTF-8"))
}
pub(super) fn parse_source(doc: &str, source: &str, out: &mut IndexedDocument) -> Result<()> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_pwsh::LANGUAGE.into())
        .map_err(anyhow::Error::from)?;
    let mut tree = parser
        .parse(source, None)
        .context("PowerShell parser returned no tree")?;
    if tree.root_node().has_error() {
        if let Some(normalized) = normalize_using_quotes(source) {
            tree = parser
                .parse(&normalized, None)
                .context("PowerShell normalized parser returned no tree")?;
            out.binding_limitations
                .push("POWERSHELL_LITERAL_USING_QUOTES_NORMALIZED".into());
        }
    }
    out.parse_errors = tree.root_node().has_error();
    out.powershell = Some(Facts::default());
    out.binding_limitations
        .push("POWERSHELL_SESSION_ALIAS_SCOPE_EXPORT_AND_RUNTIME_UNVERIFIED".into());
    walk(
        tree.root_node(),
        source,
        doc,
        &Context {
            caller: None,
            owner: String::new(),
            statics: false,
            scope_start: 0,
            scope_end: source.len(),
            reliable: true,
            root: true,
        },
        out,
        0,
    );
    Ok(())
}

struct Resolver<'a> {
    doc: &'a IndexedDocument,
    types: Vec<&'a IndexedDocument>,
    commands: Vec<&'a IndexedDocument>,
    complete: bool,
}
fn closure<'a>(
    root: &'a IndexedDocument,
    docs: &'a HashMap<String, IndexedDocument>,
    commands: bool,
) -> (Vec<&'a IndexedDocument>, bool) {
    let mut found = vec![];
    let mut pending = vec![root];
    let mut seen = BTreeSet::new();
    let mut complete = true;
    while let Some(doc) = pending.pop() {
        if found.len() >= 4096 {
            complete = false;
            break;
        }
        if !seen.insert((doc.path.clone(), doc.hash.clone())) {
            continue;
        }
        found.push(doc);
        for load in &doc.powershell.as_ref().unwrap().loads {
            if commands && load.kind != "powershell_dot_source_candidate" {
                continue;
            }
            let matches: Vec<_> = docs
                .values()
                .filter(|d| {
                    d.project == root.project
                        && d.path == load.path
                        && d.powershell.is_some()
                        && !d.parse_errors
                })
                .collect();
            if matches.len() == 1 {
                pending.push(matches[0]);
            } else {
                complete = false;
            }
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    (found, complete)
}
impl<'a> Resolver<'a> {
    fn new(doc: &'a IndexedDocument, docs: &'a HashMap<String, IndexedDocument>) -> Self {
        let (types, a) = closure(doc, docs, false);
        let (commands, b) = closure(doc, docs, true);
        Self {
            doc,
            types,
            commands,
            complete: a && b,
        }
    }
    fn class(&self, name: &str) -> Vec<(&'a IndexedDocument, &'a Class)> {
        self.types
            .iter()
            .flat_map(|d| {
                d.powershell
                    .as_ref()
                    .unwrap()
                    .classes
                    .iter()
                    .filter(move |c| c.name == name)
                    .map(move |c| (*d, c))
            })
            .collect()
    }
    fn methods(
        &self,
        owner: &str,
        name: &str,
        statics: bool,
        argc: usize,
        depth: usize,
    ) -> Vec<String> {
        if depth >= 32 {
            return vec![];
        }
        let classes = self.class(owner);
        if classes.len() != 1 {
            return vec![];
        }
        let (doc, class) = classes[0];
        let constructor = statics && name == "new";
        let name = if constructor { owner } else { name };
        let named: Vec<_> = doc
            .powershell
            .as_ref()
            .unwrap()
            .methods
            .iter()
            .filter(|m| m.owner == owner && m.name == name)
            .collect();
        if !named.is_empty() {
            return named
                .into_iter()
                .filter(|m| {
                    m.statics == if constructor { false } else { statics } && m.arity == argc
                })
                .map(|m| m.id.clone())
                .collect();
        }
        if constructor || class.bases.len() != 1 {
            return vec![];
        }
        self.methods(&class.bases[0], name, statics, argc, depth + 1)
    }
    fn candidates(&self, r: &Reference, site: &Site) -> Vec<String> {
        if !site.reliable
            || !self.complete
            || self.doc.parse_errors
            || self
                .types
                .iter()
                .any(|d| d.powershell.as_ref().unwrap().dynamic)
        {
            return vec![];
        }
        match &site.target {
            Target::Command(name) => {
                let mut local: Vec<_> = self
                    .doc
                    .powershell
                    .as_ref()
                    .unwrap()
                    .functions
                    .iter()
                    .filter(|f| {
                        f.name == *name
                            && f.scope_start <= r.start_byte
                            && r.start_byte < f.scope_end
                    })
                    .collect();
                local.sort_by_key(|f| (f.scope_start, usize::MAX - f.scope_end));
                if let Some(nearest) = local.last() {
                    let key = (nearest.scope_start, nearest.scope_end);
                    let nearest_root = nearest.root;
                    let local: Vec<_> = local
                        .into_iter()
                        .filter(|f| (f.scope_start, f.scope_end) == key)
                        .collect();
                    if local.iter().any(|f| !f.reliable) {
                        return vec![];
                    }
                    let mut result: Vec<_> = local.into_iter().map(|f| f.id.clone()).collect();
                    if nearest_root {
                        for imported in self
                            .commands
                            .iter()
                            .filter(|d| !std::ptr::eq(**d, self.doc))
                            .flat_map(|d| {
                                d.powershell
                                    .as_ref()
                                    .unwrap()
                                    .functions
                                    .iter()
                                    .filter(|f| f.root && f.name == *name)
                            })
                        {
                            if !imported.reliable {
                                return vec![];
                            }
                            result.push(imported.id.clone());
                        }
                    }
                    return result;
                }
                let imported: Vec<_> = self
                    .commands
                    .iter()
                    .filter(|d| !std::ptr::eq(**d, self.doc))
                    .flat_map(|d| {
                        d.powershell
                            .as_ref()
                            .unwrap()
                            .functions
                            .iter()
                            .filter(|f| f.root && f.name == *name)
                    })
                    .collect();
                if imported.iter().any(|f| !f.reliable) {
                    return vec![];
                }
                imported.into_iter().map(|f| f.id.clone()).collect()
            }
            Target::Member {
                owner: Some(owner),
                name,
                statics,
                argc,
            } => self.methods(owner, name, *statics, *argc, 0),
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
        let mut candidates=doc.powershell.as_ref().unwrap().sites.get(&r.start_byte).map_or_else(Vec::new,|s|resolver.candidates(r,s));candidates.sort();candidates.dedup();
        CallEdge {reference:r.clone(),resolution:match candidates.len(){0=>"unresolved",1=>"resolved_static",_=>"ambiguous"}.into(),candidates,provenance:"derived",resolution_basis:"powershell_captured_scope_literal_load_type_arity_candidates;session_alias_runtime_unverified".into()}
    }).collect()
}
pub(super) fn edges(
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Vec<inheritance::InheritanceEdge> {
    let resolver = Resolver::new(doc, docs);
    let mut result = vec![];
    for class in &doc.powershell.as_ref().unwrap().classes {
        for base in &class.bases {
            let mut candidates: Vec<_> = if resolver.complete && !doc.parse_errors {
                resolver
                    .class(base)
                    .into_iter()
                    .map(|(_, c)| c.id.clone())
                    .collect()
            } else {
                vec![]
            };
            candidates.sort();
            candidates.dedup();
            result.push(inheritance::InheritanceEdge {
                derived: class.id.clone(),
                resolution: match candidates.len() {
                    0 => "unresolved",
                    1 => "resolved_static",
                    _ => "ambiguous",
                }
                .into(),
                candidates,
                base_expression: base.clone(),
                path: doc.path.clone(),
                line: class.line,
                relation: "powershell_base_type_candidate".into(),
                hierarchy_status: "partial_runspace_type_runtime_unverified".into(),
                provenance: "derived",
            });
        }
    }
    result
}
pub(super) fn entrypoint(doc: &IndexedDocument, symbol: &Symbol) -> Option<&'static str> {
    if symbol.kind == "class_method_definition" {
        let method = doc
            .powershell
            .as_ref()?
            .methods
            .iter()
            .find(|m| m.id == symbol.id)?;
        if method.name == method.owner {
            return Some("powershell_constructor_candidate");
        }
    }
    if doc.path.ends_with(".psm1") && symbol.kind == "function_statement" {
        Some("powershell_module_function_candidate")
    } else if symbol.name.eq_ignore_ascii_case("main")
        || symbol.name.eq_ignore_ascii_case("invoke-main")
    {
        Some("powershell_conventional_main_candidate")
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
