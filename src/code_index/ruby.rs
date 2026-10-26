//! Ruby syntax and captured-owner candidates; never VM dispatch or load-path proof.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Facts {
    methods: Vec<Method>,
    classes: Vec<Class>,
    sites: BTreeMap<usize, Site>,
    locals: Vec<Local>,
    requires: Vec<String>,
    invalid_owners: BTreeSet<String>,
    constant_writes: BTreeSet<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Method {
    id: String,
    owner: String,
    name: String,
    singleton: bool,
    reliable: bool,
    object: Option<String>,
    lexical: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Class {
    id: String,
    name: String,
    kind: String,
    base: Option<String>,
    line: usize,
    lexical: Vec<String>,
    reliable: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Local {
    name: String,
    start: usize,
    end: usize,
    visible: usize,
    constructor: Option<String>,
    lexical: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Receiver {
    Implicit,
    SelfValue,
    Constant(String),
    Local(String),
    Constructed(String),
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Site {
    name: String,
    owner: String,
    singleton: bool,
    lexical: Vec<String>,
    receiver: Receiver,
    bare: bool,
    type_use: bool,
    local_value: bool,
    reliable: bool,
}
#[derive(Clone)]
struct Context {
    owner: String,
    singleton: bool,
    lexical: Vec<String>,
    caller: Option<String>,
    start: usize,
    end: usize,
    reliable: bool,
    controlled: bool,
}
fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut c = node.walk();
    node.named_children(&mut c).collect()
}
fn constant(node: Node<'_>, source: &str) -> Option<String> {
    if !matches!(node.kind(), "constant" | "scope_resolution") {
        return None;
    }
    let raw = text(node, source);
    let trimmed = raw.trim_start_matches("::");
    (!trimmed.is_empty()
        && trimmed
            .split("::")
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_alphanumeric() || c == '_')))
    .then(|| raw.into())
}
fn string(node: Node<'_>, source: &str) -> Option<String> {
    if node.kind() != "string"
        || children(node)
            .iter()
            .any(|n| matches!(n.kind(), "interpolation" | "escape_sequence"))
    {
        return None;
    }
    let raw = text(node, source);
    let quote = *raw.as_bytes().first()?;
    if !matches!(quote, b'\'' | b'"') || raw.as_bytes().last() != Some(&quote) {
        return None;
    }
    Some(raw.get(1..raw.len().checked_sub(1)?)?.into())
}
fn constructor(node: Node<'_>, source: &str) -> Option<String> {
    if node.kind() != "call"
        || node
            .child_by_field_name("method")
            .is_none_or(|n| text(n, source) != "new")
    {
        return None;
    }
    constant(node.child_by_field_name("receiver")?, source)
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
    let signature = if let Some(body) = node.child_by_field_name("body") {
        source[node.start_byte()..body.start_byte()].trim()
    } else {
        text(node, source).lines().next().unwrap_or("")
    };
    out.symbols.push(Symbol {
        build_context: None,
        documentation: None,
        documentation_examined: false,
        parameter_contracts: None,
        id: id.clone(),
        document_id: doc.into(),
        path: out.path.clone(),
        name: name.into(),
        qualified_name: qualified,
        kind: node.kind().into(),
        signature: signature.chars().take(512).collect(),
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        revision: out.revision,
        content_hash: out.hash.clone(),
    });
    id
}
struct ReferenceKind {
    receiver: Receiver,
    bare: bool,
    type_use: bool,
}
fn reference(
    doc: &str,
    node: Node<'_>,
    name: &str,
    expression: String,
    ctx: &Context,
    kind: ReferenceKind,
    out: &mut IndexedDocument,
) {
    let ReferenceKind {
        receiver,
        bare,
        type_use,
    } = kind;
    let pos = node.start_byte();
    out.references.push(Reference {
        build_context: None,
        document_id: doc.into(),
        path: out.path.clone(),
        name: name.into(),
        expression,
        start_byte: pos,
        line: node.start_position().row + 1,
        column: node.start_position().column + 1,
        caller: ctx.caller.clone(),
        is_call: !type_use,
        revision: out.revision,
        content_hash: out.hash.clone(),
    });
    out.ruby.as_mut().unwrap().sites.insert(
        pos,
        Site {
            name: name.into(),
            owner: ctx.owner.clone(),
            singleton: ctx.singleton,
            lexical: ctx.lexical.clone(),
            receiver,
            bare,
            type_use,
            local_value: false,
            reliable: ctx.reliable,
        },
    );
}
fn parameters(
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
    for p in children(node) {
        if p.kind() == "identifier" {
            out.ruby.as_mut().unwrap().locals.push(Local {
                name: text(p, source).into(),
                start: ctx.start,
                end: ctx.end,
                visible: ctx.start,
                constructor: None,
                lexical: ctx.lexical.clone(),
            });
        } else if let Some(name) = p.child_by_field_name("name") {
            out.ruby.as_mut().unwrap().locals.push(Local {
                name: text(name, source).into(),
                start: ctx.start,
                end: ctx.end,
                visible: ctx.start,
                constructor: None,
                lexical: ctx.lexical.clone(),
            });
            if let Some(value) = p.child_by_field_name("value") {
                walk(value, source, doc, ctx, out, depth + 1);
            }
        } else {
            parameters(p, source, doc, ctx, out, depth + 1);
        }
    }
}
fn local_pattern(
    node: Node<'_>,
    source: &str,
    ctx: &Context,
    visible: usize,
    out: &mut IndexedDocument,
    depth: usize,
    assignment: bool,
) {
    if depth >= 256 {
        out.parse_errors = true;
        return;
    }
    if node.kind() == "identifier" {
        out.ruby.as_mut().unwrap().locals.push(Local {
            name: text(node, source).into(),
            start: ctx.start,
            end: ctx.end,
            visible,
            constructor: None,
            lexical: ctx.lexical.clone(),
        });
    } else if (node.kind() == "constant" || node.kind() == "scope_resolution") && assignment {
        out.ruby
            .as_mut()
            .unwrap()
            .constant_writes
            .insert(text(node, source).into());
    } else {
        for child in children(node) {
            local_pattern(child, source, ctx, visible, out, depth + 1, assignment);
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
    if matches!(node.kind(), "class" | "module") {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let raw = text(name, source);
        let reliable = ctx.reliable
            && constant(name, source).is_some()
            && (ctx.owner.is_empty() || !raw.contains("::") || raw.starts_with("::"));
        let qualified = if raw.starts_with("::") {
            raw.trim_start_matches("::").into()
        } else if ctx.owner.is_empty() {
            raw.into()
        } else {
            format!("{}::{raw}", ctx.owner)
        };
        let id = symbol(
            doc,
            node,
            raw.trim_start_matches("::")
                .rsplit("::")
                .next()
                .unwrap_or(raw),
            qualified.clone(),
            source,
            out,
        );
        let superclass = node
            .child_by_field_name("superclass")
            .and_then(|n| children(n).into_iter().next());
        let base = superclass.map(|n| text(n, source).to_string());
        out.ruby.as_mut().unwrap().classes.push(Class {
            id: id.clone(),
            name: qualified.clone(),
            kind: node.kind().into(),
            base,
            line: node.start_position().row + 1,
            lexical: ctx.lexical.clone(),
            reliable,
        });
        if let Some(base) = superclass {
            reference(
                doc,
                base,
                text(base, source),
                text(base, source).into(),
                ctx,
                ReferenceKind {
                    receiver: Receiver::Unknown,
                    bare: false,
                    type_use: true,
                },
                out,
            );
        }
        let mut nested = ctx.clone();
        nested.owner = qualified.clone();
        nested.lexical.push(qualified);
        nested.singleton = true;
        nested.caller = None;
        nested.start = node.start_byte();
        nested.end = node.end_byte();
        nested.reliable = reliable;
        if let Some(body) = node.child_by_field_name("body") {
            walk(body, source, doc, &nested, out, depth + 1);
        }
        return;
    }
    if node.kind() == "singleton_class" {
        let mut nested = ctx.clone();
        nested.singleton = true;
        nested.start = node.start_byte();
        nested.end = node.end_byte();
        if node
            .child_by_field_name("value")
            .is_none_or(|n| text(n, source) != "self")
        {
            nested.reliable = false;
        }
        if let Some(body) = node.child_by_field_name("body") {
            walk(body, source, doc, &nested, out, depth + 1);
        }
        return;
    }
    if matches!(node.kind(), "method" | "singleton_method") {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let name = text(name, source);
        let explicit = node.kind() == "singleton_method";
        let singleton = explicit
            || ctx.singleton
                && node.parent().is_some_and(|_| {
                    let mut n = node.parent();
                    while let Some(p) = n {
                        if p.kind() == "singleton_class" {
                            return true;
                        }
                        if matches!(p.kind(), "class" | "module" | "method" | "singleton_method") {
                            break;
                        }
                        n = p.parent();
                    }
                    false
                });
        let object = node.child_by_field_name("object");
        let target_object = object.and_then(|n| constant(n, source));
        let reliable = ctx.reliable
            && (!explicit
                || object.is_some_and(|n| text(n, source) == "self")
                || target_object.is_some());
        let qualified = if explicit && object.is_some_and(|n| text(n, source) != "self") {
            format!(
                "{}.{}",
                object
                    .map(|n| constant(n, source)
                        .unwrap_or_else(|| format!("<object@{}>", n.start_byte())))
                    .unwrap_or_default(),
                name
            )
        } else if ctx.owner.is_empty() {
            if singleton {
                format!("self.{name}")
            } else {
                name.into()
            }
        } else {
            format!(
                "{}{separator}{name}",
                ctx.owner,
                separator = if singleton { "." } else { "#" }
            )
        };
        let id = symbol(doc, node, name, qualified, source, out);
        out.ruby.as_mut().unwrap().methods.push(Method {
            id: id.clone(),
            owner: ctx.owner.clone(),
            name: name.into(),
            singleton,
            reliable,
            object: target_object,
            lexical: ctx.lexical.clone(),
        });
        let mut nested = ctx.clone();
        nested.singleton = singleton;
        nested.caller = Some(id);
        nested.start = node.start_byte();
        nested.end = node.end_byte();
        nested.reliable = reliable;
        nested.controlled = false;
        if let Some(params) = node.child_by_field_name("parameters") {
            parameters(params, source, doc, &nested, out, depth + 1);
        }
        if let Some(body) = node.child_by_field_name("body") {
            walk(body, source, doc, &nested, out, depth + 1);
        }
        return;
    }
    if matches!(node.kind(), "block" | "do_block" | "lambda") {
        let mut nested = ctx.clone();
        nested.controlled = true;
        if let Some(params) = node.child_by_field_name("parameters") {
            let mut shadow = nested.clone();
            shadow.start = node.start_byte();
            shadow.end = node.end_byte();
            parameters(params, source, doc, &shadow, out, depth + 1);
        }
        if let Some(body) = node.child_by_field_name("body") {
            walk(body, source, doc, &nested, out, depth + 1);
        }
        return;
    }
    if node.kind() == "for"
        || node.kind() == "in_clause"
        || node.kind() == "match_pattern"
        || node.kind() == "test_pattern"
    {
        if let Some(pattern) = node.child_by_field_name("pattern") {
            local_pattern(
                pattern,
                source,
                ctx,
                node.start_byte(),
                out,
                depth + 1,
                false,
            );
        }
        let mut nested = ctx.clone();
        nested.controlled = true;
        for field in ["value", "guard", "body"] {
            if let Some(value) = node.child_by_field_name(field) {
                walk(value, source, doc, &nested, out, depth + 1);
            }
        }
        return;
    }
    if node.kind() == "rescue" {
        if let Some(variable) = node.child_by_field_name("variable") {
            local_pattern(
                variable,
                source,
                ctx,
                node.start_byte(),
                out,
                depth + 1,
                false,
            );
        }
        let mut nested = ctx.clone();
        nested.controlled = true;
        for field in ["exceptions", "body"] {
            if let Some(value) = node.child_by_field_name(field) {
                walk(value, source, doc, &nested, out, depth + 1);
            }
        }
        return;
    }
    if matches!(node.kind(), "assignment" | "operator_assignment") {
        if let Some(left) = node.child_by_field_name("left") {
            if left.kind() == "identifier" {
                out.ruby.as_mut().unwrap().locals.push(Local {
                    name: text(left, source).into(),
                    start: ctx.start,
                    end: ctx.end,
                    visible: node.start_byte(),
                    constructor: if !ctx.controlled && node.kind() == "assignment" {
                        node.child_by_field_name("right")
                            .and_then(|n| constructor(n, source))
                    } else {
                        None
                    },
                    lexical: ctx.lexical.clone(),
                });
            } else if constant(left, source).is_some() {
                out.ruby
                    .as_mut()
                    .unwrap()
                    .constant_writes
                    .insert(text(left, source).into());
            } else if matches!(
                left.kind(),
                "left_assignment_list" | "destructured_left_assignment" | "rest_assignment"
            ) {
                local_pattern(left, source, ctx, node.start_byte(), out, depth + 1, true);
            } else {
                if !out
                    .binding_limitations
                    .iter()
                    .any(|s| s == "RUBY_SETTER_AND_OPERATOR_DISPATCH_UNVERIFIED")
                {
                    out.binding_limitations
                        .push("RUBY_SETTER_AND_OPERATOR_DISPATCH_UNVERIFIED".into());
                }
            }
        }
        if let Some(right) = node.child_by_field_name("right") {
            walk(right, source, doc, ctx, out, depth + 1);
        }
        return;
    }
    if matches!(node.kind(), "alias" | "undef") {
        out.ruby
            .as_mut()
            .unwrap()
            .invalid_owners
            .insert(ctx.owner.clone());
        return;
    }
    if node.kind() == "call" {
        if let Some(method) = node.child_by_field_name("method") {
            let name = text(method, source);
            let receiver = node.child_by_field_name("receiver");
            if matches!(
                name,
                "include"
                    | "prepend"
                    | "extend"
                    | "using"
                    | "alias_method"
                    | "define_method"
                    | "define_singleton_method"
                    | "remove_method"
                    | "undef_method"
                    | "module_function"
                    | "class_eval"
                    | "module_eval"
                    | "instance_eval"
                    | "class_exec"
                    | "module_exec"
                    | "instance_exec"
                    | "eval"
            ) {
                out.ruby.as_mut().unwrap().invalid_owners.insert(
                    if receiver.is_none_or(|n| text(n, source) == "self") {
                        ctx.owner.clone()
                    } else {
                        String::new()
                    },
                );
            }
            if matches!(name, "require_relative" | "require" | "load") && receiver.is_none() {
                if let Some(args) = node.child_by_field_name("arguments") {
                    let values = children(args);
                    if values.len() == 1 {
                        if let Some(value) = string(values[0], source) {
                            file_imports::ruby_import(node, &value, name, out);
                            if name == "require_relative" {
                                out.ruby.as_mut().unwrap().requires.push(value);
                            }
                        }
                    }
                }
            }
            let receiver_kind = match receiver {
                None => Receiver::Implicit,
                Some(n) if n.kind() == "self" => Receiver::SelfValue,
                Some(n) if constant(n, source).is_some() => {
                    Receiver::Constant(constant(n, source).unwrap())
                }
                Some(n) if constructor(n, source).is_some() => {
                    Receiver::Constructed(constructor(n, source).unwrap())
                }
                Some(n) if n.kind() == "identifier" => Receiver::Local(text(n, source).into()),
                _ => Receiver::Unknown,
            };
            let expression = match &receiver_kind {
                Receiver::Implicit => name.to_string(),
                Receiver::SelfValue => format!("self.{name}"),
                Receiver::Constant(raw) | Receiver::Local(raw) => format!("{raw}.{name}"),
                Receiver::Constructed(raw) => format!("{raw}.new(...).{name}"),
                Receiver::Unknown => format!("<receiver@{}>.{name}", node.start_byte()),
            };
            reference(
                doc,
                method,
                name,
                expression,
                ctx,
                ReferenceKind {
                    receiver: receiver_kind,
                    bare: false,
                    type_use: false,
                },
                out,
            );
            if let Some(receiver) = receiver {
                walk(receiver, source, doc, ctx, out, depth + 1);
            }
        }
        if let Some(args) = node.child_by_field_name("arguments") {
            walk(args, source, doc, ctx, out, depth + 1);
        }
        if let Some(block) = node.child_by_field_name("block") {
            walk(block, source, doc, ctx, out, depth + 1);
        }
        return;
    }
    if matches!(node.kind(), "constant" | "scope_resolution") {
        let raw = constant(node, source)
            .unwrap_or_else(|| format!("<dynamic_constant@{}>", node.start_byte()));
        reference(
            doc,
            node,
            &raw,
            raw.clone(),
            ctx,
            ReferenceKind {
                receiver: Receiver::Unknown,
                bare: false,
                type_use: true,
            },
            out,
        );
        if let Some(scope) = node
            .child_by_field_name("scope")
            .filter(|n| n.kind() == "call")
        {
            walk(scope, source, doc, ctx, out, depth + 1);
        }
        return;
    }
    if node.kind() == "identifier" {
        reference(
            doc,
            node,
            text(node, source),
            text(node, source).into(),
            ctx,
            ReferenceKind {
                receiver: Receiver::Implicit,
                bare: true,
                type_use: false,
            },
            out,
        );
        return;
    }
    let mut nested = ctx.clone();
    nested.controlled |= matches!(
        node.kind(),
        "if" | "unless"
            | "case"
            | "while"
            | "until"
            | "for"
            | "rescue"
            | "rescue_modifier"
            | "conditional"
    );
    for child in children(node) {
        walk(child, source, doc, &nested, out, depth + 1);
    }
}
pub(super) fn parse_source(doc: &str, source: &str, out: &mut IndexedDocument) -> Result<()> {
    out.ruby = Some(Facts::default());
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .map_err(|e| AppError::config(e.to_string()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AppError::config("Ruby parse cancelled"))?;
    out.parse_errors = tree.root_node().has_error();
    let ctx = Context {
        owner: String::new(),
        singleton: true,
        lexical: vec![],
        caller: None,
        start: 0,
        end: source.len(),
        reliable: true,
        controlled: false,
    };
    walk(tree.root_node(), source, doc, &ctx, out, 0);
    let facts = out.ruby.as_mut().unwrap();
    for reference in &mut out.references {
        if let Some(site) = facts.sites.get_mut(&reference.start_byte) {
            if site.bare
                && facts.locals.iter().any(|b| {
                    b.name == site.name
                        && b.start <= reference.start_byte
                        && reference.start_byte < b.end
                })
            {
                reference.is_call = false;
                site.local_value = true;
            }
        }
    }
    out.binding_limitations
        .push("RUBY_RUNTIME_MIXINS_VISIBILITY_METAPROGRAMMING_AND_LOAD_ORDER_UNVERIFIED".into());
    Ok(())
}

struct Resolver<'a> {
    methods: HashMap<(String, bool, String), Vec<String>>,
    classes: HashMap<String, Vec<&'a Class>>,
    invalid: BTreeSet<String>,
    constants: BTreeSet<String>,
    complete: bool,
}
impl<'a> Resolver<'a> {
    fn new(docs: &'a HashMap<String, IndexedDocument>, id: &str, doc: &IndexedDocument) -> Self {
        let paths: HashMap<_, Vec<_>> = docs
            .iter()
            .filter(|(_, d)| d.project == doc.project && d.ruby.is_some() && !d.parse_errors)
            .fold(HashMap::new(), |mut map, (id, d)| {
                map.entry(d.path.as_str()).or_default().push(id.as_str());
                map
            });
        let mut scope = BTreeSet::new();
        let mut todo = vec![id];
        let mut work = 0;
        let mut complete = true;
        while let Some(id) = todo.pop() {
            if !scope.insert(id) {
                continue;
            }
            if scope.len() > 4096 {
                complete = false;
                break;
            }
            if let Some(facts) = docs.get(id).and_then(|d| d.ruby.as_ref()) {
                for import in &facts.requires {
                    work += 1;
                    if work > 16384 {
                        complete = false;
                        break;
                    }
                    for path in file_imports::ruby_paths(&docs[id].path, import) {
                        todo.extend(paths.get(path.as_str()).into_iter().flatten().copied());
                    }
                }
            }
            if !complete {
                break;
            }
        }
        let mut out = Self {
            methods: HashMap::new(),
            classes: HashMap::new(),
            invalid: BTreeSet::new(),
            constants: BTreeSet::new(),
            complete,
        };
        for id in &scope {
            if let Some(d) = docs
                .get(*id)
                .filter(|d| !d.parse_errors && d.project == doc.project)
            {
                if let Some(f) = &d.ruby {
                    for c in f.classes.iter().filter(|c| c.reliable) {
                        out.classes.entry(c.name.clone()).or_default().push(c);
                    }
                    out.invalid.extend(f.invalid_owners.iter().cloned());
                    out.constants.extend(f.constant_writes.iter().cloned());
                }
            }
        }
        for member in &scope {
            if let Some(d) = docs
                .get(*member)
                .filter(|d| d.project == doc.project && !d.parse_errors)
            {
                if let Some(facts) = &d.ruby {
                    for m in facts.methods.iter().filter(|m| m.reliable) {
                        let owner = if let Some(raw) = &m.object {
                            let Some(owner) = out.constant(raw, &m.lexical) else {
                                continue;
                            };
                            owner
                        } else {
                            m.owner.clone()
                        };
                        out.methods
                            .entry((owner, m.singleton, m.name.clone()))
                            .or_default()
                            .push(m.id.clone());
                    }
                }
            }
        }
        for methods in out.methods.values_mut() {
            methods.sort();
            methods.dedup();
        }
        out
    }
    fn constant(&self, raw: &str, lexical: &[String]) -> Option<String> {
        let absolute = raw.starts_with("::");
        let raw = raw.trim_start_matches("::");
        if raw.is_empty()
            || raw
                .split("::")
                .any(|p| p.is_empty() || !p.chars().all(|c| c.is_alphanumeric() || c == '_'))
        {
            return None;
        }
        if self.constants.contains(raw) || raw.split("::").any(|p| self.constants.contains(p)) {
            return None;
        }
        for prefix in lexical
            .iter()
            .rev()
            .map(String::as_str)
            .filter(|_| !absolute)
            .chain(std::iter::once(""))
        {
            let name = if prefix.is_empty() {
                raw.into()
            } else {
                format!("{prefix}::{raw}")
            };
            if self.constants.contains(&name) {
                return None;
            }
            if self.classes.contains_key(&name) {
                return Some(name);
            }
        }
        None
    }
    fn parent(&self, owner: &str) -> std::result::Result<Option<String>, ()> {
        let mut bases = BTreeSet::new();
        for class in self.classes.get(owner).into_iter().flatten() {
            if let Some(base) = &class.base {
                let parent = self.constant(base, &class.lexical).ok_or(())?;
                if self
                    .classes
                    .get(&parent)
                    .is_none_or(|types| types.iter().any(|c| c.kind != "class"))
                {
                    return Err(());
                }
                bases.insert(parent);
            }
        }
        if bases.len() > 1 {
            Err(())
        } else {
            Ok(bases.into_iter().next())
        }
    }
    fn lookup(&self, owner: &str, singleton: bool, name: &str) -> (Vec<String>, &'static str) {
        let mut owner = owner.to_string();
        let mut seen = BTreeSet::new();
        for _ in 0..64 {
            if !seen.insert(owner.clone()) {
                return (vec![], "ruby_inheritance_cycle");
            }
            if self.invalid.contains("") || self.invalid.contains(&owner) {
                return (vec![], "ruby_captured_runtime_mutation_unverified");
            }
            if let Some(targets) = self.methods.get(&(owner.clone(), singleton, name.into())) {
                return (targets.clone(), "ruby_captured_owner_method_candidate");
            }
            match self.parent(&owner) {
                Ok(Some(parent)) => owner = parent,
                Ok(None) => return (vec![], "ruby_method_or_implicit_object_ancestor_unresolved"),
                Err(()) => return (vec![], "ruby_superclass_identity_unresolved"),
            }
        }
        (vec![], "ruby_ancestor_budget_exceeded")
    }
    fn instance(&self, raw: &str, lexical: &[String]) -> Option<String> {
        let owner = self.constant(raw, lexical)?;
        if !self.classes.get(&owner)?.iter().all(|c| c.kind == "class") {
            return None;
        }
        let (overrides, reason) = self.lookup(&owner, true, "new");
        if !overrides.is_empty() || reason != "ruby_method_or_implicit_object_ancestor_unresolved" {
            return None;
        }
        Some(owner)
    }
    fn resolve(&self, doc: &IndexedDocument, r: &Reference) -> CallEdge {
        let facts = doc.ruby.as_ref().unwrap();
        let site = facts.sites.get(&r.start_byte);
        let (mut candidates, mut reason) = (vec![], "ruby_site_unresolved");
        if doc.parse_errors {
            reason = "ruby_parse_error";
        } else if !self.complete {
            reason = "ruby_require_scope_budget_exceeded";
        } else if let Some(s) = site {
            if !s.reliable {
                reason = "ruby_dynamic_definition_owner_unverified";
            } else if s.local_value {
                reason = "ruby_local_value_not_method_call";
            } else if s.type_use {
                if let Some(owner) = self.constant(&r.expression, &s.lexical) {
                    candidates = self.classes[&owner].iter().map(|c| c.id.clone()).collect();
                    reason = "ruby_captured_lexical_constant_candidate";
                }
            } else {
                let target = match &s.receiver {
                    Receiver::Implicit | Receiver::SelfValue => {
                        Some((s.owner.clone(), s.singleton))
                    }
                    Receiver::Constant(raw) => self.constant(raw, &s.lexical).map(|o| (o, true)),
                    Receiver::Constructed(raw) => {
                        self.instance(raw, &s.lexical).map(|o| (o, false))
                    }
                    Receiver::Local(name) => {
                        let mut values: Vec<_> = facts
                            .locals
                            .iter()
                            .filter(|b| {
                                b.name == *name && b.start <= r.start_byte && r.start_byte < b.end
                            })
                            .collect();
                        values.sort_by_key(|b| b.end - b.start);
                        values
                            .first()
                            .and_then(|b| {
                                (values
                                    .iter()
                                    .filter(|v| v.start == b.start && v.end == b.end)
                                    .count()
                                    == 1
                                    && b.visible <= r.start_byte)
                                    .then_some(*b)
                            })
                            .and_then(|b| {
                                b.constructor
                                    .as_deref()
                                    .and_then(|raw| self.instance(raw, &b.lexical))
                            })
                            .map(|o| (o, false))
                    }
                    Receiver::Unknown => None,
                };
                if let Some((owner, singleton)) = target {
                    (candidates, reason) = self.lookup(&owner, singleton, &s.name);
                    if candidates.is_empty()
                        && owner.is_empty()
                        && singleton
                        && reason == "ruby_method_or_implicit_object_ancestor_unresolved"
                    {
                        (candidates, reason) = self.lookup(&owner, false, &s.name);
                    }
                } else {
                    reason = "ruby_receiver_or_constructor_unresolved";
                }
            }
        }
        candidates.sort();
        candidates.dedup();
        CallEdge {
            reference: r.clone(),
            resolution: match candidates.len() {
                0 => "unresolved",
                1 => "resolved_static",
                _ => "ambiguous",
            }
            .into(),
            candidates,
            provenance: "derived",
            resolution_basis: reason.into(),
        }
    }
}
pub(super) fn resolve_document(
    symbols: &[Symbol],
    docs: &HashMap<String, IndexedDocument>,
    id: &str,
    doc: &IndexedDocument,
) -> Vec<CallEdge> {
    let resolver = Resolver::new(docs, id, doc);
    let _ = symbols;
    doc.references
        .iter()
        .map(|r| resolver.resolve(doc, r))
        .collect()
}
pub(super) fn edges(
    docs: &HashMap<String, IndexedDocument>,
    id: &str,
    doc: &IndexedDocument,
) -> Vec<inheritance::InheritanceEdge> {
    if doc.parse_errors {
        return vec![];
    }
    let resolver = Resolver::new(docs, id, doc);
    let mut result = vec![];
    if let Some(facts) = &doc.ruby {
        for class in facts.classes.iter().filter(|c| c.reliable) {
            if let Some(base) = &class.base {
                let mut candidates = resolver
                    .constant(base, &class.lexical)
                    .and_then(|name| resolver.classes.get(&name))
                    .into_iter()
                    .flatten()
                    .filter(|c| c.kind == "class")
                    .map(|c| c.id.clone())
                    .collect::<Vec<_>>();
                candidates.sort();
                candidates.dedup();
                result.push(inheritance::InheritanceEdge {
                    derived: class.id.clone(),
                    candidates,
                    base_expression: base.clone(),
                    path: doc.path.clone(),
                    line: class.line,
                    relation: "ruby_superclass_candidate".into(),
                    resolution: "unverified_static_candidates".into(),
                    hierarchy_status: "ruby_runtime_mixin_order_unverified".into(),
                    provenance: "derived",
                });
            }
        }
    }
    result
}
pub(super) fn entrypoint(doc: &IndexedDocument, symbol: &Symbol) -> Option<&'static str> {
    let method = doc
        .ruby
        .as_ref()?
        .methods
        .iter()
        .find(|m| m.id == symbol.id)?;
    if !method.singleton && method.name == "initialize" {
        Some("ruby_constructor_hook_candidate")
    } else if method.name.starts_with("test_") {
        Some("test_function")
    } else if method.owner.is_empty() && method.name == "main" {
        Some("conventional_main")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn docs(files: &[(&str, &str, &str)]) -> HashMap<String, IndexedDocument> {
        files
            .iter()
            .enumerate()
            .map(|(i, (path, project, source))| {
                let mut d = parse(&i.to_string(), path, source, 1).unwrap();
                assert!(!d.parse_errors, "{path}: {source}");
                d.project = Some((*project).into());
                (i.to_string(), d)
            })
            .collect()
    }
    fn bound(files: &[(&str, &str, &str)]) -> (Vec<Symbol>, Vec<CallEdge>) {
        let docs = docs(files);
        let symbols = docs
            .values()
            .flat_map(|d| d.symbols.clone())
            .collect::<Vec<_>>();
        let edges = resolve::references(&symbols, &docs);
        (symbols, edges)
    }
    fn targets(
        symbols: &[Symbol],
        edges: &[CallEdge],
        caller: &str,
        expression: &str,
    ) -> Vec<String> {
        let edge = edges
            .iter()
            .find(|e| {
                e.reference.expression == expression
                    && e.reference
                        .caller
                        .as_ref()
                        .and_then(|id| symbols.iter().find(|s| &s.id == id))
                        .is_some_and(|s| s.qualified_name == caller)
            })
            .unwrap_or_else(|| panic!("missing {caller} {expression}: {edges:?}"));
        edge.candidates
            .iter()
            .map(|id| {
                symbols
                    .iter()
                    .find(|s| &s.id == id)
                    .unwrap()
                    .qualified_name
                    .clone()
            })
            .collect()
    }
    #[test]
    fn ruby_owner_and_receiver_kinds_distinguish_instance_class_and_top_level_methods() {
        let (symbols,edges)=bound(&[("/p/main.rb","p","module A\n class C\n  def helper; 1; end\n  def run; helper(); self.helper(); end\n  def self.helper; 2; end\n  def self.run; helper(); end\n end\nend\ndef helper; 3; end\ndef main\n helper()\n A::C.run()\n c=A::C.new('PRIVATE_BODY')\n c.run()\nend\nmain()\n")]);
        assert_eq!(
            targets(&symbols, &edges, "A::C#run", "helper"),
            ["A::C#helper"]
        );
        assert_eq!(
            targets(&symbols, &edges, "A::C#run", "self.helper"),
            ["A::C#helper"]
        );
        assert_eq!(
            targets(&symbols, &edges, "A::C.run", "helper"),
            ["A::C.helper"]
        );
        assert_eq!(targets(&symbols, &edges, "main", "helper"), ["helper"]);
        assert_eq!(targets(&symbols, &edges, "main", "A::C.run"), ["A::C.run"]);
        assert_eq!(targets(&symbols, &edges, "main", "c.run"), ["A::C#run"]);
        assert!(!serde_json::to_string(&edges)
            .unwrap()
            .contains("PRIVATE_BODY"));
    }
    #[test]
    fn ruby_parameters_reassignments_control_flow_and_block_shadows_do_not_invent_receiver_types() {
        let source="class C\n def run; 1; end\nend\ndef safe\n c=C.new\n c.run()\nend\ndef parameter(c)\n c.run()\nend\ndef changed\n c=C.new\n c=other\n c.run()\nend\ndef conditional(flag)\n if flag\n  c=C.new\n end\n c.run()\nend\ndef blocks\n c=C.new\n [1].each do |c|\n  c.run()\n end\n c.run()\nend\n";
        let (symbols, edges) = bound(&[("/p/main.rb", "p", source)]);
        assert_eq!(targets(&symbols, &edges, "safe", "c.run"), ["C#run"]);
        for caller in ["parameter", "changed", "conditional"] {
            assert!(targets(&symbols, &edges, caller, "c.run").is_empty());
        }
        let calls: Vec<_> = edges
            .iter()
            .filter(|e| {
                e.reference.expression == "c.run"
                    && e.reference
                        .caller
                        .as_ref()
                        .is_some_and(|id| symbols.iter().any(|s| &s.id == id && s.name == "blocks"))
            })
            .collect();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].candidates.is_empty());
        assert_eq!(calls[1].candidates.len(), 1);
        assert!(edges
            .iter()
            .filter(|e| e.reference.name == "c")
            .all(|e| !e.reference.is_call));
    }
    #[test]
    fn ruby_require_relative_superclass_and_project_scopes_preserve_captured_candidates() {
        let files = [
            (
                "/p/lib/base.rb",
                "p",
                "class Base\n def inherited; 1; end\n def self.api; 2; end\nend\n",
            ),
            (
                "/p/lib/child.rb",
                "p",
                "require_relative 'base'\nclass Child < Base\n def run; inherited(); end\nend\n",
            ),
            (
                "/p/main.rb",
                "p",
                "require_relative 'lib/child'\ndef main\n Child.new.run()\n Child.api()\nend\n",
            ),
            (
                "/p/lib/base.rb",
                "foreign",
                "class Base\n def inherited; 9; end\nend\n",
            ),
        ];
        let docs = docs(&files);
        let symbols = docs
            .values()
            .flat_map(|d| d.symbols.clone())
            .collect::<Vec<_>>();
        let refs = resolve::references(&symbols, &docs);
        assert_eq!(
            targets(&symbols, &refs, "Child#run", "inherited"),
            ["Base#inherited"]
        );
        assert_eq!(
            targets(&symbols, &refs, "main", "Child.new(...).run"),
            ["Child#run"]
        );
        assert_eq!(targets(&symbols, &refs, "main", "Child.api"), ["Base.api"]);
        let edges = inheritance::edges(&symbols, &docs);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].candidates.len(), 1);
        assert_eq!(
            symbols
                .iter()
                .find(|s| s.id == edges[0].candidates[0])
                .unwrap()
                .document_id,
            "0"
        );
        assert_ne!(edges[0].hierarchy_status, "complete_static");
        let imports = file_imports::edges(&docs);
        assert_eq!(
            imports.iter().filter(|e| !e.candidates.is_empty()).count(),
            2
        );
    }
    #[test]
    fn ruby_nesting_and_absolute_constants_do_not_guess_qualified_outer_lexical_scope() {
        let source="class Target\n def self.ping; 0; end\nend\nmodule Outer\n class Target\n  def self.ping; 1; end\n end\n module Inner\n  def self.run; Target.ping(); ::Target.ping(); end\n end\nend\nmodule Outer::Flat\n def self.run; Target.ping(); end\nend\n";
        let (symbols, edges) = bound(&[("/p/names.rb", "p", source)]);
        assert_eq!(
            targets(&symbols, &edges, "Outer::Inner.run", "Target.ping"),
            ["Outer::Target.ping"]
        );
        assert_eq!(
            targets(&symbols, &edges, "Outer::Inner.run", "::Target.ping"),
            ["Target.ping"]
        );
        assert_eq!(
            targets(&symbols, &edges, "Outer::Flat.run", "Target.ping"),
            ["Target.ping"]
        );
    }
    #[test]
    fn ruby_reopened_methods_mutations_constant_writes_and_constructor_overrides_remain_explicit() {
        let source="class C\n def run; 1; end\nend\nclass C\n def run; 2; end\nend\nclass D\n include M\n def run; 3; end\nend\nclass E\n def self.new; Object.new; end\n def run; 4; end\nend\nclass F < E\n def run; 5; end\nend\ndef main\n C.new.run()\n D.new.run()\n E.new.run()\n F.new.run()\nend\n";
        let (symbols, edges) = bound(&[("/p/reopen.rb", "p", source)]);
        assert_eq!(targets(&symbols, &edges, "main", "C.new(...).run").len(), 2);
        for receiver in ["D", "E", "F"] {
            assert!(targets(
                &symbols,
                &edges,
                "main",
                &format!("{receiver}.new(...).run")
            )
            .is_empty());
        }
        let (symbols, edges) = bound(&[(
            "/p/alias.rb",
            "p",
            "class C\n def run; 1; end\nend\nC=Other\ndef main; C.new.run(); end\n",
        )]);
        assert!(targets(&symbols, &edges, "main", "C.new(...).run").is_empty());
    }
    #[test]
    fn ruby_bare_values_method_parentheses_endless_defs_and_eigenclass_defs_have_source_evidence() {
        let source="def helper = 'PRIVATE_BODY'\ndef main(helper)\n helper\n helper()\nend\nclass C\n class << self\n  def run?; true; end\n end\nend\nC.run?()\n";
        let docs = docs(&[("/p/syntax.rb", "p", source)]);
        let d = &docs["0"];
        let symbols = d.symbols.clone();
        let refs = resolve::references(&symbols, &docs);
        let helper = symbols.iter().find(|s| s.name == "helper").unwrap();
        assert!(!helper.signature.contains("PRIVATE_BODY"));
        assert_eq!(
            &source[helper.start_byte..helper.end_byte],
            "def helper = 'PRIVATE_BODY'"
        );
        let uses: Vec<_> = refs
            .iter()
            .filter(|e| e.reference.name == "helper")
            .collect();
        assert_eq!(uses.len(), 2);
        assert!(!uses[0].reference.is_call && uses[0].candidates.is_empty());
        assert!(uses[1].reference.is_call && uses[1].candidates.len() == 1);
        assert!(refs
            .iter()
            .any(|e| e.reference.name == "run?" && e.candidates.len() == 1));
        assert!(d
            .binding_limitations
            .iter()
            .any(|s| s.starts_with("RUBY_RUNTIME_")));
    }
    #[test]
    fn ruby_parallel_loop_rescue_and_pattern_bindings_are_values_not_method_calls() {
        let source="def item; 1; end\ndef tuple_values\n item, rest=[1,2]\n item\n item()\nend\ndef loop_values\n for item in [1]\n  item\n end\nend\ndef rescue_values\n begin\n  raise 'error'\n rescue StandardError => item\n  item\n end\nend\ndef pattern_values(value)\n case value\n in [item]\n  item\n end\nend\n";
        let (symbols, edges) = bound(&[("/p/bindings.rb", "p", source)]);
        let uses: Vec<_> = edges
            .iter()
            .filter(|e| e.reference.name == "item")
            .collect();
        assert_eq!(
            uses.iter().filter(|e| e.reference.is_call).count(),
            1,
            "{uses:?}"
        );
        assert_eq!(
            targets(&symbols, &edges, "tuple_values", "item"),
            Vec::<String>::new()
        );
        assert_eq!(
            uses.iter().filter(|e| !e.candidates.is_empty()).count(),
            1,
            "{uses:?}"
        );
    }
    #[test]
    fn ruby_initialize_and_instance_state_do_not_invalidate_normal_method_candidates() {
        let source="class C\n def initialize(value)\n  @value=value\n  ready()\n end\n def ready; 1; end\n def run; ready(); end\nend\nC.new(1).run()\n";
        let documents = docs(&[("/p/constructor.rb", "p", source)]);
        let symbols = documents
            .values()
            .flat_map(|d| d.symbols.clone())
            .collect::<Vec<_>>();
        let edges = resolve::references(&symbols, &documents);
        assert_eq!(
            targets(&symbols, &edges, "C#initialize", "ready"),
            ["C#ready"]
        );
        assert_eq!(targets(&symbols, &edges, "C#run", "ready"), ["C#ready"]);
        let entries = analysis::entrypoints(&symbols, &documents);
        assert!(entries
            .iter()
            .any(|e| e.kind == "ruby_constructor_hook_candidate"));
    }
    #[test]
    fn ruby_dynamic_loads_unknown_receivers_and_parse_errors_do_not_guess_bindings() {
        let docs=docs(&[("/p/a.rb","p","require 'other'\nrequire_relative \"#{prefix}/dynamic\"\nclass C\n def work; 1; end\nend\ndef main(other); other.work(); end\n")]);
        let symbols = docs
            .values()
            .flat_map(|d| d.symbols.clone())
            .collect::<Vec<_>>();
        let refs = resolve::references(&symbols, &docs);
        assert!(targets(&symbols, &refs, "main", "other.work").is_empty());
        let imports = file_imports::edges(&docs);
        assert_eq!(imports.len(), 1);
        assert!(imports[0].candidates.is_empty());
        let broken = parse("broken", "/p/b.rb", "class C\n def work(\n", 1).unwrap();
        assert!(broken.parse_errors);
        let docs = HashMap::from([("broken".into(), broken)]);
        let symbols = docs
            .values()
            .flat_map(|d| d.symbols.clone())
            .collect::<Vec<_>>();
        assert!(resolve::references(&symbols, &docs)
            .iter()
            .all(|e| e.candidates.is_empty()));
        assert!(inheritance::edges(&symbols, &docs).is_empty());
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::models::Document;
    #[test]
    fn ruby_index_cache_refresh_body_and_immutable_import_history_are_qualified() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("ruby.duckdb")).unwrap();
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
            "/p/base.rb",
            "class Base\n def work; 'PRIVATE_BODY'; end\nend\n",
        );
        let mut client=doc("client","/p/client.rb","require_relative 'base'\nclass Child < Base\n def run; work(); end\nend\ndef main; Child.new.run(); end\nmain()\n");
        store.upsert_document(&base).unwrap();
        store.upsert_document(&client).unwrap();
        let report = store.index_code().unwrap();
        assert_eq!(report.indexed, 2);
        let q = CodeQuery {
            symbol: Some("Base#work".into()),
            ..Default::default()
        };
        let snapshot = store.code_snapshot().unwrap();
        let callers = snapshot.query("callers", &q);
        assert!(
            !callers["items"].as_array().unwrap().is_empty(),
            "{callers}"
        );
        let impact = snapshot.query(
            "impact",
            &CodeQuery {
                impact_file: Some("/p/base.rb".into()),
                ..Default::default()
            },
        );
        assert_eq!(impact["items"][0]["path"], "/p/client.rb");
        let body = store.code_body(&q).unwrap();
        assert!(body.to_string().contains("PRIVATE_BODY"));
        assert!(!callers.to_string().contains("PRIVATE_BODY"));
        let full = store
            .index_code_with_options(&resolution_cache::IndexOptions {
                force_full_resolution: true,
            })
            .unwrap();
        assert!(!full.revision_created);
        assert_eq!(full.resolution.unwrap().equivalent_to_existing, Some(true));
        client.content = "class Child\n def run; 1; end\nend\n".into();
        store.upsert_document(&client).unwrap();
        assert!(store.code_snapshot().unwrap().query("callers", &q)["items"]
            .as_array()
            .unwrap()
            .is_empty());
        store.index_code().unwrap();
        let dq = history::DiffQuery {
            before: 1,
            after: Some(2),
            project: Some("p".into()),
            limit: None,
        };
        let diff = store.code_index_diff(&dq).unwrap();
        assert_eq!(diff["ruby_relations_available"], true);
        assert!(
            diff["relations_removed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["kind"] == "file_import_candidate"),
            "{diff}"
        );
        assert!(
            diff["relations_removed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["kind"] == "inheritance"),
            "{diff}"
        );
        client.content = "require_relative 'base'\n".into();
        store.upsert_document(&client).unwrap();
        store.index_code().unwrap();
        assert_eq!(store.code_index_diff(&dq).unwrap(), diff);
        let c = store.lock().unwrap();
        let raw: String = c
            .query_row(
                "SELECT graph_json FROM code_index_revisions WHERE id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut graph: serde_json::Value = serde_json::from_str(&raw).unwrap();
        graph.as_object_mut().unwrap().remove("ruby_syntax_format");
        c.execute(
            "UPDATE code_index_revisions SET graph_json=? WHERE id=1",
            [graph.to_string()],
        )
        .unwrap();
        drop(c);
        let legacy = store.code_index_diff(&dq).unwrap();
        assert_eq!(legacy["ruby_relations_available"], false);
        assert!(!legacy["relations_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["import_kind"] == "ruby_require_relative_candidate"
                || e["relation"] == "ruby_superclass_candidate"));
    }
}
