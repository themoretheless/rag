//! C3 method lookup over explicit syntax inheritance; unknown bases fail closed.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct BaseSpec {
    pub derived: String,
    pub base: String,
    pub line: usize,
    pub start: usize,
    pub relation: String,
}
#[derive(Debug, Serialize)]
pub struct InheritanceEdge {
    pub derived: String,
    pub candidates: Vec<String>,
    pub base_expression: String,
    pub path: String,
    pub line: usize,
    pub relation: String,
    pub resolution: String,
    pub hierarchy_status: String,
    pub provenance: &'static str,
}
fn qualified(node: Node, source: &str) -> String {
    let mut parts = Vec::new();
    let mut cur = Some(node);
    while let Some(n) = cur {
        if definition(n) {
            if let Some(name) = n.child_by_field_name("name") {
                parts.push(text(name, source));
            }
        }
        cur = n.parent();
    }
    parts.reverse();
    parts.join("::")
}
fn simple(node: Node, source: &str) -> Option<String> {
    if matches!(
        node.kind(),
        "identifier"
            | "type_identifier"
            | "attribute"
            | "member_expression"
            | "nested_type_identifier"
            | "scoped_type_identifier"
            | "generic_type"
    ) {
        let s = text(node, source);
        if !s.contains(['(', ')', '[', ']', '<', '>']) {
            return Some(s.into());
        }
    }
    None
}
pub(super) fn collect(node: Node, source: &str, out: &mut IndexedDocument) {
    let derived = qualified(node, source);
    if matches!(
        node.kind(),
        "class_definition" | "class_declaration" | "interface_declaration"
    ) {
        if let Some(body) = node.child_by_field_name("body") {
            let mut cursor = body.walk();
            for child in body.named_children(&mut cursor) {
                let candidate = if child.kind() == "expression_statement" {
                    child.named_child(0).unwrap_or(child)
                } else {
                    child
                };
                let field = if candidate.kind() == "assignment" {
                    candidate.child_by_field_name("left")
                } else if matches!(
                    candidate.kind(),
                    "field_definition" | "public_field_definition" | "property_signature"
                ) {
                    candidate
                        .child_by_field_name("name")
                        .or_else(|| candidate.child_by_field_name("property"))
                } else {
                    None
                };
                if let Some(field) = field {
                    out.blocked_members
                        .insert(format!("{derived}::{}", text(field, source)));
                }
                if candidate.kind() == "decorated_definition"
                    && text(candidate, source)
                        .lines()
                        .any(|l| matches!(l.trim(), "@property" | "@cached_property"))
                {
                    if let Some(name) = candidate
                        .child_by_field_name("definition")
                        .and_then(|d| d.child_by_field_name("name"))
                    {
                        out.blocked_members
                            .insert(format!("{derived}::{}", text(name, source)));
                    }
                }
            }
        }
        let mut bases = Vec::new();
        if let Some(args) = node.child_by_field_name("superclasses") {
            let mut cursor = args.walk();
            for n in args.named_children(&mut cursor) {
                // metaclass= is not a base class.
                if n.kind() != "keyword_argument" {
                    bases.push((n, simple(n, source)));
                }
            }
        } else {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if matches!(child.kind(), "class_heritage" | "extends_type_clause") {
                    let mut cursor = child.walk();
                    for n in child.named_children(&mut cursor) {
                        if n.kind() == "implements_clause" {
                            continue;
                        }
                        if n.kind() == "extends_clause" {
                            let mut cursor = n.walk();
                            for value in n.named_children(&mut cursor) {
                                if value.kind() != "type_arguments" {
                                    bases.push((value, simple(value, source)));
                                }
                            }
                        } else {
                            bases.push((n, simple(n, source)));
                        }
                    }
                }
            }
        }
        for (n, base) in bases {
            out.bases.push(BaseSpec {
                derived: derived.clone(),
                base: base.unwrap_or_else(|| format!("?{}", text(n, source))),
                line: n.start_position().row + 1,
                start: node.start_byte(),
                relation: "extends".into(),
            });
        }
    }
    if node.kind() == "impl_item" {
        if let (Some(ty), Some(tr)) = (
            node.child_by_field_name("type"),
            node.child_by_field_name("trait"),
        ) {
            out.bases.push(BaseSpec {
                derived: text(ty, source).into(),
                base: text(tr, source).into(),
                line: tr.start_position().row + 1,
                start: node.start_byte(),
                relation: "implements".into(),
            });
        }
    }
}
fn is_type(s: &Symbol) -> bool {
    matches!(
        s.kind.as_str(),
        "class_definition"
            | "class_declaration"
            | "interface_declaration"
            | "struct_item"
            | "enum_item"
            | "trait_item"
    )
}
pub(super) fn type_symbols<'a>(
    symbols: &'a [Symbol],
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
    name: &str,
    at: usize,
    owner: &str,
) -> Vec<&'a Symbol> {
    let name = name.replace('.', "::");
    let (head, tail) = name.split_once("::").unwrap_or((&name, ""));
    let mut found = if let Some(import) = resolve::import_target(doc, head, at) {
        resolve::imported_type(
            symbols,
            docs,
            &doc.path,
            import,
            if tail.is_empty() { None } else { Some(tail) },
        )
    } else {
        let mut prefix = owner.rsplit_once("::").map(|p| p.0).unwrap_or("");
        let mut result = Vec::new();
        loop {
            let q = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}::{name}")
            };
            result.extend(
                symbols
                    .iter()
                    .filter(|s| s.path == doc.path && s.qualified_name == q && is_type(s)),
            );
            if !result.is_empty() || prefix.is_empty() {
                break;
            }
            prefix = prefix.rsplit_once("::").map(|p| p.0).unwrap_or("");
        }
        if result.is_empty() && name.contains("::") {
            result = resolve::imported_type(symbols, docs, &doc.path, &name, None);
        }
        result
    };
    found.retain(|s| {
        is_type(s)
            && docs
                .get(&s.document_id)
                .is_some_and(|d| d.project == doc.project)
    });
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found.dedup_by(|a, b| a.id == b.id);
    found
}
pub(super) fn edges(
    symbols: &[Symbol],
    docs: &HashMap<String, IndexedDocument>,
) -> Vec<InheritanceEdge> {
    let mut result = Vec::new();
    for doc in docs.values() {
        if doc.c_family.is_some() {
            result.extend(super::c_family::edges(symbols, docs, doc));
            continue;
        }
        if doc.java.is_some() {
            result.extend(super::java::edges(symbols, docs, doc));
            continue;
        }
        for spec in &doc.bases {
            for derived in symbols
                .iter()
                .filter(|s| s.path == doc.path && s.qualified_name == spec.derived && is_type(s))
            {
                let targets =
                    type_symbols(symbols, docs, doc, &spec.base, spec.start, &spec.derived);
                result.push(InheritanceEdge {
                    derived: derived.id.clone(),
                    candidates: targets.iter().map(|s| s.id.clone()).collect(),
                    base_expression: spec.base.clone(),
                    path: doc.path.clone(),
                    line: spec.line,
                    relation: spec.relation.clone(),
                    resolution: match targets.len() {
                        0 => "unresolved",
                        1 => "resolved_static",
                        _ => "ambiguous",
                    }
                    .into(),
                    hierarchy_status: if mro(
                        &derived.id,
                        symbols,
                        docs,
                        &mut HashSet::new(),
                        &mut 4096,
                    )
                    .is_some()
                    {
                        "complete_static"
                    } else {
                        "incomplete_or_invalid"
                    }
                    .into(),
                    provenance: "derived",
                });
            }
        }
    }
    result.sort_by(|a, b| (&a.path, a.line, &a.derived).cmp(&(&b.path, b.line, &b.derived)));
    result
}
fn mro<'a>(
    id: &str,
    symbols: &'a [Symbol],
    docs: &HashMap<String, IndexedDocument>,
    stack: &mut HashSet<String>,
    budget: &mut usize,
) -> Option<Vec<&'a Symbol>> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    if stack.len() >= 64 || !stack.insert(id.into()) {
        return None;
    }
    let class = symbols.iter().find(|s| s.id == id)?;
    let doc = docs.get(&class.document_id)?;
    let mut parents = Vec::new();
    for spec in doc
        .bases
        .iter()
        .filter(|b| b.derived == class.qualified_name)
    {
        let matches = type_symbols(symbols, docs, doc, &spec.base, spec.start, &spec.derived);
        if matches.is_empty() && doc.path.ends_with(".py") && spec.base == "object" {
            continue;
        }
        if matches.len() != 1 {
            return None;
        }
        // Rust has trait defaults, not class inheritance or C3 ordering between traits.
        if spec.relation == "implements" && parents.len() > 0 {
            return None;
        }
        parents.push(matches[0]);
    }
    let mut sequences = Vec::new();
    for p in &parents {
        sequences.push(mro(&p.id, symbols, docs, stack, budget)?);
    }
    sequences.push(parents);
    let mut result = vec![class];
    while sequences.iter().any(|s| !s.is_empty()) {
        let next = sequences
            .iter()
            .filter_map(|s| s.first().copied())
            .find(|candidate| {
                !sequences
                    .iter()
                    .any(|s| s.iter().skip(1).any(|s| s.id == candidate.id))
            })?;
        result.push(next);
        for sequence in &mut sequences {
            if sequence.first().is_some_and(|s| s.id == next.id) {
                sequence.remove(0);
            }
        }
        if result.len() > 256 {
            return None;
        }
    }
    stack.remove(id);
    Some(result)
}
pub(super) fn methods<'a>(
    symbols: &'a [Symbol],
    docs: &HashMap<String, IndexedDocument>,
    path: &str,
    qualified: &str,
    skip_self: bool,
) -> Vec<&'a Symbol> {
    let Some((owner, name)) = qualified.rsplit_once("::") else {
        return vec![];
    };
    let types: Vec<_> = symbols
        .iter()
        .filter(|s| s.path == path && s.qualified_name == owner && is_type(s))
        .collect();
    let mut found = Vec::new();
    for ty in types {
        // An override can be used even if a remote ancestor is unavailable.
        if !skip_self {
            if docs
                .get(&ty.document_id)
                .is_some_and(|d| d.blocked_members.contains(qualified))
            {
                continue;
            }
            let own = resolve::matching(symbols, path, qualified);
            if !own.is_empty() {
                found.extend(own);
                continue;
            }
        }
        let Some(order) = mro(&ty.id, symbols, docs, &mut HashSet::new(), &mut 4096) else {
            continue;
        };
        for base in order.iter().skip(1) {
            if docs.get(&base.document_id).is_some_and(|d| {
                d.blocked_members
                    .contains(&format!("{}::{name}", base.qualified_name))
            }) {
                break;
            }
            let methods: Vec<_> = resolve::matching(
                symbols,
                &base.path,
                &format!("{}::{name}", base.qualified_name),
            )
            .into_iter()
            .filter(|s| s.kind != "function_signature_item")
            .collect();
            if !methods.is_empty() {
                found.extend(methods);
                break;
            }
        }
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found.dedup_by(|a, b| a.id == b.id);
    found
}
pub(super) fn lookup<'a>(
    symbols: &'a [Symbol],
    docs: &HashMap<String, IndexedDocument>,
    path: &str,
    name: &str,
) -> Vec<&'a Symbol> {
    if docs
        .values()
        .any(|d| d.path == path && d.blocked_members.contains(name))
    {
        return vec![];
    }
    let own = resolve::matching(symbols, path, name);
    if own.is_empty() {
        methods(symbols, docs, path, name, false)
    } else {
        own
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(files: &[(&str, &str)]) -> CodeSnapshot {
        let mut documents = HashMap::new();
        let mut symbols = Vec::new();
        for (i, (path, source)) in files.iter().enumerate() {
            let doc = parse(&i.to_string(), path, source, 1).unwrap();
            assert!(!doc.parse_errors, "{path}");
            symbols.extend(doc.symbols.clone());
            documents.insert(i.to_string(), doc);
        }
        CodeSnapshot {
            calls: resolve::calls(&symbols, &documents),
            inheritance: edges(&symbols, &documents),
            symbols,
            ..Default::default()
        }
    }
    #[test]
    fn inherited_methods_imported_bases_and_overrides() {
        for (path,main,base_path,base) in [
            ("/p/main.py","from base import Base as Parent\nclass Child(Parent):\n    def run(self):\n        self.work()\n", "/p/base.py","class Base:\n    def work(self):\n        pass\n"),
            ("/p/main.ts","import {Base as Parent} from './base'; class Child extends Parent { run() { this.work(); } }", "/p/base.ts","export class Base { work() {} }"),
            ("/p/main.js","import Parent from './base'; class Child extends Parent { run() { this.work(); } }", "/p/base.js","export default class Base { work() {} }"),
        ] {
            let snap=snapshot(&[(path,main),(base_path,base)]);
            let call=snap.calls.iter().find(|c|c.reference.name=="work").unwrap();
            assert_eq!(call.resolution,"resolved_static","{path}: {call:?}");
            let target=snap.symbols.iter().find(|s|call.candidates.contains(&s.id)).unwrap();assert_eq!(target.qualified_name,"Base::work");
            assert_eq!(snap.inheritance[0].resolution,"resolved_static");
        }
        let snap=snapshot(&[("/p/a.ts","class Base { work() {} } class Child extends Base { work() {} run(){ this.work(); super.work(); } }")]);
        let targets: Vec<_> = snap
            .calls
            .iter()
            .filter(|c| c.reference.name == "work")
            .map(|c| {
                snap.symbols
                    .iter()
                    .find(|s| c.candidates.contains(&s.id))
                    .unwrap()
                    .qualified_name
                    .as_str()
            })
            .collect();
        assert_eq!(targets, vec!["Child::work", "Base::work"]);
    }
    #[test]
    fn c3_diamond_order_cycles_and_unknown_bases() {
        let snap=snapshot(&[("/p/a.py","class A:\n    def work(self):\n        pass\nclass B(A):\n    pass\nclass C(A):\n    def work(self):\n        pass\nclass D(B,C):\n    def run(self):\n        self.work()\n")]);
        let call = snap
            .calls
            .iter()
            .find(|c| c.reference.name == "work")
            .unwrap();
        let target = snap
            .symbols
            .iter()
            .find(|s| call.candidates.contains(&s.id))
            .unwrap();
        assert_eq!(target.qualified_name, "C::work");
        for source in [
            "class A(B):\n    pass\nclass B(A):\n    def run(self):\n        self.work()\n",
            "class A(Unknown):\n    def run(self):\n        self.work()\n",
            "class A(factory()):\n    def run(self):\n        self.work()\n",
        ] {
            let snap = snapshot(&[("/p/a.py", source)]);
            let call = snap
                .calls
                .iter()
                .find(|c| c.reference.name == "work")
                .unwrap();
            assert!(call.candidates.is_empty());
        }
    }
    #[test]
    fn python_super_skips_override_and_rust_trait_defaults() {
        let snap=snapshot(&[("/p/a.py","class Base(object):\n    def work(self):\n        pass\nclass Child(Base):\n    def work(self):\n        super().work()\n")]);
        let call = snap
            .calls
            .iter()
            .find(|c| c.reference.name == "work")
            .unwrap();
        assert_eq!(call.resolution_basis, "super_method");
        assert!(snap
            .symbols
            .iter()
            .any(|s| s.qualified_name == "Base::work" && call.candidates.contains(&s.id)));
        let snap=snapshot(&[("/p/src/main.rs","trait Work { fn work(&self) {} } struct Worker; impl Work for Worker {} fn run(w: Worker) { w.work(); }")]);
        let call = snap
            .calls
            .iter()
            .find(|c| c.reference.name == "work")
            .unwrap();
        assert_eq!(call.resolution, "resolved_static", "{call:?}");
        assert_eq!(snap.inheritance[0].relation, "implements");
    }
}

