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
mod tests {
    use super::*;
    struct Snapshot {
        symbols: Vec<Symbol>,
        refs: Vec<CallEdge>,
        docs: HashMap<String, IndexedDocument>,
    }
    fn snapshot(files: &[(&str, &str, &str)]) -> Snapshot {
        let docs: HashMap<_, _> = files
            .iter()
            .enumerate()
            .map(|(i, (path, project, source))| {
                let id = format!("d{i}");
                let mut doc = parse(&id, path, source, 1).unwrap();
                doc.project = Some((*project).into());
                assert!(!doc.parse_errors, "{path}: {source}");
                (id, doc)
            })
            .collect();
        let symbols = docs
            .values()
            .flat_map(|d| d.symbols.clone())
            .collect::<Vec<_>>();
        let refs = resolve::references(&symbols, &docs);
        Snapshot {
            symbols,
            refs,
            docs,
        }
    }
    fn targets(s: &Snapshot, caller: &str, expression: &str) -> Vec<String> {
        let ids: BTreeSet<_> = s
            .symbols
            .iter()
            .filter(|s| s.qualified_name == caller)
            .map(|s| s.id.as_str())
            .collect();
        let edges: Vec<_> = s
            .refs
            .iter()
            .filter(|e| {
                e.reference
                    .caller
                    .as_deref()
                    .is_some_and(|id| ids.contains(id))
                    && e.reference.expression == expression
            })
            .collect();
        assert_eq!(edges.len(), 1, "{caller} {expression}: {:?}", s.refs);
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
    fn powershell_functions_are_case_insensitive_and_nested_scope_is_bounded() {
        let s=snapshot(&[("/p/a.ps1","p","function Get-Work { 1 }\nfunction Invoke-Main { get-work; function Get-Inner { 2 }; GET-INNER }\nfunction Outside { Get-Inner }")]);
        assert_eq!(targets(&s, "Invoke-Main", "get-work"), ["Get-Work"]);
        assert_eq!(targets(&s, "Invoke-Main", "get-inner"), ["Get-Inner"]);
        assert!(targets(&s, "Outside", "get-inner").is_empty());
    }
    #[test]
    fn powershell_dot_source_literal_root_links_functions_and_project_guards() {
        let s = snapshot(&[
            ("/p/lib.ps1", "p", "function Get-Work {1}"),
            (
                "/p/main.ps1",
                "p",
                ". \"$PSScriptRoot/lib.ps1\"\nfunction Invoke-Main {Get-Work}",
            ),
            ("/foreign/lib.ps1", "other", "function Get-Work {2}"),
        ]);
        assert_eq!(targets(&s, "Invoke-Main", "get-work"), ["Get-Work"]);
        let edges = file_imports::edges(&s.docs);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].candidates.len(), 1);
        assert!(!edges[0].runtime_import_proven);
        let s = snapshot(&[
            ("/p/lib.ps1", "p", "function Get-Work {1}"),
            (
                "/p/main.ps1",
                "p",
                ". ./lib.ps1\nfunction Invoke-Main {Get-Work}",
            ),
        ]);
        assert!(targets(&s, "Invoke-Main", "get-work").is_empty());
    }
    #[test]
    fn powershell_class_static_instance_constructor_and_base_candidates() {
        let s=snapshot(&[("/p/a.ps1","p","class Base { [int] Work(){return 1} static [int] Api(){return 2} }\nclass Child : Base { Child(){} [int] Run(){return $this.Work()} static [int] Call(){return [Base]::Api()} }\nfunction Invoke-Main {[Child]::new()}")]);
        assert_eq!(targets(&s, "child::Run", "child.work"), ["base::Work"]);
        assert_eq!(targets(&s, "child::Call", "base::api"), ["base::Api"]);
        assert_eq!(targets(&s, "Invoke-Main", "child::new"), ["child::Child"]);
        let bases = edges(&s.docs, s.docs.values().next().unwrap());
        assert_eq!(bases.len(), 1);
        assert_eq!(bases[0].candidates.len(), 1);
        assert_ne!(bases[0].hierarchy_status, "complete_static");
    }
    #[test]
    fn powershell_using_module_types_do_not_invent_function_exports() {
        let s = snapshot(&[
            (
                "/p/types.psm1",
                "p",
                "class Worker { static [int] Run(){return 1} }\nfunction Get-Work {1}",
            ),
            (
                "/p/main.ps1",
                "p",
                "using module './types.psm1'\nfunction Invoke-Main {[Worker]::Run();Get-Work}",
            ),
        ]);
        assert_eq!(targets(&s, "Invoke-Main", "worker::run"), ["worker::Run"]);
        assert!(targets(&s, "Invoke-Main", "get-work").is_empty());
    }
    #[test]
    fn powershell_alias_dynamic_loader_scoped_and_conditional_definitions_fail_closed() {
        for source in [
            "function Get-Work {1}\nSet-Alias Get-Work Other\nfunction Invoke-Main {Get-Work}",
            "function Get-Work {1}\nImport-Module Whatever\nfunction Invoke-Main {Get-Work}",
            "function script:Get-Work {1}\nfunction Invoke-Main {Get-Work}",
            "if($true){function Get-Work {1}}\nfunction Invoke-Main {Get-Work}",
        ] {
            let s = snapshot(&[("/p/a.ps1", "p", source)]);
            assert!(
                targets(&s, "Invoke-Main", "get-work").is_empty(),
                "{source}"
            );
        }
    }
    #[test]
    fn powershell_overloads_preserve_ambiguity_and_unknown_receivers_stay_unbound() {
        let s=snapshot(&[("/p/a.ps1","p","class Worker { static [int] Run([int]$n){return 1} static [int] Run([string]$n){return 2} [int] Work(){return 3} }\nfunction Invoke-Main {[Worker]::Run(1);$unknown.Work();[Worker].Work()}")]);
        assert_eq!(targets(&s, "Invoke-Main", "worker::run").len(), 2);
        assert!(s
            .refs
            .iter()
            .filter(|e| e.reference.name == "work")
            .all(|e| e.candidates.is_empty()));
    }
    #[test]
    fn powershell_deferred_blocks_keep_occurrences_without_assuming_execution_scope() {
        let s = snapshot(&[(
            "/p/a.ps1",
            "p",
            "function Get-Work {1}\nfunction Invoke-Main { & { Get-Work }; $f={Get-Work}; & $f }",
        )]);
        let uses: Vec<_> = s
            .refs
            .iter()
            .filter(|e| e.reference.name == "get-work")
            .collect();
        assert_eq!(uses.len(), 2);
        assert!(uses.iter().all(|e| e.candidates.is_empty()));
    }
    #[test]
    fn powershell_enum_properties_and_body_spans_are_source_bound() {
        let source="# café\nenum Color { Red; Blue }\nclass Worker { [string] $Value='PRIVATE_INITIALIZER'; [int] Work(){ return 1 } }";
        let s = snapshot(&[("/p/a.ps1", "p", source)]);
        assert!(s.symbols.iter().any(|s| s.kind == "enum_statement"));
        let property = s
            .symbols
            .iter()
            .find(|s| s.kind == "class_property_definition")
            .unwrap();
        assert!(!property.signature.contains("PRIVATE_INITIALIZER"));
        let method = s
            .symbols
            .iter()
            .find(|s| s.kind == "class_method_definition")
            .unwrap();
        assert_eq!(
            &source[method.start_byte..method.end_byte],
            "[int] Work(){ return 1 }"
        );
    }
    #[test]
    fn powershell_load_collisions_and_nested_loads_remain_uncertain() {
        let s=snapshot(&[("/p/lib.ps1","p","function Get-Work {2}"),("/p/main.ps1","p","function Get-Work {1}\n. \"$PSScriptRoot/lib.ps1\"\nfunction Invoke-Main {Get-Work}")]);
        assert_eq!(targets(&s, "Invoke-Main", "get-work").len(), 2);
        let s = snapshot(&[
            ("/p/lib.ps1", "p", "function Get-Work {2}"),
            (
                "/p/main.ps1",
                "p",
                "function Loader {. \"$PSScriptRoot/lib.ps1\"}\nfunction Invoke-Main {Get-Work}",
            ),
        ]);
        assert!(targets(&s, "Invoke-Main", "get-work").is_empty());
    }
    #[test]
    fn powershell_body_comments_are_excluded_from_metadata() {
        let s=snapshot(&[("/p/a.ps1","p","# café\nfunction Invoke-Main { # PRIVATE_BODY\n 1 }\nclass C {[int] Run(){ # PRIVATE_BODY\n return 1 }}")]);
        assert!(!serde_json::to_string(&s.symbols)
            .unwrap()
            .contains("PRIVATE_BODY"));
    }
    #[test]
    fn powershell_store_body_cache_stale_sync_and_legacy_history() {
        use crate::models::Document;
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("powershell.duckdb")).unwrap();
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
            "/p/base.psm1",
            "# café\nclass Base {[int] Work(){ # PRIVATE_BODY\n return 1 }}",
        );
        let mut client = doc(
            "client",
            "/p/client.ps1",
            "using module './base.psm1'\nclass Child : Base {[int] Run(){return $this.Work()}}",
        );
        store.upsert_document(&base).unwrap();
        store.upsert_document(&client).unwrap();
        assert_eq!(store.index_code().unwrap().indexed, 2);
        let q = CodeQuery {
            symbol: Some("base::Work".into()),
            ..Default::default()
        };
        let before = store.code_snapshot().unwrap().query("callers", &q);
        assert_eq!(before["items"].as_array().unwrap().len(), 1, "{before}");
        assert!(store
            .code_body(&q)
            .unwrap()
            .to_string()
            .contains("PRIVATE_BODY"));
        assert!(!before.to_string().contains("PRIVATE_BODY"));
        let forced = store
            .index_code_with_options(&resolution_cache::IndexOptions {
                force_full_resolution: true,
            })
            .unwrap();
        assert!(!forced.revision_created);
        assert_eq!(
            forced.resolution.unwrap().equivalent_to_existing,
            Some(true)
        );
        client.content = "class Child {[int] Run(){return 1}}".into();
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
        assert_eq!(diff["powershell_relations_available"], true);
        assert!(
            diff["relations_removed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["relation"] == "powershell_base_type_candidate"),
            "{diff}"
        );
        assert!(diff["relations_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["import_kind"] == "powershell_using_module_candidate"));
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
            .remove("powershell_syntax_format");
        conn.execute(
            "UPDATE code_index_revisions SET graph_json=? WHERE id=1",
            [graph.to_string()],
        )
        .unwrap();
        drop(conn);
        let legacy = store.code_index_diff(&query).unwrap();
        assert_eq!(legacy["powershell_relations_available"], false);
        assert!(!legacy["relations_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["relation"] == "powershell_base_type_candidate"
                || e["import_kind"] == "powershell_using_module_candidate"));
    }

    #[test]
    fn powershell_literal_using_double_quotes_preserve_original_byte_anchors() {
        let source="# café\nusing module \"./child.psm1\"\n. \"$PSScriptRoot/functions.ps1\"\nfunction Invoke-Main {GET-WORK; [Child]::Call(); & $unknown}\nInvoke-Main\n";
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_pwsh::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        assert!(tree.root_node().has_error());
        let normalized = normalize_using_quotes(source).unwrap();
        assert_eq!(normalized.len(), source.len());
        assert!(normalized.contains("using module './child.psm1'"));
        let doc = parse("main", "/p/main.ps1", source, 1).unwrap();
        assert!(!doc.parse_errors);
        assert!(doc
            .binding_limitations
            .iter()
            .any(|v| v == "POWERSHELL_LITERAL_USING_QUOTES_NORMALIZED"));
        let symbol = doc
            .symbols
            .iter()
            .find(|s| s.name == "Invoke-Main")
            .unwrap();
        assert_eq!(
            &source[symbol.start_byte..symbol.end_byte],
            "function Invoke-Main {GET-WORK; [Child]::Call(); & $unknown}"
        );
    }
    #[test]
    fn powershell_using_quote_adapter_rejects_interpolation_escapes_and_non_header_text() {
        for source in [
            "using module \"$root/file.psm1\"\n",
            "function Main { 'using module \"./x.psm1\"' }\n",
            "<#\nusing module \"./x.psm1\"\n#>\n",
            "using module \"./a'b.psm1\"\n",
            "using module \"./x.psm1\" unexpected\n",
        ] {
            assert!(normalize_using_quotes(source).is_none(), "{source}");
        }
        let escaped = format!("using module \"./a{}b.psm1\"\n", char::from(96));
        assert!(normalize_using_quotes(&escaped).is_none());
        let source="# café\r\n<# comment #>\r\n  USING   MODULE   \"./types.psm1\" # trailing\r\nfunction Main {}";
        let normalized = normalize_using_quotes(source).unwrap();
        assert_eq!(normalized.len(), source.len());
        assert_eq!(normalized.lines().count(), source.lines().count());
        let doc = parse("main", "/p/main.ps1", source, 1).unwrap();
        assert!(!doc.parse_errors);
    }
}
