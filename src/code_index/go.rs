//! Go package/type binding over ingested sources only; no filesystem or toolchain execution.
use super::*;
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Facts {
    package: String,
    module: Option<String>,
    imports: Vec<Import>,
    bindings: Vec<Binding>,
    types: Vec<Type>,
    constrained: bool,
    pointer_methods: Vec<String>,
    opaque_ranges: Vec<(usize, usize)>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Import {
    alias: Option<String>,
    path: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Binding {
    name: String,
    ty: Option<String>,
    start: usize,
    end: usize,
    declared: usize,
    target_start: Option<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Type {
    name: String,
    alias: Option<String>,
    interface: bool,
}

pub(super) fn manifest(path: &str) -> bool {
    Path::new(path).file_name().is_some_and(|n| n == "go.mod")
}
fn diagnostic(out: &mut IndexedDocument, code: &str) {
    if !out.binding_limitations.iter().any(|s| s == code) {
        out.binding_limitations.push(code.into());
    }
}
fn uncomment(source: &str) -> Option<String> {
    if source.len() > 65536 {
        return None;
    }
    let b = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut quote = None;
    while i < b.len() {
        if let Some(q) = quote {
            out.push(b[i]);
            if b[i] == q {
                quote = None;
            } else if q == b'"' && b[i] == b'\\' {
                i += 1;
                out.push(*b.get(i)?);
            }
            i += 1;
            continue;
        }
        if matches!(b[i], b'"' | b'`') {
            quote = Some(b[i]);
            out.push(b[i]);
            i += 1;
        } else if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i..].starts_with(b"/*") {
            // go.mod accepts line comments only (unlike Go source files).
            return None;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    if quote.is_some() {
        None
    } else {
        String::from_utf8(out).ok()
    }
}
fn module_token(raw: &str) -> Option<String> {
    let mut chars = raw.chars();
    let first = chars.next()?;
    let value = if matches!(first, '"' | '`') {
        let mut value = String::new();
        let mut closed = false;
        while let Some(c) = chars.next() {
            if c == first {
                closed = true;
                break;
            }
            if first == '"' && c == '\\' {
                value.push(chars.next()?);
            } else {
                value.push(c);
            }
        }
        if !closed || chars.any(|c| !c.is_whitespace()) {
            return None;
        }
        value
    } else {
        raw.to_owned()
    };
    if value.is_empty()
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | '_' | '-' | '~'))
        || value
            .split('/')
            .any(|p| p.is_empty() || matches!(p, "." | ".."))
    {
        None
    } else {
        Some(value)
    }
}
pub(super) fn parse_manifest(source: &str, out: &mut IndexedDocument) {
    let mut facts = Facts::default();
    let cleaned = uncomment(source).unwrap_or_default();
    let declarations: Vec<_> = cleaned
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let mut words = line.split_whitespace();
            if words.next()? != "module" {
                return None;
            }
            Some(module_token(line["module".len()..].trim()))
        })
        .collect();
    if source.len() <= 65536 && declarations.len() == 1 {
        facts.module = declarations[0].clone();
    }
    if facts.module.is_none() {
        diagnostic(out, "GO_MODULE_DECLARATION_UNVERIFIED");
    }
    out.go = Some(facts);
}
fn simple_type(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "type_identifier" | "qualified_type" => Some(text(node, source).into()),
        "pointer_type" => simple_type(node.named_child(0)?, source),
        // Generic instantiation/type parameter semantics require more than a name.
        _ => None,
    }
}
pub(super) fn receiver_type(node: Node, source: &str) -> Option<String> {
    simple_type(
        node.child_by_field_name("receiver")?
            .named_child(0)?
            .child_by_field_name("type")?,
        source,
    )
}
fn scope(node: Node, parameter: bool) -> (usize, usize) {
    let mut parent = node.parent();
    while let Some(n) = parent {
        if (parameter
            && matches!(
                n.kind(),
                "function_declaration"
                    | "method_declaration"
                    | "func_literal"
                    | "type_spec"
                    | "type_alias"
            ))
            || (!parameter
                && matches!(
                    n.kind(),
                    "block" | "if_statement" | "for_statement" | "switch_statement"
                ))
        {
            return (n.start_byte(), n.end_byte());
        }
        parent = n.parent();
    }
    (0, usize::MAX)
}
fn names(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.children_by_field_name("name", &mut cursor).collect()
}
fn inferred(node: Node, source: &str) -> Option<String> {
    match node.kind() {
        "composite_literal" => simple_type(node.child_by_field_name("type")?, source),
        "unary_expression" if text(node, source).starts_with('&') => {
            inferred(node.named_child(0)?, source)
        }
        _ => None,
    }
}
pub(super) fn collect(node: Node, source: &str, out: &mut IndexedDocument) {
    if node.kind() == "source_file" {
        let filename = Path::new(&out.path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let mut cursor = node.walk();
        let header_end = node
            .named_children(&mut cursor)
            .find(|n| n.kind() == "package_clause")
            .map_or(source.len(), |n| n.start_byte());
        let tags = source[..header_end]
            .lines()
            .any(|l| l.trim().starts_with("//go:build ") || l.trim().starts_with("// +build "));
        let suffix = filename.trim_end_matches(".go").trim_end_matches("_test");
        let platform = suffix.rsplit('_').next().is_some_and(|s| {
            matches!(
                s,
                "aix"
                    | "android"
                    | "darwin"
                    | "dragonfly"
                    | "freebsd"
                    | "illumos"
                    | "ios"
                    | "js"
                    | "linux"
                    | "netbsd"
                    | "openbsd"
                    | "plan9"
                    | "solaris"
                    | "wasip1"
                    | "windows"
                    | "386"
                    | "amd64"
                    | "arm"
                    | "arm64"
                    | "loong64"
                    | "mips"
                    | "mipsle"
                    | "mips64"
                    | "mips64le"
                    | "ppc64"
                    | "ppc64le"
                    | "riscv64"
                    | "s390x"
                    | "wasm"
            )
        });
        let ignored = filename.starts_with(['_', '.']);
        out.go.as_mut().unwrap().constrained = tags || platform || ignored;
        if tags || platform || ignored {
            diagnostic(out, "GO_BUILD_SELECTION_UNVERIFIED");
        }
    }
    if node.kind() == "method_declaration" {
        if let (Some(receiver), Some(name)) = (
            node.child_by_field_name("receiver"),
            node.child_by_field_name("name"),
        ) {
            if receiver
                .named_child(0)
                .and_then(|n| n.child_by_field_name("type"))
                .is_some_and(|n| n.kind() == "pointer_type")
            {
                if let Some(ty) = receiver_type(node, source) {
                    out.go
                        .as_mut()
                        .unwrap()
                        .pointer_methods
                        .push(format!("{ty}::{}", text(name, source)));
                }
            }
        }
    }
    if node.kind() == "package_clause" {
        out.go.as_mut().unwrap().package = node
            .named_child(0)
            .map(|n| text(n, source).into())
            .unwrap_or_default();
    }
    if node.kind() == "import_spec" {
        if let Some(path) = node.child_by_field_name("path") {
            let raw = text(path, source);
            let path = if raw.starts_with('`') {
                Some(raw.trim_matches('`').to_owned())
            } else {
                serde_json::from_str::<String>(raw).ok()
            };
            if let Some(path) = path {
                let alias = node
                    .child_by_field_name("name")
                    .map(|n| text(n, source).to_owned());
                if alias.as_deref() == Some(".") {
                    diagnostic(out, "GO_DOT_IMPORT_UNMODELED");
                }
                if alias.as_deref() == Some("_") {
                    diagnostic(out, "GO_SIDE_EFFECT_IMPORT_CALLS_UNMODELED");
                }
                if path == "C" {
                    out.go.as_mut().unwrap().constrained = true;
                    diagnostic(out, "GO_CGO_BUILD_UNVERIFIED");
                }
                out.go
                    .as_mut()
                    .unwrap()
                    .imports
                    .push(Import { alias, path });
            } else {
                diagnostic(out, "GO_IMPORT_LITERAL_UNVERIFIED");
            }
        }
    }
    if matches!(node.kind(), "type_spec" | "type_alias") {
        if let (Some(name), Some(ty)) = (
            node.child_by_field_name("name"),
            node.child_by_field_name("type"),
        ) {
            let alias = if node.kind() == "type_alias" {
                simple_type(ty, source)
            } else {
                None
            };
            if node.child_by_field_name("type_parameters").is_some() {
                diagnostic(out, "GO_GENERIC_TYPE_BINDING_UNMODELED");
            }
            let (start, end) = scope(node, false);
            if start == 0 {
                out.go.as_mut().unwrap().types.push(Type {
                    name: text(name, source).into(),
                    alias,
                    interface: ty.kind() == "interface_type",
                });
            } else {
                out.go.as_mut().unwrap().bindings.push(Binding {
                    name: text(name, source).into(),
                    ty: None,
                    start,
                    end,
                    declared: name.start_byte(),
                    target_start: Some(node.start_byte()),
                });
            }
        }
    }
    if node.kind() == "func_literal" {
        out.go
            .as_mut()
            .unwrap()
            .opaque_ranges
            .push((node.start_byte(), node.end_byte()));
        diagnostic(out, "GO_ANONYMOUS_FUNCTION_OWNER_UNMODELED");
    }
    if node.kind() == "field_declaration" && node.child_by_field_name("name").is_none() {
        diagnostic(out, "GO_EMBEDDED_MEMBER_PROMOTION_UNMODELED");
    }
    let parameter = matches!(
        node.kind(),
        "parameter_declaration" | "variadic_parameter_declaration" | "type_parameter_declaration"
    );
    let mut ns = if parameter || matches!(node.kind(), "var_spec" | "const_spec") {
        names(node)
    } else {
        vec![]
    };
    let mut ty = node
        .child_by_field_name("type")
        .and_then(|n| simple_type(n, source));
    if matches!(
        node.kind(),
        "short_var_declaration" | "assignment_statement" | "range_clause"
    ) {
        if let Some(left) = node.child_by_field_name("left") {
            let mut cursor = left.walk();
            ns = left
                .named_children(&mut cursor)
                .filter(|n| n.kind() == "identifier")
                .collect();
        }
        if ns.len() == 1 {
            ty = node
                .child_by_field_name("right")
                .and_then(|r| r.named_child(0))
                .and_then(|n| inferred(n, source));
        }
    } else if node.kind() == "var_spec" && ty.is_none() && ns.len() == 1 {
        ty = node
            .child_by_field_name("value")
            .and_then(|r| r.named_child(0))
            .and_then(|n| inferred(n, source));
    }
    let (start, end) = scope(node, parameter);
    for name in ns {
        if text(name, source) != "_" {
            out.go.as_mut().unwrap().bindings.push(Binding {
                name: text(name, source).into(),
                ty: ty.clone(),
                start,
                end,
                declared: if parameter { start } else { node.end_byte() },
                target_start: None,
            });
        }
    }
}
/// Declaration names, package clauses, imports and selector fields are not bare uses.
pub(super) fn non_usage(node: Node) -> bool {
    let mut parent = node.parent();
    while let Some(p) = parent {
        if matches!(p.kind(), "package_clause" | "import_declaration") {
            return true;
        }
        if matches!(
            p.kind(),
            "parameter_declaration"
                | "variadic_parameter_declaration"
                | "type_parameter_declaration"
                | "var_spec"
                | "const_spec"
        ) && names(p).iter().any(|n| n.id() == node.id())
        {
            return true;
        }
        if p.kind() == "expression_list" {
            if let Some(decl) = p.parent() {
                if matches!(
                    decl.kind(),
                    "short_var_declaration" | "assignment_statement" | "range_clause"
                ) && decl
                    .child_by_field_name("left")
                    .is_some_and(|n| n.id() == p.id())
                {
                    return true;
                }
            }
        }
        parent = p.parent();
    }
    false
}
fn directory(doc: &IndexedDocument) -> &Path {
    Path::new(&doc.path).parent().unwrap_or(Path::new(""))
}
fn module<'a>(
    doc: &IndexedDocument,
    docs: &'a HashMap<String, IndexedDocument>,
) -> Option<(&'a Path, &'a str)> {
    let mut matches: Vec<_> = docs
        .values()
        .filter(|d| {
            manifest(&d.path)
                && d.project == doc.project
                && directory(doc).starts_with(directory(d))
        })
        .collect();
    matches.sort_by_key(|d| std::cmp::Reverse(directory(d).components().count()));
    let first = *matches.first()?;
    let root = directory(first);
    if matches.iter().filter(|d| directory(d) == root).count() != 1 {
        return None;
    }
    Some((root, first.go.as_ref()?.module.as_deref()?))
}
fn package<'a>(
    doc: &IndexedDocument,
    docs: &'a HashMap<String, IndexedDocument>,
    directory: &Path,
    importing: bool,
) -> Vec<&'a IndexedDocument> {
    docs.values()
        .filter(|d| {
            d.path.ends_with(".go")
                && d.project == doc.project
                && self::directory(d) == directory
                && !d.parse_errors
                && d.go.as_ref().is_some_and(|g| !g.constrained)
                && ((!importing && doc.path.ends_with("_test.go")) || !d.path.ends_with("_test.go"))
        })
        .collect()
}
fn imported<'a>(
    doc: &IndexedDocument,
    docs: &'a HashMap<String, IndexedDocument>,
    path: &str,
) -> Vec<&'a IndexedDocument> {
    let Some((root, own)) = module(doc, docs) else {
        return vec![];
    };
    let relative = if path == own {
        ""
    } else {
        let Some(r) = path.strip_prefix(own).and_then(|s| s.strip_prefix('/')) else {
            return vec![];
        };
        r
    };
    if !relative.is_empty()
        && relative
            .split('/')
            .any(|p| p.is_empty() || matches!(p, "." | ".."))
    {
        return vec![];
    }
    let dir = root.join(relative);
    let candidates: Vec<_> = package(doc, docs, &dir, true)
        .into_iter()
        .filter(|d| module(d, docs).is_some_and(|(r, m)| r == root && m == own))
        .collect();
    let packages: HashSet<_> = candidates
        .iter()
        .map(|d| d.go.as_ref().unwrap().package.as_str())
        .collect();
    if packages.len() == 1 {
        candidates
    } else {
        vec![]
    }
}
fn named<'a>(symbols: &'a [Symbol], pkg: &[&IndexedDocument], name: &str) -> Vec<&'a Symbol> {
    symbols
        .iter()
        .filter(|s| {
            s.qualified_name == name
                && pkg
                    .iter()
                    .any(|d| d.symbols.iter().any(|own| own.id == s.id))
        })
        .collect()
}
fn exported(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}
fn type_members<'a>(
    symbols: &'a [Symbol],
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
    ty: &str,
    member: &str,
    depth: usize,
    pointer_allowed: bool,
) -> (Vec<&'a Symbol>, &'static str) {
    if depth >= 16 {
        return (vec![], "go_type_alias_cycle_or_limit");
    }
    let (pkg, ty) = if let Some((alias, ty)) = ty.split_once('.') {
        let found: Vec<_> = doc
            .go
            .as_ref()
            .unwrap()
            .imports
            .iter()
            .flat_map(|i| {
                let pkg = imported(doc, docs, &i.path);
                if i.alias.as_deref().is_some_and(|a| a == alias)
                    || i.alias.is_none()
                        && pkg.iter().any(|d| d.go.as_ref().unwrap().package == alias)
                {
                    pkg
                } else {
                    vec![]
                }
            })
            .collect();
        if !exported(ty) || !exported(member) {
            return (vec![], "go_unexported_import_member");
        }
        (found, ty)
    } else {
        (
            package(doc, docs, directory(doc), false)
                .into_iter()
                .filter(|d| d.go.as_ref().unwrap().package == doc.go.as_ref().unwrap().package)
                .collect(),
            ty,
        )
    };
    let definitions: Vec<_> = pkg
        .iter()
        .flat_map(|d| {
            d.go.as_ref()
                .unwrap()
                .types
                .iter()
                .filter(move |t| t.name == ty)
                .map(move |t| (*d, t))
        })
        .collect();
    if definitions.len() != 1 {
        return (vec![], "go_type_missing_or_ambiguous");
    }
    let (owner, definition) = definitions[0];
    if let Some(alias) = &definition.alias {
        return type_members(
            symbols,
            docs,
            owner,
            alias,
            member,
            depth + 1,
            pointer_allowed,
        );
    }
    (
        named(symbols, &pkg, &format!("{ty}::{member}"))
            .into_iter()
            .filter(|s| {
                pointer_allowed
                    || !pkg.iter().any(|d| {
                        d.path == s.path
                            && d.go
                                .as_ref()
                                .unwrap()
                                .pointer_methods
                                .contains(&s.qualified_name)
                    })
            })
            .collect(),
        if definition.interface {
            "go_interface_contract"
        } else {
            "go_receiver_type"
        },
    )
}
pub(super) fn resolve(
    symbols: &[Symbol],
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
    r: &Reference,
) -> CallEdge {
    let mut candidates = vec![];
    let mut reason = "go_unresolved";
    let facts = doc.go.as_ref().unwrap();
    let parts: Vec<_> = r.expression.split('.').collect();
    let head = parts[0];
    if facts
        .opaque_ranges
        .iter()
        .any(|(start, end)| *start <= r.start_byte && r.start_byte < *end)
    {
        reason = "go_anonymous_owner_unmodeled";
    } else if facts.constrained || doc.parse_errors {
        reason = "go_build_or_parse_unverified";
    } else if !r
        .expression
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.'))
    {
        reason = "go_dynamic_or_generic_expression";
    } else if let Some(binding) = facts
        .bindings
        .iter()
        .filter(|b| {
            b.name == head
                && b.start <= r.start_byte
                && r.start_byte < b.end
                && b.declared <= r.start_byte
        })
        .max_by_key(|b| (b.start, b.declared))
    {
        reason = "go_shadowed_or_dynamic_value";
        if parts.len() == 1 {
            if let Some(start) = binding.target_start {
                candidates = symbols
                    .iter()
                    .filter(|s| s.document_id == r.document_id && s.start_byte == start)
                    .collect();
                reason = "go_local_type";
            }
        }
        if parts.len() == 2 {
            if let Some(ty) = &binding.ty {
                (candidates, reason) = type_members(symbols, docs, doc, ty, parts[1], 0, true);
            }
        }
    } else if parts.len() == 2 {
        let mut imports = 0;
        for i in &facts.imports {
            let pkg = imported(doc, docs, &i.path);
            let alias = i.alias.as_deref();
            if alias == Some(head)
                || alias.is_none() && pkg.iter().any(|d| d.go.as_ref().unwrap().package == head)
            {
                imports += 1;
                if exported(parts[1]) {
                    candidates.extend(named(symbols, &pkg, parts[1]));
                }
            }
        }
        if imports > 1 {
            candidates.clear();
            reason = "go_import_alias_ambiguous";
        } else if imports > 0 {
            reason = "go_module_import";
        } else {
            (candidates, reason) = type_members(symbols, docs, doc, head, parts[1], 0, false);
        }
    } else if parts.len() == 3 {
        (candidates, reason) = type_members(
            symbols,
            docs,
            doc,
            &parts[..2].join("."),
            parts[2],
            0,
            false,
        );
    } else if parts.len() == 1 && head != "init" {
        let pkg: Vec<_> = package(doc, docs, directory(doc), false)
            .into_iter()
            .filter(|d| d.go.as_ref().unwrap().package == facts.package)
            .collect();
        candidates = named(symbols, &pkg, head);
        reason = "go_package_scope";
    }
    candidates.sort_by_key(|s| &s.id);
    candidates.dedup_by_key(|s| &s.id);
    CallEdge {
        reference: r.clone(),
        resolution: match candidates.len() {
            0 => "unresolved",
            1 => "resolved_static",
            _ => "ambiguous",
        }
        .into(),
        candidates: candidates.iter().map(|s| s.id.clone()).collect(),
        provenance: "derived",
        resolution_basis: reason.into(),
    }
}
pub(super) fn entrypoint(doc: &IndexedDocument, s: &Symbol) -> Option<&'static str> {
    if !matches!(
        s.kind.as_str(),
        "function_declaration" | "method_declaration"
    ) {
        return None;
    }
    if s.name == "main" && doc.go.as_ref()?.package == "main" {
        Some("conventional_main")
    } else if s.name == "init" {
        Some("go_package_initializer")
    } else if doc.path.ends_with("_test.go")
        && ["Test", "Benchmark", "Fuzz", "Example"]
            .iter()
            .any(|p| s.name.starts_with(p))
    {
        Some("go_test_candidate")
    } else if exported(&s.name) {
        Some("exported_api")
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
        out.entrypoints = analysis::entrypoints(&out.symbols, &docs);
        out
    }
    fn call<'a>(s: &'a CodeSnapshot, expression: &str) -> &'a CallEdge {
        s.calls
            .iter()
            .find(|r| r.reference.expression == expression)
            .unwrap_or_else(|| panic!("missing {expression}: {:?}", s.calls))
    }
    fn target<'a>(s: &'a CodeSnapshot, expression: &str) -> Vec<&'a str> {
        call(s, expression)
            .candidates
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
    fn package_functions_receivers_and_interface_contracts_are_distinct() {
        let s=snapshot(&[
            ("/p/service.go","p","package main\ntype Service struct{}\nfunc (s *Service) Run() { helper() }\nfunc helper() {}\ntype Reader interface { Read() }\n"),
            ("/p/main.go","p","package main\nfunc main() { helper() }\nfunc use(s *Service, r Reader) { s.Run(); r.Read() }\n"),
        ]);
        assert_eq!(target(&s, "helper"), vec!["helper"]);
        assert_eq!(target(&s, "s.Run"), vec!["Service::Run"]);
        assert_eq!(target(&s, "r.Read"), vec!["Reader::Read"]);
        assert_eq!(call(&s, "r.Read").resolution_basis, "go_interface_contract");
        assert!(s.entrypoints.iter().any(|e| e.kind == "conventional_main"));
        assert!(s
            .symbols
            .iter()
            .any(|s| s.name == "Reader" && s.kind == "type_spec"));
    }
    #[test]
    fn module_imports_use_real_package_name_and_do_not_guess_external_dependencies() {
        let s=snapshot(&[
            ("/p/go.mod","p","// module fake/path\nmodule example.org/app\n"),
            ("/p/api/api.go","p","package renamed\nfunc Work() {}\nfunc private() {}\n"),
            ("/p/main.go","p","package main\nimport \"example.org/app/api\"\nfunc main() { renamed.Work(); renamed.private() }"),
            ("/p/alias.go","p","package main\nimport svc \"example.org/app/api\"\nimport ext \"external.org/api\"\nfunc use() {svc.Work();ext.Work()}"),
        ]);
        assert_eq!(target(&s, "renamed.Work"), vec!["Work"]);
        assert_eq!(target(&s, "svc.Work"), vec!["Work"]);
        assert!(target(&s, "renamed.private").is_empty());
        assert!(target(&s, "ext.Work").is_empty());
    }
    #[test]
    fn shadowed_names_composite_receivers_and_pointer_method_expressions() {
        let s=snapshot(&[("/p/a.go","p","package p\ntype Service struct{}\nfunc (s *Service) Run() {}\nfunc Work() {}\nfunc use(Work func()) { Work(); s := &Service{}; s.Run(); callback := Service.Run; _ = callback }\n")]);
        assert!(target(&s, "Work").is_empty());
        assert_eq!(target(&s, "s.Run"), vec!["Service::Run"]);
        let bound = s
            .bound_references
            .iter()
            .find(|r| r.reference.expression == "Service.Run")
            .unwrap();
        assert!(bound.candidates.is_empty(), "{bound:?}");
        let r = s.query(
            "references",
            &CodeQuery {
                symbol: Some("Service::Run".into()),
                ..Default::default()
            },
        );
        assert_eq!(r["total"], 1, "{r}");
    }
    #[test]
    fn tests_build_tags_other_projects_and_nested_modules_do_not_leak_targets() {
        let s=snapshot(&[
            ("/p/go.mod","p","module example.org/app"),
            ("/p/main.go","p","package main\nimport api \"example.org/app/nested\"\nfunc main() { testOnly(); platform(); foreign(); api.Work() }"),
            ("/p/main_test.go","p","package main\nfunc testOnly() {}\nfunc TestExample() {testOnly()}"),
            ("/p/impl_linux.go","p","package main\nfunc platform() {}"),
            ("/p/foreign.go","other","package main\nfunc foreign() {}"),
            ("/p/nested/go.mod","p","module other.org/nested"),
            ("/p/nested/a.go","p","package nested\nfunc Work() {}"),
        ]);
        let production = s.calls.iter().filter(|r| r.reference.path == "/p/main.go");
        assert!(
            production.clone().all(|r| r.candidates.is_empty()),
            "{:?}",
            s.calls
        );
        assert!(s
            .calls
            .iter()
            .any(|r| r.reference.path.ends_with("_test.go") && !r.candidates.is_empty()));
    }
    #[test]
    fn aliases_interfaces_cycles_and_anonymous_owners_are_honest() {
        let s=snapshot(&[("/p/a.go","p","package p\ntype Service struct{}\nfunc (s Service) Run() {}\ntype Alias = Service\ntype A = B\ntype B = A\nfunc helper() {}\nfunc use(a Alias,b A) { a.Run(); b.Run(); f := func(){helper()}; _ = f }" )]);
        assert_eq!(target(&s, "a.Run"), vec!["Service::Run"]);
        assert!(target(&s, "b.Run").is_empty());
        assert!(target(&s, "helper").is_empty());
        assert_eq!(
            call(&s, "helper").resolution_basis,
            "go_anonymous_owner_unmodeled"
        );
    }
    #[test]
    fn go_manifest_comments_and_malformed_declarations_are_not_module_facts() {
        for (source, expected) in [
            (
                "/* module wrong */\nmodule example.org/app // comment",
                None,
            ),
            ("/*\nmodule wrong\n*/\nmodule example.org/app", None),
            ("/* module wrong", None),
            ("module x\nmodule y", None),
            ("module ../x", None),
            ("module `example.org/app`", Some("example.org/app")),
            (
                "module \"example.org/app\" // comment",
                Some("example.org/app"),
            ),
            ("module\nmodule example.org/app", None),
        ] {
            let d = parse("m", "/p/go.mod", source, 1).unwrap();
            assert_eq!(d.go.unwrap().module.as_deref(), expected, "{source}");
        }
    }
    #[test]
    fn identical_source_paths_in_different_projects_keep_distinct_symbol_ids() {
        let s = snapshot(&[
            ("/p/api.go", "p", "package p\nfunc Work() {}"),
            ("/p/api.go", "other", "package p\nfunc Work() {}"),
            ("/p/main.go", "p", "package p\nfunc use(){Work()}"),
        ]);
        let binding = call(&s, "Work");
        assert_eq!(binding.candidates.len(), 1, "{binding:?}");
        assert_eq!(binding.candidates[0], s.symbols[0].id);
    }
    #[test]
    fn local_types_and_constants_shadow_package_declarations() {
        let s=snapshot(&[("/p/a.go","p","package p\ntype Name struct{}\nfunc Work() {}\nfunc use(){type Name int;const Work=1; _=Work;var v Name;_=v}\n")]);
        let outer = s
            .symbols
            .iter()
            .find(|s| s.qualified_name == "Name")
            .unwrap();
        let inner = s
            .symbols
            .iter()
            .find(|s| s.qualified_name == "use::Name")
            .unwrap();
        let r = s.query(
            "references",
            &CodeQuery {
                symbol: Some(outer.id.clone()),
                ..Default::default()
            },
        );
        assert_eq!(r["total"], 0, "{r}");
        let r = s.query(
            "references",
            &CodeQuery {
                symbol: Some(inner.id.clone()),
                ..Default::default()
            },
        );
        assert_eq!(r["total"], 1, "{r}");
        let r = s.query(
            "references",
            &CodeQuery {
                symbol: Some("Work".into()),
                ..Default::default()
            },
        );
        assert_eq!(r["total"], 0, "{r}");
    }
    #[test]
    fn unindexed_and_stale_nested_manifests_block_parent_module_fallback() {
        use crate::models::Document;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("boundaries.duckdb")).unwrap();
        let doc = |id: &str, path: &str, content: &str| Document {
            id: id.into(),
            uri: format!("file://{path}"),
            source_file: Some(path.into()),
            content: content.into(),
            title: path.into(),
            wing: Some("p".into()),
            ..Default::default()
        };
        for d in [doc("root","/p/go.mod","module example.org/app"),doc("main","/p/main.go","package main\nimport nested \"example.org/app/nested\"\nfunc main(){nested.Work()}"),doc("api","/p/nested/a.go","package nested\nfunc Work(){}")]{store.upsert_document(&d).unwrap();}
        store.index_code().unwrap();
        assert_eq!(
            target(&store.code_snapshot().unwrap(), "nested.Work"),
            vec!["Work"]
        );
        store
            .upsert_document(&doc(
                "boundary",
                "/p/nested/go.mod",
                "module other.org/nested",
            ))
            .unwrap();
        let s = store.code_snapshot().unwrap();
        assert!(target(&s, "nested.Work").is_empty());
        assert!(s.unindexed_documents.contains(&"boundary".to_owned()));
        store.index_code().unwrap();
        assert!(target(&store.code_snapshot().unwrap(), "nested.Work").is_empty());
        store
            .upsert_document(&doc(
                "boundary",
                "/p/nested/go.mod",
                "module changed.org/nested",
            ))
            .unwrap();
        let s = store.code_snapshot().unwrap();
        assert!(target(&s, "nested.Work").is_empty());
        assert!(s.stale_documents.contains(&"boundary".to_owned()));
    }
}
