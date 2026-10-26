//! Captured module imports and bounded Cargo-local re-export lookup.
use super::*;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Facts {
    complete: bool,
    imports: Vec<Import>,
    items: BTreeMap<usize, Item>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Item {
    scope: Vec<String>,
    visibility: String,
    #[serde(default)]
    namespace_item: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Import {
    scope: Vec<String>,
    visibility: String,
    local: Option<String>,
    target: String,
    at: usize,
    known: bool,
    opaque: bool,
}

fn children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}
fn visibility(node: Node, source: &str) -> String {
    children(node)
        .into_iter()
        .find(|n| n.kind() == "visibility_modifier")
        .map(|n| {
            text(n, source)
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect()
        })
        .unwrap_or_default()
}
fn path(raw: &str) -> Option<String> {
    let parts: Vec<_> = raw.trim().split("::").map(str::trim).collect();
    if parts.is_empty()
        || parts.iter().any(|p| {
            let p = p.strip_prefix("r#").unwrap_or(p);
            p.is_empty()
                || !p
                    .chars()
                    .enumerate()
                    .all(|(i, c)| c == '_' || c.is_alphabetic() || (i > 0 && c.is_numeric()))
        })
    {
        return None;
    }
    Some(parts.join("::"))
}
fn use_items(
    node: Node,
    source: &str,
    prefix: &str,
    depth: usize,
    out: &mut Vec<(Option<String>, String)>,
) -> Option<()> {
    if depth > 32 || out.len() >= 2048 {
        return None;
    }
    match node.kind() {
        "use_list" => {
            for child in children(node)
                .into_iter()
                .filter(|n| !n.kind().ends_with("comment"))
            {
                use_items(child, source, prefix, depth + 1, out)?;
            }
        }
        "scoped_use_list" => {
            let head = path(text(node.child_by_field_name("path")?, source))?;
            let prefix = format!("{prefix}{head}::");
            use_items(
                node.child_by_field_name("list")?,
                source,
                &prefix,
                depth + 1,
                out,
            )?;
        }
        "use_as_clause" => {
            let raw = path(text(node.child_by_field_name("path")?, source))?;
            let alias = path(text(node.child_by_field_name("alias")?, source))?;
            if alias.contains("::") {
                return None;
            }
            let target = if raw == "self" && !prefix.is_empty() {
                prefix.trim_end_matches("::").into()
            } else {
                format!("{prefix}{raw}")
            };
            out.push((Some(alias), target));
        }
        "use_wildcard" => {
            let raw = text(node, source).trim();
            let raw = raw.strip_suffix('*')?.trim().trim_end_matches("::");
            let target = if raw.is_empty() {
                prefix.trim_end_matches("::").into()
            } else {
                format!("{prefix}{}", path(raw)?)
            };
            if target.is_empty() {
                return None;
            }
            out.push((None, target));
        }
        "identifier" | "scoped_identifier" | "self" => {
            let raw = path(text(node, source))?;
            let target = if raw == "self" && !prefix.is_empty() {
                prefix.trim_end_matches("::").into()
            } else {
                format!("{prefix}{raw}")
            };
            out.push((Some(target.rsplit("::").next()?.into()), target));
        }
        _ => return None,
    }
    Some(())
}
pub(super) fn collect(root: Node, source: &str, doc: &mut IndexedDocument) {
    let mut facts = Facts {
        complete: true,
        ..Default::default()
    };
    let starts: HashMap<_, _> = doc
        .symbols
        .iter()
        .map(|s| (s.start_byte, s.kind.as_str()))
        .collect();
    let mut stack = vec![(root, Vec::<String>::new(), false, false, false)];
    let mut work = 0;
    while let Some((node, scope, local, in_trait, associated)) = stack.pop() {
        work += 1;
        if work > 200_000 || scope.len() > 64 {
            facts.complete = false;
            break;
        }
        if !local
            && starts
                .get(&node.start_byte())
                .is_some_and(|kind| *kind == node.kind())
        {
            let mut vis = visibility(node, source);
            if in_trait && matches!(node.kind(), "function_item" | "function_signature_item") {
                vis = "pub".into();
            }
            facts.items.insert(
                node.start_byte(),
                Item {
                    scope: scope.clone(),
                    visibility: vis,
                    namespace_item: !associated,
                },
            );
        }
        if node.kind() == "use_declaration" && !local {
            let mut entries = vec![];
            let parsed = node
                .child_by_field_name("argument")
                .and_then(|n| use_items(n, source, "", 0, &mut entries));
            if parsed.is_none() || facts.imports.len() + entries.len() > 2048 {
                facts.complete = false;
                continue;
            }
            let mut conditional = false;
            let mut opaque = false;
            let mut previous = node.prev_named_sibling();
            while let Some(attr) = previous {
                if attr.kind().ends_with("comment") {
                    previous = attr.prev_named_sibling();
                    continue;
                }
                if attr.kind() != "attribute_item" {
                    break;
                }
                let raw = text(attr, source)
                    .trim()
                    .trim_start_matches("#[")
                    .trim_end_matches(']');
                let head = raw.split(['(', '=']).next().unwrap_or("").trim();
                if matches!(head, "cfg" | "cfg_attr") {
                    conditional = true;
                } else if !matches!(
                    head,
                    "doc" | "allow" | "warn" | "deny" | "forbid" | "expect"
                ) {
                    opaque = true;
                }
                previous = attr.prev_named_sibling();
            }
            facts
                .imports
                .extend(entries.into_iter().map(|(local, target)| Import {
                    scope: scope.clone(),
                    visibility: visibility(node, source),
                    local,
                    target,
                    at: node.start_byte(),
                    known: !conditional && !opaque,
                    opaque,
                }));
        }
        let local = local
            || matches!(
                node.kind(),
                "function_item" | "closure_expression" | "macro_definition" | "macro_invocation"
            );
        let in_trait = in_trait || node.kind() == "trait_item";
        let associated = associated || matches!(node.kind(), "impl_item" | "trait_item");
        for child in children(node).into_iter().rev() {
            let mut next = scope.clone();
            if node.kind() == "mod_item" && child.kind() == "declaration_list" {
                if let Some(name) = node.child_by_field_name("name") {
                    next.push(text(name, source).into());
                }
            }
            stack.push((child, next, local, in_trait, associated));
        }
    }
    doc.rust_reexports = facts;
}
pub(super) fn configure(doc: &mut IndexedDocument, decision: impl Fn(usize) -> Option<bool>) {
    doc.rust_reexports
        .imports
        .retain_mut(|i| match decision(i.at) {
            Some(false) => false,
            state => {
                i.known = state == Some(true) && !i.opaque;
                true
            }
        });
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
enum Region {
    Public,
    Within(Vec<String>),
}
impl Region {
    fn allows(&self, scope: &[String], same_crate: bool) -> bool {
        match self {
            Self::Public => true,
            Self::Within(at) => same_crate && scope.starts_with(at),
        }
    }
    fn covers(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Public, _) => true,
            (Self::Within(a), Self::Within(b)) => b.starts_with(a),
            _ => false,
        }
    }
}
fn region(raw: &str, scope: &[String]) -> Option<Region> {
    Some(match raw {
        "pub" => Region::Public,
        "pub(crate)" => Region::Within(vec![]),
        "" | "pub(self)" => Region::Within(scope.to_vec()),
        "pub(super)" => {
            let mut p = scope.to_vec();
            p.pop()?;
            Region::Within(p)
        }
        _ => {
            let path = raw.strip_prefix("pub(in")?.strip_suffix(')')?;
            let p = absolute(path, scope)?;
            if !scope.starts_with(&p) {
                return None;
            }
            Region::Within(p)
        }
    })
}
fn absolute(target: &str, scope: &[String]) -> Option<Vec<String>> {
    let mut parts = target.split("::");
    let first = parts.next()?;
    let mut out = match first {
        "crate" => vec![],
        "self" => scope.to_vec(),
        "super" => {
            let mut p = scope.to_vec();
            p.pop()?;
            p
        }
        _ => return None,
    };
    for part in parts {
        match part {
            "super" => {
                out.pop()?;
            }
            "self" | "crate" => return None,
            _ => out.push(part.into()),
        }
    }
    Some(out)
}

