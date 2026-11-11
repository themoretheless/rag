//! Bounded Rust expression typing from captured AST facts. All edges are static
//! candidates; unknown types, arbitrary operators and runtime dispatch fail closed.
use super::*;
use std::collections::BTreeMap;
mod traits;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Facts {
    functions: BTreeMap<String, Function>,
    fields: BTreeMap<String, Ty>,
    #[serde(default)]
    field_positions: BTreeMap<String, usize>,
    #[serde(default)]
    field_declarations: Vec<FieldDeclaration>,
    aliases: BTreeMap<String, Ty>,
    bindings: Vec<Binding>,
    expressions: BTreeMap<String, Expr>,
    invalidations: Vec<Invalidation>,
    #[serde(default)]
    trait_data: traits::Facts,
    #[serde(default)]
    macro_ranges: Vec<(usize, usize)>,
}
pub(super) fn configure(doc: &mut IndexedDocument, decision: &impl Fn(usize) -> Option<bool>) {
    let ids: HashSet<_> = doc.symbols.iter().map(|s| s.id.clone()).collect();
    let facts = &mut doc.rust_semantics;
    facts.functions.retain(|id, _| ids.contains(id));
    facts.aliases.retain(|id, _| ids.contains(id));
    if facts.field_declarations.is_empty() {
        facts.fields.retain(|name, _| {
            facts
                .field_positions
                .get(name)
                .is_some_and(|at| decision(*at) == Some(true))
        });
    } else {
        facts
            .field_declarations
            .retain(|f| ids.contains(&f.owner) && decision(f.at) != Some(false));
        facts.fields.clear();
        facts.field_positions.clear();
        let mut groups: BTreeMap<&str, Vec<&FieldDeclaration>> = BTreeMap::new();
        for field in &facts.field_declarations {
            groups.entry(&field.name).or_default().push(field);
        }
        for (name, fields) in groups {
            if fields.len() == 1 && decision(fields[0].at) == Some(true) {
                facts.fields.insert(name.into(), fields[0].ty.clone());
                facts.field_positions.insert(name.into(), fields[0].at);
            }
        }
    }
    facts
        .bindings
        .retain(|b| decision(b.declared) == Some(true));
    // An unknown conditional mutation may execute. Keep its invalidation even
    // though references in that unknown source range cannot be published.
    facts
        .invalidations
        .retain(|i| decision(i.at) != Some(false));
    facts.expressions.retain(|key, _| {
        key.split_once(':')
            .and_then(|(n, _)| n.parse::<usize>().ok())
            .is_some_and(|n| decision(n) == Some(true))
    });
    traits::configure(&mut facts.trait_data, decision, &ids);
}
pub(super) fn remap(doc: &mut IndexedDocument, ids: &HashMap<String, String>) {
    let facts = &mut doc.rust_semantics;
    facts.functions = std::mem::take(&mut facts.functions)
        .into_iter()
        .filter_map(|(id, f)| ids.get(&id).map(|next| (next.clone(), f)))
        .collect();
    facts.aliases = std::mem::take(&mut facts.aliases)
        .into_iter()
        .filter_map(|(id, t)| ids.get(&id).map(|next| (next.clone(), t)))
        .collect();
    for field in &mut facts.field_declarations {
        if let Some(id) = ids.get(&field.owner) {
            field.owner = id.clone();
        }
    }
    traits::remap(&mut facts.trait_data, ids);
}
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Ty {
    Named(String, Vec<Ty>),
    Reference(Box<Ty>),
    Pointer(bool, Box<Ty>),
    Primitive(String),
    Array(Box<Ty>),
    Bounds(Vec<Ty>),
    Projection(Box<Ty>, Box<Ty>, String),
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FieldDeclaration {
    owner: String,
    name: String,
    ty: Ty,
    at: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Expr {
    Name(String),
    Member(Box<Expr>, String),
    Call(Box<Expr>, Vec<Ty>),
    Await(Box<Expr>),
    Index(Box<Expr>),
    Construct(Ty),
    Reference(Box<Expr>),
    Ufcs(Ty, Ty, String),
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Function {
    #[serde(default)]
    self_type: Option<Ty>,
    result: Ty,
    asynchronous: bool,
    receiver: bool,
    generics: Vec<String>,
    enclosing_generics: Vec<String>,
    #[serde(default)]
    bounds: BTreeMap<String, Vec<Ty>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Binding {
    name: String,
    ty: Option<Ty>,
    init: Option<Expr>,
    start: usize,
    end: usize,
    declared: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Invalidation {
    name: String,
    binding_start: usize,
    binding_end: usize,
    at: usize,
}

fn children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}
fn ty(node: Node, source: &str, depth: usize) -> Ty {
    if depth > 32 {
        return Ty::Unknown;
    }
    match node.kind() {
        "primitive_type" => Ty::Primitive(text(node, source).into()),
        "pointer_type" => node
            .child_by_field_name("type")
            .map(|n| {
                Ty::Pointer(
                    children(node)
                        .iter()
                        .any(|c| c.kind() == "mutable_specifier"),
                    Box::new(ty(n, source, depth + 1)),
                )
            })
            .unwrap_or(Ty::Unknown),
        "type_identifier" | "scoped_type_identifier" | "identifier" => {
            if let Some(path) = node
                .child_by_field_name("path")
                .filter(|p| p.kind() == "bracketed_type")
            {
                if let Some(qualified) =
                    path.named_child(0).filter(|p| p.kind() == "qualified_type")
                {
                    if let (Some(t), Some(tr), Some(name)) = (
                        qualified.child_by_field_name("type"),
                        qualified.child_by_field_name("alias"),
                        node.child_by_field_name("name"),
                    ) {
                        return Ty::Projection(
                            Box::new(ty(t, source, depth + 1)),
                            Box::new(ty(tr, source, depth + 1)),
                            text(name, source).into(),
                        );
                    }
                }
            }
            Ty::Named(text(node, source).into(), vec![])
        }
        "dynamic_type" | "abstract_type" => node
            .child_by_field_name("trait")
            .map(|n| Ty::Bounds(traits::bound_types(n, source)))
            .unwrap_or(Ty::Unknown),
        "bounded_type" => Ty::Bounds(traits::bound_types(node, source)),
        "reference_type" => node
            .child_by_field_name("type")
            .map(|n| Ty::Reference(Box::new(ty(n, source, depth + 1))))
            .unwrap_or(Ty::Unknown),
        "array_type" => node
            .child_by_field_name("element")
            .map(|n| Ty::Array(Box::new(ty(n, source, depth + 1))))
            .unwrap_or(Ty::Unknown),
        "generic_type" | "generic_type_with_turbofish" => {
            let Some(name) = node.child_by_field_name("type") else {
                return Ty::Unknown;
            };
            let args = node
                .child_by_field_name("type_arguments")
                .map(|n| {
                    children(n)
                        .into_iter()
                        .filter(|n| n.kind() != "lifetime")
                        .map(|n| ty(n, source, depth + 1))
                        .collect()
                })
                .unwrap_or_default();
            Ty::Named(text(name, source).into(), args)
        }
        _ => Ty::Unknown,
    }
}
fn expr(node: Node, source: &str, depth: usize) -> Expr {
    if depth > 32 {
        return Expr::Unknown;
    }
    let nested = |n| Box::new(expr(n, source, depth + 1));
    match node.kind() {
        "identifier" | "scoped_identifier" | "self" => {
            if let Some(path) = node
                .child_by_field_name("path")
                .filter(|p| p.kind() == "bracketed_type")
            {
                if let Some(qualified) =
                    path.named_child(0).filter(|p| p.kind() == "qualified_type")
                {
                    if let (Some(t), Some(tr), Some(name)) = (
                        qualified.child_by_field_name("type"),
                        qualified.child_by_field_name("alias"),
                        node.child_by_field_name("name"),
                    ) {
                        return Expr::Ufcs(
                            ty(t, source, 0),
                            ty(tr, source, 0),
                            text(name, source).into(),
                        );
                    }
                }
            }
            Expr::Name(text(node, source).into())
        }
        "field_expression" => match (
            node.child_by_field_name("value"),
            node.child_by_field_name("field"),
        ) {
            (Some(value), Some(field)) => Expr::Member(nested(value), text(field, source).into()),
            _ => Expr::Unknown,
        },
        "call_expression" => {
            let Some(function) = node.child_by_field_name("function") else {
                return Expr::Unknown;
            };
            if function.kind() == "generic_function" {
                let Some(callee) = function.child_by_field_name("function") else {
                    return Expr::Unknown;
                };
                let args = function
                    .child_by_field_name("type_arguments")
                    .map(|n| {
                        children(n)
                            .into_iter()
                            .filter(|n| n.kind() != "lifetime")
                            .map(|n| ty(n, source, depth + 1))
                            .collect()
                    })
                    .unwrap_or_default();
                Expr::Call(nested(callee), args)
            } else {
                Expr::Call(nested(function), vec![])
            }
        }
        "parenthesized_expression" => node
            .named_child(0)
            .map(|n| expr(n, source, depth + 1))
            .unwrap_or(Expr::Unknown),
        "await_expression" => node
            .named_child(0)
            .map(|n| Expr::Await(nested(n)))
            .unwrap_or(Expr::Unknown),
        "reference_expression" => node
            .child_by_field_name("value")
            .map(|n| Expr::Reference(nested(n)))
            .unwrap_or(Expr::Unknown),
        "index_expression" => node
            .named_child(0)
            .map(|n| Expr::Index(nested(n)))
            .unwrap_or(Expr::Unknown),
        "type_cast_expression" => node
            .child_by_field_name("type")
            .map(|n| Expr::Construct(ty(n, source, depth + 1)))
            .unwrap_or(Expr::Unknown),
        "struct_expression" => node
            .child_by_field_name("name")
            .map(|n| Expr::Construct(ty(n, source, depth + 1)))
            .unwrap_or(Expr::Unknown),
        _ => Expr::Unknown,
    }
}
fn scope(node: Node) -> (usize, usize) {
    let mut parent = node.parent();
    while let Some(n) = parent {
        if matches!(n.kind(), "block" | "function_item" | "closure_expression") {
            return (n.start_byte(), n.end_byte());
        }
        parent = n.parent();
    }
    (0, usize::MAX)
}
pub(super) fn collect(root: Node, source: &str, doc: &mut IndexedDocument) {
    let mut facts = Facts::default();
    let mut stack = vec![(root, 0)];
    while let Some((node, depth)) = stack.pop() {
        if depth > 128 {
            continue;
        }
        match node.kind() {
            "macro_invocation" | "macro_definition" => {
                facts
                    .macro_ranges
                    .push((node.start_byte(), node.end_byte()));
                continue;
            }
            "function_item" | "function_signature_item" => {
                if let Some(s) = doc
                    .symbols
                    .iter()
                    .find(|s| s.start_byte == node.start_byte())
                {
                    let mut enclosing_generics = Vec::new();
                    let mut self_type = None;
                    let mut ancestor = node.parent();
                    while let Some(parent) = ancestor {
                        if self_type.is_none() {
                            if parent.kind() == "function_item" {
                                // Nested items do not inherit an enclosing
                                // method's Self type or receiver context.
                                self_type = Some(Ty::Unknown);
                            } else if parent.kind() == "impl_item" {
                                self_type =
                                    parent.child_by_field_name("type").map(|n| ty(n, source, 0));
                            }
                        }
                        if let Some(parameters) = parent.child_by_field_name("type_parameters") {
                            enclosing_generics.extend(
                                children(parameters)
                                    .into_iter()
                                    .filter(|p| p.kind() == "type_parameter")
                                    .filter_map(|p| p.child_by_field_name("name"))
                                    .map(|p| text(p, source).to_string()),
                            );
                        }
                        ancestor = parent.parent();
                    }
                    let generics = node
                        .child_by_field_name("type_parameters")
                        .map(|n| {
                            children(n)
                                .into_iter()
                                .filter(|n| n.kind() == "type_parameter")
                                .filter_map(|n| n.child_by_field_name("name"))
                                .map(|n| text(n, source).to_string())
                                .collect()
                        })
                        .unwrap_or_default();
                    facts.functions.insert(
                        s.id.clone(),
                        Function {
                            self_type,
                            result: node
                                .child_by_field_name("return_type")
                                .map(|n| ty(n, source, 0))
                                .unwrap_or(Ty::Unknown),
                            asynchronous: children(node).iter().any(|n| {
                                n.kind() == "function_modifiers"
                                    && text(*n, source).split_whitespace().any(|v| v == "async")
                            }),
                            receiver: node.child_by_field_name("parameters").is_some_and(|p| {
                                children(p).iter().any(|n| {
                                    n.kind() == "self_parameter"
                                        || n.child_by_field_name("pattern")
                                            .is_some_and(|p| text(p, source) == "self")
                                })
                            }),
                            generics,
                            enclosing_generics,
                            bounds: traits::bounds(node, source),
                        },
                    );
                }
            }
            "type_item" => {
                if let (Some(s), Some(t)) = (
                    doc.symbols
                        .iter()
                        .find(|s| s.start_byte == node.start_byte()),
                    node.child_by_field_name("type"),
                ) {
                    if node.child_by_field_name("type_parameters").is_none() {
                        facts.aliases.insert(s.id.clone(), ty(t, source, 0));
                    }
                }
            }
            "ordered_field_declaration_list" => {
                // Positional names belong to tuple structs, not enum variants.
                // Conditional slots change subsequent indices; until their layout
                // is projected, no raw ordinal may be advertised as configured.
                if let Some(parent) = node.parent().filter(|p| p.kind() == "struct_item") {
                    if !children(node).iter().any(|n| n.kind() == "attribute_item") {
                        if let Some(owner) = doc.symbols.iter().find(|s| {
                            s.kind == "struct_item" && s.start_byte == parent.start_byte()
                        }) {
                            let mut cursor = node.walk();
                            for (index, field) in
                                node.children_by_field_name("type", &mut cursor).enumerate()
                            {
                                let name = format!("{}::{index}", owner.qualified_name);
                                let field_type = ty(field, source, 0);
                                facts.fields.insert(name.clone(), field_type.clone());
                                facts
                                    .field_positions
                                    .insert(name.clone(), field.start_byte());
                                facts.field_declarations.push(FieldDeclaration {
                                    owner: owner.id.clone(),
                                    name,
                                    ty: field_type,
                                    at: field.start_byte(),
                                });
                            }
                        }
                    }
                }
            }
            "field_declaration" => {
                if let (Some(name), Some(t)) = (
                    node.child_by_field_name("name"),
                    node.child_by_field_name("type"),
                ) {
                    if let Some(owner) = doc
                        .symbols
                        .iter()
                        .filter(|s| {
                            s.kind == "struct_item"
                                && s.start_byte <= node.start_byte()
                                && node.end_byte() <= s.end_byte
                        })
                        .min_by_key(|s| s.end_byte - s.start_byte)
                    {
                        facts.fields.insert(
                            format!("{}::{}", owner.qualified_name, text(name, source)),
                            ty(t, source, 0),
                        );
                        facts.field_positions.insert(
                            format!("{}::{}", owner.qualified_name, text(name, source)),
                            node.start_byte(),
                        );
                        facts.field_declarations.push(FieldDeclaration {
                            owner: owner.id.clone(),
                            name: format!("{}::{}", owner.qualified_name, text(name, source)),
                            ty: ty(t, source, 0),
                            at: node.start_byte(),
                        });
                    }
                }
            }
            "parameter"
            | "let_declaration"
            | "assignment_expression"
            | "compound_assignment_expr" => {
                let assignment = matches!(
                    node.kind(),
                    "assignment_expression" | "compound_assignment_expr"
                );
                let pattern = node.child_by_field_name(if assignment { "left" } else { "pattern" });
                if let Some(pattern) = pattern.filter(|n| n.kind() != "identifier") {
                    let (start, end) = scope(node);
                    let mut pending = vec![pattern];
                    while let Some(p) = pending.pop() {
                        if p.kind() == "identifier" {
                            facts.bindings.push(Binding {
                                name: text(p, source).into(),
                                ty: None,
                                init: None,
                                start,
                                end,
                                declared: if node.kind() == "parameter" {
                                    start
                                } else {
                                    node.start_byte()
                                },
                            });
                        }
                        pending.extend(children(p));
                    }
                } else if let Some(pattern) = pattern {
                    let (start, end) = scope(node);
                    // An assignment beneath control flow cannot establish a post-branch type.
                    // Reassignment conservatively invalidates initializer-based inference.
                    facts.bindings.push(Binding {
                        name: text(pattern, source).into(),
                        start,
                        end,
                        declared: if node.kind() == "parameter" {
                            start
                        } else {
                            node.start_byte()
                        },
                        ty: node.child_by_field_name("type").map(|n| ty(n, source, 0)),
                        init: if assignment {
                            None
                        } else {
                            node.child_by_field_name("value")
                                .map(|n| expr(n, source, 0))
                        },
                    });
                }
            }
            "closure_parameters" => {
                let (start, end) = scope(node);
                let mut pending = children(node);
                while let Some(p) = pending.pop() {
                    if p.kind() == "identifier" {
                        facts.bindings.push(Binding {
                            name: text(p, source).into(),
                            ty: None,
                            init: None,
                            start,
                            end,
                            declared: start,
                        });
                    }
                    pending.extend(children(p));
                }
            }
            "call_expression" => {
                if let Some(function) = node.child_by_field_name("function") {
                    facts.expressions.insert(
                        format!("{}:{}", node.start_byte(), text(function, source)),
                        expr(node, source, 0),
                    );
                }
            }
            _ => (),
        }
        stack.extend(children(node).into_iter().rev().map(|n| (n, depth + 1)));
    }
    // Mutations in a branch still invalidate the outer initializer after the
    // branch. Keep the identity of the lexical binding rather than the name alone.
    let assignments: Vec<_> = facts
        .bindings
        .iter()
        .filter(|b| b.ty.is_none() && b.init.is_none())
        .cloned()
        .collect();
    for assignment in assignments {
        if let Some(original) = facts
            .bindings
            .iter()
            .filter(|b| {
                b.name == assignment.name
                    && b.start <= assignment.declared
                    && assignment.declared < b.end
                    && b.declared < assignment.declared
            })
            .max_by_key(|b| (b.start, b.declared))
        {
            facts.invalidations.push(Invalidation {
                name: assignment.name,
                binding_start: original.start,
                binding_end: original.end,
                at: assignment.declared,
            });
        }
    }
    facts.trait_data = traits::collect(root, source, doc);
    doc.rust_semantics = facts;
}

#[derive(Debug, Clone, PartialEq)]
enum ValueType {
    Nominal(String, Vec<ValueType>),
    Traits(Vec<String>),
    Standard(String, Vec<ValueType>),
    Reference(Box<ValueType>),
    Pointer(bool, Box<ValueType>),
    Primitive(String),
    Array(Box<ValueType>),
    Future(Box<ValueType>),
    Unknown,
}
struct Engine<'a> {
    symbols: &'a [Symbol],
    docs: &'a HashMap<String, IndexedDocument>,
    budget: usize,
}
impl<'a> Engine<'a> {
    fn step(&mut self) -> bool {
        if self.budget == 0 {
            false
        } else {
            self.budget -= 1;
            true
        }
    }
    fn symbol(&self, id: &str) -> Option<&'a Symbol> {
        self.symbols.iter().find(|s| s.id == id)
    }
    fn types(
        &mut self,
        doc: &IndexedDocument,
        at: usize,
        owner: &str,
        t: &Ty,
        substitutions: &HashMap<String, ValueType>,
        depth: usize,
    ) -> ValueType {
        if depth > 32 || !self.step() {
            return ValueType::Unknown;
        }
        match t {
            Ty::Primitive(name) => ValueType::Primitive(name.clone()),
            Ty::Pointer(mutable, t) => ValueType::Pointer(
                *mutable,
                Box::new(self.types(doc, at, owner, t, substitutions, depth + 1)),
            ),
            Ty::Reference(t) => ValueType::Reference(Box::new(self.types(
                doc,
                at,
                owner,
                t,
                substitutions,
                depth + 1,
            ))),
            Ty::Array(t) => ValueType::Array(Box::new(self.types(
                doc,
                at,
                owner,
                t,
                substitutions,
                depth + 1,
            ))),
            Ty::Unknown => ValueType::Unknown,
            Ty::Projection(t, tr, name) => {
                let value = self.types(doc, at, owner, t, substitutions, depth + 1);
                let ids = self.trait_ids(
                    doc,
                    at,
                    owner,
                    std::slice::from_ref(tr.as_ref()),
                    substitutions,
                    depth + 1,
                );
                self.associated_type(&value, name, &ids, depth + 1)
            }
            Ty::Bounds(bounds) => {
                let ids = self.trait_ids(doc, at, owner, bounds, substitutions, depth + 1);
                if ids.is_empty() {
                    ValueType::Unknown
                } else {
                    ValueType::Traits(ids)
                }
            }
            Ty::Named(name, args) => {
                if let Some(t) = substitutions.get(name) {
                    return t.clone();
                }
                if let Some((head, item)) = name.split_once("::") {
                    if let Some(value) = substitutions.get(head) {
                        let bounds = doc
                            .rust_semantics
                            .functions
                            .iter()
                            .find(|(id, _)| {
                                self.symbol(id).is_some_and(|s| s.qualified_name == owner)
                            })
                            .and_then(|(_, f)| f.bounds.get(head))
                            .cloned()
                            .unwrap_or_default();
                        let ids = self.trait_ids(doc, at, owner, &bounds, substitutions, depth + 1);
                        return self.associated_type(value, item, &ids, depth + 1);
                    }
                }
                if name == "Self" {
                    let captured = doc
                        .rust_semantics
                        .functions
                        .iter()
                        .find(|(id, _)| self.symbol(id).is_some_and(|s| s.qualified_name == owner))
                        .and_then(|(_, f)| f.self_type.clone());
                    if let Some(captured) = captured {
                        return self.types(doc, at, owner, &captured, substitutions, depth + 1);
                    }
                }
                let name = if name == "Self" {
                    owner.rsplit_once("::").map(|v| v.0).unwrap_or("")
                } else {
                    name
                };
                if let Some((_, function)) = doc.rust_semantics.functions.iter().find(|(id, f)| {
                    self.symbol(id).is_some_and(|s| s.qualified_name == owner)
                        && f.generics
                            .iter()
                            .chain(&f.enclosing_generics)
                            .any(|g| g == name)
                }) {
                    let bounds = function.bounds.get(name).cloned().unwrap_or_default();
                    let ids = self.trait_ids(doc, at, owner, &bounds, substitutions, depth + 1);
                    return if ids.is_empty() {
                        ValueType::Unknown
                    } else {
                        ValueType::Traits(ids)
                    };
                }
                if resolve::unknown_rust_namespace(doc, name.split("::").next().unwrap_or(""), at) {
                    return ValueType::Unknown;
                }
                let found: Vec<_> = self
                    .names(doc, at, owner, name, false)
                    .into_iter()
                    .filter(|s| {
                        matches!(
                            s.kind.as_str(),
                            "struct_item" | "enum_item" | "trait_item" | "type_item"
                        )
                    })
                    .collect();
                if found.len() == 1 {
                    let declaration = found[0];
                    if declaration.kind == "type_item" {
                        let Some(d) = self.docs.get(&declaration.document_id) else {
                            return ValueType::Unknown;
                        };
                        let Some(alias) = d.rust_semantics.aliases.get(&declaration.id) else {
                            return ValueType::Unknown;
                        };
                        return self.types(
                            d,
                            declaration.start_byte,
                            &declaration.qualified_name,
                            alias,
                            &HashMap::new(),
                            depth + 1,
                        );
                    }
                    let expected = self
                        .docs
                        .get(&declaration.document_id)
                        .and_then(|d| {
                            d.rust_semantics
                                .trait_data
                                .type_parameters
                                .get(&declaration.id)
                        })
                        .map_or(0, Vec::len);
                    if expected != args.len() {
                        return ValueType::Unknown;
                    }
                    let arguments = args
                        .iter()
                        .map(|t| self.types(doc, at, owner, t, substitutions, depth + 1))
                        .collect();
                    return ValueType::Nominal(declaration.id.clone(), arguments);
                }
                if !found.is_empty() {
                    return ValueType::Unknown;
                }
                let (head, tail) = name.split_once("::").unwrap_or((name, ""));
                let canonical = if let Some(import) = resolve::import_target(doc, head, at) {
                    let import = import.strip_prefix("\0extern_crate::").unwrap_or(import);
                    if tail.is_empty() {
                        import.to_string()
                    } else {
                        format!("{import}::{tail}")
                    }
                } else {
                    name.to_string()
                };
                let terminal = canonical.rsplit("::").next().unwrap_or("");
                let standard = (matches!(canonical.as_str(), "Box" | "Arc" | "Rc" | "Vec")
                    || matches!(
                        canonical.as_str(),
                        "std::boxed::Box"
                            | "alloc::boxed::Box"
                            | "std::sync::Arc"
                            | "alloc::sync::Arc"
                            | "std::rc::Rc"
                            | "alloc::rc::Rc"
                            | "std::vec::Vec"
                            | "alloc::vec::Vec"
                    ))
                    && rust_cargo::standard_crate_unshadowed(
                        self.docs,
                        doc,
                        at,
                        canonical.split("::").next().unwrap_or(""),
                    );
                if standard && args.len() == 1 {
                    // A user-defined generic parameter with a prelude name still shadows std.
                    if doc.rust_semantics.functions.iter().any(|(id, f)| {
                        self.symbol(id).is_some_and(|s| s.qualified_name == owner)
                            && f.generics
                                .iter()
                                .chain(&f.enclosing_generics)
                                .any(|g| g == head)
                    }) {
                        return ValueType::Unknown;
                    }
                    return ValueType::Standard(
                        terminal.into(),
                        vec![self.types(doc, at, owner, &args[0], substitutions, depth + 1)],
                    );
                }
                ValueType::Unknown
            }
        }
    }
    fn names(
        &mut self,
        doc: &IndexedDocument,
        at: usize,
        owner: &str,
        name: &str,
        shadow_values: bool,
    ) -> Vec<&'a Symbol> {
        if !self.step() {
            return vec![];
        }
        let (head, tail) = name.split_once("::").unwrap_or((name, ""));
        let shadow = doc
            .rust_semantics
            .bindings
            .iter()
            .any(|b| b.name == head && b.start <= at && at < b.end && b.declared <= at);
        if shadow && shadow_values {
            return vec![];
        }
        let mut found = if let Some(import) = resolve::import_target(doc, head, at) {
            resolve::imported(
                self.symbols,
                self.docs,
                &doc.path,
                import,
                (!tail.is_empty()).then_some(tail),
            )
        } else {
            let module = rust_cargo::inline_namespace(doc, at).join("::");
            let mut prefix = owner.rsplit_once("::").map(|v| v.0).unwrap_or(&module);
            let mut result = loop {
                let qualified = if prefix.is_empty() {
                    name.to_string()
                } else {
                    format!("{prefix}::{name}")
                };
                let type_scope = !name.contains("::")
                    && self.symbols.iter().any(|s| {
                        s.path == doc.path
                            && s.qualified_name == prefix
                            && matches!(s.kind.as_str(), "struct_item" | "enum_item" | "trait_item")
                    });
                let found = if type_scope {
                    vec![]
                } else {
                    inheritance::lookup(self.symbols, self.docs, &doc.path, &qualified)
                };
                if !found.is_empty() || prefix.is_empty() {
                    break found;
                }
                prefix = prefix.rsplit_once("::").map(|v| v.0).unwrap_or("");
            };
            if result.is_empty() && name.contains("::") {
                let target = rust_cargo::inline_relative_path(doc, at, name);
                result = resolve::imported(self.symbols, self.docs, &doc.path, &target, None);
            }
            result
        };
        found.retain(|s| {
            self.docs
                .get(&s.document_id)
                .is_some_and(|d| d.project == doc.project)
        });
        found.sort_by_key(|s| &s.id);
        found.dedup_by_key(|s| &s.id);
        found
    }
    fn methods(
        &mut self,
        value: &ValueType,
        name: &str,
        context: (&IndexedDocument, usize, &str),
        depth: usize,
    ) -> Vec<&'a Symbol> {
        if depth > 16 || !self.step() {
            return vec![];
        }
        match value {
            ValueType::Nominal(..) | ValueType::Traits(..) => {
                self.trait_methods(value, name, context, None, true, depth + 1)
            }
            ValueType::Pointer(..) => {
                // Inherent sysroot methods take precedence over extension traits.
                // Their bodies are outside this captured source graph.
                if matches!(
                    name,
                    "add"
                        | "addr"
                        | "align_offset"
                        | "as_array"
                        | "as_mut"
                        | "as_mut_array"
                        | "as_mut_ptr"
                        | "as_mut_slice"
                        | "as_mut_unchecked"
                        | "as_ptr"
                        | "as_ref"
                        | "as_ref_unchecked"
                        | "as_slice"
                        | "as_uninit_mut"
                        | "as_uninit_ref"
                        | "as_uninit_slice"
                        | "as_uninit_slice_mut"
                        | "byte_add"
                        | "byte_offset"
                        | "byte_offset_from"
                        | "byte_offset_from_unsigned"
                        | "byte_sub"
                        | "cast"
                        | "cast_array"
                        | "cast_const"
                        | "cast_init"
                        | "cast_mut"
                        | "cast_slice"
                        | "cast_uninit"
                        | "copy_from"
                        | "copy_from_nonoverlapping"
                        | "copy_to"
                        | "copy_to_nonoverlapping"
                        | "drop_in_place"
                        | "expose_provenance"
                        | "get_unchecked"
                        | "get_unchecked_mut"
                        | "guaranteed_eq"
                        | "guaranteed_ne"
                        | "is_aligned"
                        | "is_aligned_to"
                        | "is_empty"
                        | "is_null"
                        | "len"
                        | "map_addr"
                        | "mask"
                        | "offset"
                        | "offset_from"
                        | "offset_from_unsigned"
                        | "read"
                        | "read_unaligned"
                        | "read_volatile"
                        | "replace"
                        | "split_at_mut"
                        | "split_at_mut_unchecked"
                        | "sub"
                        | "swap"
                        | "to_raw_parts"
                        | "try_cast_aligned"
                        | "with_addr"
                        | "with_metadata_of"
                        | "wrapping_add"
                        | "wrapping_byte_add"
                        | "wrapping_byte_offset"
                        | "wrapping_byte_sub"
                        | "wrapping_offset"
                        | "wrapping_sub"
                        | "write"
                        | "write_bytes"
                        | "write_unaligned"
                        | "write_volatile"
                ) {
                    return vec![];
                }
                self.trait_methods(value, name, context, None, true, depth + 1)
            }
            ValueType::Reference(t) => self.methods(t, name, context, depth + 1),
            ValueType::Standard(wrapper, args)
                if matches!(wrapper.as_str(), "Box" | "Arc" | "Rc") =>
            {
                // Do not unwrap when captured local-trait implementations could own dispatch.
                if self.docs.values().any(|d| {
                    d.bases.iter().any(|b| {
                        b.relation == "implements"
                            && b.derived
                                .split('<')
                                .next()
                                .is_some_and(|n| n.rsplit("::").next() == Some(wrapper))
                    })
                }) {
                    return vec![];
                }
                self.methods(&args[0], name, context, depth + 1)
            }
            _ => vec![],
        }
    }
    fn value(
        &mut self,
        doc: &IndexedDocument,
        at: usize,
        owner: &str,
        e: &Expr,
        depth: usize,
    ) -> ValueType {
        if depth > 32 || !self.step() {
            return ValueType::Unknown;
        }
        match e {
            Expr::Name(name) if name == "self" || name == "Self" => self.types(
                doc,
                at,
                owner,
                &Ty::Named("Self".into(), vec![]),
                &HashMap::new(),
                depth + 1,
            ),
            Expr::Name(name) => {
                let binding = doc
                    .rust_semantics
                    .bindings
                    .iter()
                    .filter(|b| b.name == *name && b.start <= at && at < b.end && b.declared <= at)
                    .max_by_key(|b| (b.start, b.declared));
                let Some(b) = binding else {
                    return ValueType::Unknown;
                };
                if doc.rust_semantics.invalidations.iter().any(|i| {
                    i.name == b.name
                        && i.binding_start == b.start
                        && i.binding_end == b.end
                        && b.declared < i.at
                        && i.at <= at
                }) {
                    return ValueType::Unknown;
                }
                if let Some(t) = &b.ty {
                    self.types(doc, at, owner, t, &HashMap::new(), depth + 1)
                } else if let Some(init) = &b.init {
                    self.value(doc, b.declared.saturating_sub(1), owner, init, depth + 1)
                } else {
                    ValueType::Unknown
                }
            }
            Expr::Construct(t) => self.types(doc, at, owner, t, &HashMap::new(), depth + 1),
            Expr::Reference(e) => {
                ValueType::Reference(Box::new(self.value(doc, at, owner, e, depth + 1)))
            }
            Expr::Await(e) => match self.value(doc, at, owner, e, depth + 1) {
                ValueType::Future(t) => *t,
                _ => ValueType::Unknown,
            },
            Expr::Index(e) => {
                let mut t = self.value(doc, at, owner, e, depth + 1);
                for _ in 0..16 {
                    if let ValueType::Reference(inner) = t {
                        t = *inner;
                    } else {
                        break;
                    }
                }
                match t {
                    ValueType::Array(t) => *t,
                    ValueType::Standard(name, args) if name == "Vec" => args[0].clone(),
                    _ => ValueType::Unknown,
                }
            }
            Expr::Member(e, field) => {
                let mut t = self.value(doc, at, owner, e, depth + 1);
                for _ in 0..16 {
                    match t {
                        ValueType::Reference(inner) => t = *inner,
                        ValueType::Standard(ref n, ref a)
                            if matches!(n.as_str(), "Box" | "Arc" | "Rc") =>
                        {
                            t = a[0].clone()
                        }
                        _ => break,
                    }
                }
                let ValueType::Nominal(id, arguments) = t else {
                    return ValueType::Unknown;
                };
                let Some(s) = self.symbol(&id) else {
                    return ValueType::Unknown;
                };
                let Some(d) = self.docs.get(&s.document_id) else {
                    return ValueType::Unknown;
                };
                let Some(t) = d
                    .rust_semantics
                    .fields
                    .get(&format!("{}::{field}", s.qualified_name))
                else {
                    return ValueType::Unknown;
                };
                let substitutions: HashMap<_, _> = d
                    .rust_semantics
                    .trait_data
                    .type_parameters
                    .get(&s.id)
                    .into_iter()
                    .flatten()
                    .cloned()
                    .zip(arguments)
                    .collect();
                self.types(
                    d,
                    s.start_byte,
                    &s.qualified_name,
                    t,
                    &substitutions,
                    depth + 1,
                )
            }
            Expr::Call(callee, type_args) => {
                let found = self.callees(doc, at, owner, callee, depth + 1);
                if found.len() != 1 {
                    return ValueType::Unknown;
                }
                let function = found[0];
                let Some(d) = self.docs.get(&function.document_id) else {
                    return ValueType::Unknown;
                };
                let Some(f) = d.rust_semantics.functions.get(&function.id) else {
                    return ValueType::Unknown;
                };
                let mut substitution: HashMap<_, _> = f
                    .generics
                    .iter()
                    .map(|g| (g.clone(), ValueType::Unknown))
                    .collect();
                if let Expr::Member(receiver, _) = callee.as_ref() {
                    let value = self.value(doc, at, owner, receiver, depth + 1);
                    substitution.extend(self.method_substitutions(function, &value, depth + 1));
                } else if let Expr::Ufcs(t, _, _) = callee.as_ref() {
                    let value = self.types(doc, at, owner, t, &HashMap::new(), depth + 1);
                    substitution.extend(self.method_substitutions(function, &value, depth + 1));
                }
                if !type_args.is_empty() {
                    if f.generics.len() != type_args.len() {
                        return ValueType::Unknown;
                    }
                    for (name, t) in f.generics.iter().zip(type_args) {
                        substitution.insert(
                            name.clone(),
                            self.types(doc, at, owner, t, &HashMap::new(), depth + 1),
                        );
                    }
                }
                let result = self.types(
                    d,
                    function.start_byte,
                    &function.qualified_name,
                    &f.result,
                    &substitution,
                    depth + 1,
                );
                if f.asynchronous {
                    ValueType::Future(Box::new(result))
                } else {
                    result
                }
            }
            _ => ValueType::Unknown,
        }
    }
    fn callees(
        &mut self,
        doc: &IndexedDocument,
        at: usize,
        owner: &str,
        e: &Expr,
        depth: usize,
    ) -> Vec<&'a Symbol> {
        if depth > 32 || !self.step() {
            return vec![];
        }
        match e {
            Expr::Name(name) => {
                if let Some((t, method)) = name.rsplit_once("::") {
                    let value = self.types(
                        doc,
                        at,
                        owner,
                        &Ty::Named(t.into(), vec![]),
                        &HashMap::new(),
                        depth + 1,
                    );
                    if !matches!(value, ValueType::Unknown) {
                        return self.trait_methods(
                            &value,
                            method,
                            (doc, at, owner),
                            None,
                            false,
                            depth + 1,
                        );
                    }
                    if t == "Self"
                        || doc.rust_semantics.functions.iter().any(|(id, f)| {
                            self.symbol(id).is_some_and(|s| s.qualified_name == owner)
                                && f.generics
                                    .iter()
                                    .chain(&f.enclosing_generics)
                                    .any(|p| p == t)
                        })
                    {
                        return vec![];
                    }
                }
                self.names(doc, at, owner, name, true)
            }
            Expr::Ufcs(t, tr, name) => {
                let value = self.types(doc, at, owner, t, &HashMap::new(), depth + 1);
                let ValueType::Nominal(trait_id, _) =
                    self.types(doc, at, owner, tr, &HashMap::new(), depth + 1)
                else {
                    return vec![];
                };
                if self
                    .symbol(&trait_id)
                    .is_none_or(|s| s.kind != "trait_item")
                {
                    return vec![];
                }
                self.trait_methods(
                    &value,
                    name,
                    (doc, at, owner),
                    Some(&trait_id),
                    false,
                    depth + 1,
                )
            }
            Expr::Member(value, name) => {
                let t = self.value(doc, at, owner, value, depth + 1);
                self.methods(&t, name, (doc, at, owner), 0)
            }
            _ => vec![],
        }
    }
}