#[cfg(test)]
mod override_tests {
    use super::*;
    #[test]
    fn fields_and_properties_do_not_fall_through_to_base_methods() {
        for (path,source) in [
            ("/p/a.py","class Base:\n    def work(self):\n        pass\nclass Child(Base):\n    work = None\n    def run(self):\n        self.work()\n"),
            ("/p/a.ts","class Base { work() {} } class Child extends Base { work = null; run() { this.work(); } }"),
            ("/p/a.py","class Base:\n    def work(self):\n        pass\nclass Child(Base):\n    @property\n    def work(self):\n        return None\n    def run(self):\n        self.work()\n"),
        ] {
            let doc=parse("d",path,source,1).unwrap();assert!(!doc.parse_errors);
            let docs=HashMap::from([("d".into(),doc.clone())]);
            let call=resolve::calls(&doc.symbols,&docs).into_iter().find(|c|c.reference.name=="work").unwrap();assert!(call.candidates.is_empty(),"{path}: {call:?}");
        }
    }
    #[test]
    fn malformed_nested_base_paths_and_inconsistent_mro_fail_closed() {
        for source in [
            "class A(A.Missing):\n    def run(self):\n        self.work()\n",
            "class X:\n    pass\nclass Y:\n    pass\nclass A(X,Y):\n    def work(self):\n        pass\nclass B(Y,X):\n    pass\nclass C(A,B):\n    def run(self):\n        self.work()\n",
        ] {
            let doc=parse("d","/p/a.py",source,1).unwrap();let docs=HashMap::from([("d".into(),doc.clone())]);
            let call=resolve::calls(&doc.symbols,&docs).into_iter().find(|c|c.reference.name=="work").unwrap();assert!(call.candidates.is_empty(),"{call:?}");
        }
    }
}