type ActiveKey = (Vec<String>, Vec<String>, bool, Option<Region>, usize);
struct Context {
    requester: Vec<String>,
    same_crate: bool,
    floor: Option<Region>,
    base: usize,
    depth: usize,
    touched: bool,
}
struct Lookup<'a, 'context> {
    symbols: &'a [Symbol],
    docs: &'context HashMap<String, IndexedDocument>,
    map: &'context BTreeMap<Vec<String>, Location>,
    project: &'context Option<String>,
    methods: bool,
    budget: usize,
    active: HashSet<ActiveKey>,
}
impl<'a, 'context> Lookup<'a, 'context> {
    fn direct(&self, path: &[String]) -> Vec<&'a Symbol> {
        for split in (0..path.len()).rev() {
            if let Some(loc) = self.map.get(&path[..split]) {
                let name = loc
                    .prefix
                    .iter()
                    .chain(&path[split..])
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("::");
                return if self.methods {
                    inheritance::lookup(self.symbols, self.docs, &loc.file, &name)
                } else {
                    resolve::matching(self.symbols, &loc.file, &name)
                }
                .into_iter()
                .filter(|s| {
                    self.docs
                        .get(&s.document_id)
                        .is_some_and(|d| d.project == *self.project)
                })
                .collect();
            }
        }
        vec![]
    }
    fn item_region(&self, symbol: &Symbol) -> Option<Region> {
        let doc = self.docs.get(&symbol.document_id)?;
        let item = doc.rust_reexports.items.get(&symbol.start_byte)?;
        let (base, _) = self
            .map
            .iter()
            .find(|(_, loc)| loc.file == symbol.path && loc.prefix.is_empty())?;
        let scope = base.iter().chain(&item.scope).cloned().collect::<Vec<_>>();
        region(&item.visibility, &scope)
    }
    fn namespace_item(&self, symbol: &Symbol) -> bool {
        self.docs
            .get(&symbol.document_id)
            .and_then(|d| d.rust_reexports.items.get(&symbol.start_byte))
            .is_some_and(|i| i.namespace_item)
    }
    fn modules_visible(
        &self,
        namespace: &[String],
        requester: &[String],
        same: bool,
        floor: Option<&Region>,
        base: usize,
    ) -> bool {
        (1..=namespace.len()).all(|n| {
            let symbols = self.direct(&namespace[..n]);
            symbols.len() == 1
                && self.item_region(symbols[0]).is_some_and(|r| {
                    r.allows(requester, same)
                        && (base == 0 || n < base || floor.is_none_or(|f| r.covers(f)))
                })
        })
    }
    fn walk(
        &mut self,
        path: Vec<String>,
        context: Context,
    ) -> std::result::Result<(bool, Vec<&'a Symbol>), ()> {
        if context.depth > 16 || self.budget == 0 {
            return Err(());
        }
        self.budget -= 1;
        let key = (
            path.clone(),
            context.requester.clone(),
            context.same_crate,
            context.floor.clone(),
            context.base,
        );
        if !self.active.insert(key.clone()) {
            return Ok((true, vec![]));
        }
        let result = self.walk_inner(&path, &context);
        self.active.remove(&key);
        result
    }
    fn walk_inner(
        &mut self,
        path: &[String],
        context: &Context,
    ) -> std::result::Result<(bool, Vec<&'a Symbol>), ()> {
        let requester = &context.requester;
        let same = context.same_crate;
        let floor = context.floor.as_ref();
        let base = context.base;
        let depth = context.depth;
        let touched = context.touched;
        let Some((split, loc)) = (0..path.len())
            .rev()
            .find_map(|s| self.map.get(&path[..s]).map(|l| (s, l.clone())))
        else {
            return Ok((touched, vec![]));
        };
        let doc = at(self.docs, self.project, &loc.file).ok_or(())?;
        if !doc.rust_reexports.complete {
            return Err(());
        }
        let explicit: Vec<_> = doc
            .rust_reexports
            .imports
            .iter()
            .filter(|i| i.scope == loc.prefix && i.local.as_deref() == Some(path[split].as_str()))
            .cloned()
            .collect();
        let direct = self.direct(path);
        let imports: Vec<_> = if !explicit.is_empty() {
            explicit
        } else if direct.is_empty() {
            doc.rust_reexports
                .imports
                .iter()
                .filter(|i| i.scope == loc.prefix && i.local.is_none())
                .cloned()
                .collect()
        } else {
            vec![]
        };
        let touched = touched || !imports.is_empty();
        if !touched {
            return Ok((false, vec![]));
        }
        if !self.modules_visible(&path[..split], requester, same, floor, base) {
            return Ok((true, vec![]));
        }
        // A module/type alias must not expose a private base merely because one
        // of its children happens to be public.
        if base > 0 && base < path.len() {
            let bases = self.direct(&path[..base]);
            if !bases.is_empty()
                && !bases.iter().all(|s| {
                    self.namespace_item(s)
                        && self.item_region(s).is_some_and(|r| {
                            r.allows(requester, same) && floor.is_none_or(|f| r.covers(f))
                        })
                })
            {
                return Ok((true, vec![]));
            }
        }
        if imports.is_empty() {
            return Ok((
                true,
                direct
                    .into_iter()
                    .filter(|s| {
                        (base == 0 || base != path.len() || self.namespace_item(s))
                            && self.item_region(s).is_some_and(|r| {
                                r.allows(requester, same) && floor.is_none_or(|f| r.covers(f))
                            })
                    })
                    .collect(),
            ));
        }
        let named = imports[0].local.is_some();
        if named && (imports.len() != 1 || !direct.is_empty()) {
            return Err(());
        }
        let namespace = &path[..split];
        let mut found = vec![];
        for import in imports {
            if !import.known {
                return Err(());
            }
            let visibility = region(&import.visibility, namespace).ok_or(())?;
            if !visibility.allows(requester, same) || floor.is_some_and(|f| !visibility.covers(f)) {
                continue;
            }
            let mut target = absolute(&import.target, namespace).ok_or(())?;
            // Module glob imports are supported. An associated method namespace
            // must not masquerade as an importable module through `Type::*`.
            if !named && !self.map.contains_key(&target) {
                return Err(());
            }
            let base = target.len() + usize::from(!named);
            target.extend_from_slice(if named {
                &path[split + 1..]
            } else {
                &path[split..]
            });
            let (_, candidates) = self.walk(
                target,
                Context {
                    requester: namespace.to_vec(),
                    same_crate: true,
                    floor: Some(visibility),
                    base,
                    depth: depth + 1,
                    touched: true,
                },
            )?;
            found.extend(candidates);
        }
        found.sort_by(|a, b| a.id.cmp(&b.id));
        found.dedup_by(|a, b| a.id == b.id);
        Ok((true, found))
    }
}
pub(super) fn lookup<'a>(
    symbols: &'a [Symbol],
    docs: &HashMap<String, IndexedDocument>,
    map: &BTreeMap<Vec<String>, Location>,
    origin: &BTreeMap<Vec<String>, Location>,
    source: &IndexedDocument,
    path: &[String],
    methods: bool,
) -> std::result::Result<Option<Vec<&'a Symbol>>, ()> {
    let requester = origin
        .iter()
        .find(|(_, loc)| loc.file == source.path && loc.prefix.is_empty())
        .map(|(scope, _)| scope.clone())
        .ok_or(())?;
    let same = map.get(&vec![]).map(|l| &l.file) == origin.get(&vec![]).map(|l| &l.file);
    let mut lookup = Lookup {
        symbols,
        docs,
        map,
        project: &source.project,
        methods,
        budget: 512,
        active: HashSet::new(),
    };
    lookup
        .walk(
            path.to_vec(),
            Context {
                requester,
                same_crate: same,
                floor: None,
                base: 0,
                depth: 0,
                touched: false,
            },
        )
        .map(|(touched, found)| touched.then_some(found))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn graph(files: &[(&str, &str)]) -> CodeSnapshot {
        let mut docs = HashMap::new();
        for (i, (path, body)) in files.iter().enumerate() {
            let mut doc = parse(&i.to_string(), path, body, 1).unwrap();
            assert!(!doc.parse_errors, "{path}");
            doc.project = Some("fixture".into());
            docs.insert(i.to_string(), doc);
        }
        rust_cfg::apply(&mut docs);
        let mut out = CodeSnapshot::default();
        out.symbols = docs.values().flat_map(|d| d.symbols.clone()).collect();
        out.calls = resolve::references(&out.symbols, &docs)
            .into_iter()
            .filter(|c| c.reference.is_call)
            .collect();
        out
    }
    fn names(graph: &CodeSnapshot, expression: &str) -> Vec<String> {
        let edge = graph
            .calls
            .iter()
            .find(|c| c.reference.expression == expression)
            .unwrap_or_else(|| panic!("missing {expression}: {:?}", graph.calls));
        let mut names: Vec<_> = edge
            .candidates
            .iter()
            .map(|id| {
                graph
                    .symbols
                    .iter()
                    .find(|s| &s.id == id)
                    .unwrap()
                    .qualified_name
                    .clone()
            })
            .collect();
        names.sort();
        names
    }
    const MANIFEST: &str = "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n";
    #[test]
    fn named_grouped_multihop_reexports_preserve_exact_targets_and_inline_scope() {
        let g = graph(&[
            ("/p/Cargo.toml", MANIFEST),
            ("/p/src/lib.rs", "mod implementation; mod stage {pub use crate::implementation::work;} pub mod facade {pub use crate::stage::{work as run};} fn entry(){crate::facade::run();run();}"),
            ("/p/src/implementation.rs", "pub fn work(){}"),
        ]);
        assert_eq!(names(&g, "crate::facade::run"), ["work"]);
        assert!(
            names(&g, "run").is_empty(),
            "an inline import must not leak to crate scope"
        );
    }
    #[test]
    fn glob_reexports_preserve_ambiguity_hide_private_items_and_handle_cycles() {
        let g = graph(&[
            ("/p/Cargo.toml", MANIFEST),
            ("/p/src/lib.rs", "mod one {pub fn same(){} pub fn only(){} fn hidden(){}} mod two {pub fn same(){}} pub mod api {pub use crate::one::*;pub use crate::two::*;} pub mod a {pub use crate::b::*;}pub mod b {pub use crate::a::*;pub use crate::one::*;} fn run(){crate::api::same();crate::api::only();crate::api::hidden();crate::a::only();}"),
        ]);
        assert_eq!(names(&g, "crate::api::same"), ["one::same", "two::same"]);
        assert_eq!(names(&g, "crate::api::only"), ["one::only"]);
        assert!(names(&g, "crate::api::hidden").is_empty());
        assert_eq!(names(&g, "crate::a::only"), ["one::only"]);
    }
    #[test]
    fn restricted_exports_do_not_cross_crate_boundaries_or_widen_private_targets() {
        let g = graph(&[
            ("/w/client/Cargo.toml", "[package]\nname='client'\n[dependencies]\nengine={path='../engine'}\n"),
            ("/w/client/src/main.rs", "fn main(){engine::api::public();engine::api::internal();engine::api::bad();}"),
            ("/w/engine/Cargo.toml", MANIFEST),
            ("/w/engine/src/lib.rs", "mod inner {pub fn work(){} pub(crate) fn internal(){} fn hidden(){}}pub mod api {pub use crate::inner::work as public;pub(crate) use crate::inner::internal;pub use crate::inner::hidden as bad;pub(super) use crate::inner::work as parent;pub(in crate::api) use crate::inner::work as local;}fn run(){crate::api::internal();crate::api::parent();crate::api::local();}"),
        ]);
        assert_eq!(names(&g, "engine::api::public"), ["inner::work"]);
        assert!(names(&g, "engine::api::internal").is_empty());
        assert!(names(&g, "engine::api::bad").is_empty());
        assert_eq!(names(&g, "crate::api::internal"), ["inner::internal"]);
        assert_eq!(names(&g, "crate::api::parent"), ["inner::work"]);
        assert!(names(&g, "crate::api::local").is_empty());
    }
    #[test]
    fn module_aliases_check_the_base_visibility_before_public_children() {
        for (visibility, expected) in [("pub ", vec!["implementation::work"]), ("", vec![])] {
            let source = format!("{visibility}mod implementation {{pub fn work(){{}}}}pub mod api {{pub use crate::implementation as nested;}}fn run(){{crate::api::nested::work();}}");
            let g = graph(&[("/p/Cargo.toml", MANIFEST), ("/p/src/lib.rs", &source)]);
            assert_eq!(names(&g, "crate::api::nested::work"), expected);
        }
    }
    #[test]
    fn unknown_cfg_and_duplicate_exports_never_publish_a_partial_unique_target() {
        let configured = format!("{MANIFEST}[features]\nfast=[]\n[package.metadata.rag.graph]\nversion=1\ntarget_kind='lib'\ndefault_features=false\nfeatures=['fast']\n");
        for imports in [
            "pub use crate::one::work;#[cfg(target_os=\"linux\")]pub use crate::two::work;",
            "pub use crate::one::*;#[cfg(target_os=\"linux\")]pub use crate::two::*;",
            "pub use crate::one::work;pub use crate::two::work;",
        ] {
            let source = format!("mod one{{pub fn work(){{}}}}mod two{{pub fn work(){{}}}}pub mod api{{{imports}}}fn run(){{crate::api::work();}}");
            let g = graph(&[("/p/Cargo.toml", &configured), ("/p/src/lib.rs", &source)]);
            assert!(names(&g, "crate::api::work").is_empty());
        }
        let source = "mod one{pub fn work(){}}mod two{pub fn work(){}}pub mod api{#[cfg(feature=\"fast\")]pub use crate::one::work;#[cfg(not(feature=\"fast\"))]pub use crate::two::work;}fn run(){crate::api::work();}";
        let g = graph(&[("/p/Cargo.toml", &configured), ("/p/src/lib.rs", source)]);
        assert_eq!(names(&g, "crate::api::work"), ["one::work"]);
    }
    #[test]
    fn reexports_cannot_import_associated_or_function_local_items_or_private_glob_bases() {
        let g = graph(&[
            ("/p/Cargo.toml", MANIFEST),
            ("/p/src/lib.rs", "pub struct A;impl A{pub fn method(){}}fn make(){pub fn nested(){}}mod source{struct Private;impl Private{pub fn method(){}}pub mod visible{mod hidden{pub fn work(){}}}}pub mod api{pub use crate::A::method as associated;pub use crate::make::nested as local;pub use crate::source::*;}fn run(){crate::api::associated();crate::api::local();crate::api::Private::method();crate::api::visible::hidden::work();}"),
        ]);
        for expression in [
            "crate::api::associated",
            "crate::api::local",
            "crate::api::Private::method",
            "crate::api::visible::hidden::work",
        ] {
            assert!(names(&g, expression).is_empty(), "{expression}");
        }
        let g = graph(&[("/p/Cargo.toml", MANIFEST), ("/p/src/lib.rs", "pub struct A;impl A{pub fn method(){}}pub mod api{pub use crate::A::*;}fn run(){crate::api::method();}")]);
        assert!(names(&g, "crate::api::method").is_empty());
        let g = graph(&[("/p/Cargo.toml", MANIFEST), ("/p/src/lib.rs", "mod source{struct Private;impl Private{pub fn method(){}}pub mod inside{pub use super::*;}pub mod visible{mod hidden{pub fn work(){}}pub mod inside{pub use super::*;}}}fn run(){crate::source::inside::Private::method();crate::source::visible::inside::hidden::work();}")]);
        assert!(names(&g, "crate::source::inside::Private::method").is_empty());
        assert!(names(&g, "crate::source::visible::inside::hidden::work").is_empty());
    }
    #[test]
    fn exhausted_reexport_depth_clears_other_glob_candidates() {
        let mut source = String::from("mod leaf{pub fn work(){}}pub mod api{pub use crate::leaf::*;pub use crate::n0::*;}fn run(){crate::api::work();}");
        for i in 0..18 {
            source.push_str(&format!("pub mod n{i}{{pub use crate::n{}::*;}}", i + 1));
        }
        source.push_str("pub mod n18{pub fn work(){}}");
        let g = graph(&[("/p/Cargo.toml", MANIFEST), ("/p/src/lib.rs", &source)]);
        assert!(names(&g, "crate::api::work").is_empty());
    }
}