pub(super) fn resolve(
    symbols: &[Symbol],
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
    r: &Reference,
) -> Option<CallEdge> {
    if !doc.path.ends_with(".rs") {
        return None;
    }
    if doc
        .rust_semantics
        .macro_ranges
        .iter()
        .any(|(start, end)| *start <= r.start_byte && r.start_byte < *end)
    {
        return Some(CallEdge {
            reference: r.clone(),
            candidates: vec![],
            resolution: "unresolved".into(),
            provenance: "derived",
            resolution_basis: "rust_macro_expansion_unverified".into(),
        });
    }
    if !r.is_call {
        return None;
    }
    let expression = doc
        .rust_semantics
        .expressions
        .get(&format!("{}:{}", r.start_byte, r.expression))?;
    let Expr::Call(callee, _) = expression else {
        return None;
    };
    // Keep established bare/qualified binding behavior; own all member call forms
    // so failed expression typing cannot fall back to unrelated lexical methods.
    let caller = r
        .caller
        .as_ref()
        .and_then(|id| symbols.iter().find(|s| &s.id == id));
    let mut engine = Engine {
        symbols,
        docs,
        budget: 1024,
    };
    if !matches!(callee.as_ref(), Expr::Member(..) | Expr::Ufcs(..)) {
        let Expr::Name(name) = callee.as_ref() else {
            return None;
        };
        let (head, _) = name.rsplit_once("::")?;
        let owner = caller.map(|s| s.qualified_name.as_str()).unwrap_or("");
        let is_parameter = doc.rust_semantics.functions.iter().any(|(id, f)| {
            caller.is_some_and(|s| s.id == *id)
                && f.generics
                    .iter()
                    .chain(&f.enclosing_generics)
                    .any(|p| p == head)
        });
        if !is_parameter
            && head != "Self"
            && matches!(
                engine.types(
                    doc,
                    r.start_byte,
                    owner,
                    &Ty::Named(head.into(), vec![]),
                    &HashMap::new(),
                    0
                ),
                ValueType::Unknown
            )
        {
            return None;
        }
    }
    let mut found = engine.callees(
        doc,
        r.start_byte,
        caller.map(|s| s.qualified_name.as_str()).unwrap_or(""),
        callee,
        0,
    );
    found.retain(|s| {
        docs.get(&s.document_id)
            .is_some_and(|d| d.project == doc.project)
    });
    found.sort_by_key(|s| &s.id);
    found.dedup_by_key(|s| &s.id);
    if engine.budget == 0 {
        found.clear();
    }
    let declaration_target = found.iter().any(|method| {
        symbols.iter().any(|s| {
            s.document_id == method.document_id
                && s.kind == "trait_item"
                && s.start_byte <= method.start_byte
                && method.end_byte <= s.end_byte
        })
    });
    Some(CallEdge {
        reference: r.clone(),
        candidates: found.iter().map(|s| s.id.clone()).collect(),
        resolution: match found.len() {
            0 => "unresolved",
            1 => "resolved_static",
            _ => "ambiguous",
        }
        .into(),
        provenance: "derived",
        resolution_basis: if engine.budget == 0 {
            "rust_expression_budget_exhausted"
        } else if declaration_target {
            "rust_trait_declaration"
        } else {
            "rust_ast_receiver_type"
        }
        .into(),
    })
}

#[cfg(test)]
mod tests;
