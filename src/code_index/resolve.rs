//! Conservative static binding: imports, lexical declarations and annotated receivers.
use super::*;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Binding {
    local: String,
    target: Option<String>,
    ty: Option<String>,
    start: usize,
    end: usize,
    declared: usize,
    #[serde(default)]
    hoisted: bool,
}
fn binding_scope(node: Node) -> (usize, usize) {
    let mut parent = node.parent();
    while let Some(p) = parent {
        if matches!(
            p.kind(),
            "block"
                | "statement_block"
                | "function_definition"
                | "function_item"
                | "function_declaration"
                | "arrow_function"
                | "method_definition"
                | "class_body"
        ) {
            return (p.start_byte(), p.end_byte());
        }
        parent = p.parent();
    }
    (0, usize::MAX)
}
fn add(
    out: &mut IndexedDocument,
    node: Node,
    local: &str,
    target: Option<String>,
    ty: Option<String>,
    parameter: bool,
) {
    let local = local.trim();
    if local.is_empty()
        || !local
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
    {
        return;
    }
    let (start, end) = binding_scope(node);
    let js = matches!(
        Path::new(&out.path).extension().and_then(|e| e.to_str()),
        Some("js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx")
    );
    out.bindings.push(Binding {
        hoisted: js && node.kind() == "variable_declarator",
        local: local.into(),
        target,
        ty,
        start,
        end,
        declared: if parameter { start } else { node.start_byte() },
    });
}
fn import_items(out: &mut IndexedDocument, node: Node, prefix: &str, items: &str) {
    for item in items.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let parts: Vec<_> = item.split_whitespace().collect();
        let original = parts[0];
        let alias = if parts.len() == 3 && parts[1] == "as" {
            parts[2]
        } else {
            original.rsplit("::").next().unwrap_or(original)
        };
        add(
            out,
            node,
            alias,
            Some(format!("{prefix}{original}")),
            None,
            false,
        );
    }
}
fn rust_use(out: &mut IndexedDocument, node: Node, prefix: &str, body: &str) {
    // Split only at group depth zero (nested grouped use declarations).
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in body
        .char_indices()
        .chain(std::iter::once((body.len(), ',')))
    {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                let item = body[start..i].trim();
                if let Some(open) = item.find('{') {
                    if let Some(close) = item.rfind('}') {
                        rust_use(
                            out,
                            node,
                            &format!("{prefix}{}", &item[..open]),
                            &item[open + 1..close],
                        );
                    }
                } else if !item.is_empty() {
                    import_items(out, node, prefix, item);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
}
pub(super) fn collect(node: Node, source: &str, out: &mut IndexedDocument) {
    let raw = text(node, source);
    match node.kind() {
        "type_item" | "type_alias_declaration" => {
            if let (Some(name), Some(ty)) = (
                node.child_by_field_name("name"),
                node.child_by_field_name("type")
                    .or_else(|| node.child_by_field_name("value")),
            ) {
                add(
                    out,
                    node,
                    text(name, source),
                    None,
                    Some(text(ty, source).into()),
                    true,
                );
            }
        }
        "export_statement" => {
            if node.child_by_field_name("source").is_none() {
                if let Some(decl) = node.child_by_field_name("declaration") {
                    if let Some(name) = decl.child_by_field_name("name") {
                        let name = text(name, source).to_string();
                        out.exports.insert(
                            if raw.starts_with("export default ") {
                                "default".into()
                            } else {
                                name.clone()
                            },
                            name,
                        );
                    }
                    // Exported const arrow/function variables are declared one level below.
                    let mut cursor = decl.walk();
                    for child in decl.named_children(&mut cursor) {
                        if child.kind() == "variable_declarator" {
                            if let Some(name) = child.child_by_field_name("name") {
                                out.exports
                                    .insert(text(name, source).into(), text(name, source).into());
                            }
                        }
                    }
                }
                if raw.starts_with("export default ") {
                    if let Some(value) = node.child_by_field_name("value") {
                        if value.kind() == "identifier" {
                            out.exports
                                .insert("default".into(), text(value, source).into());
                        }
                    }
                }
                if let (Some(open), Some(close)) = (raw.find('{'), raw.rfind('}')) {
                    if raw.starts_with("export {") {
                        for item in raw[open + 1..close].split(',') {
                            let parts: Vec<_> = item.split_whitespace().collect();
                            if let Some(name) = parts.first() {
                                out.exports.insert(
                                    if parts.len() == 3 {
                                        parts[2].into()
                                    } else {
                                        (*name).into()
                                    },
                                    (*name).into(),
                                );
                            }
                        }
                    }
                }
            }
        }
        "use_declaration" => {
            if let Some((_, body)) = raw.split_once("use ") {
                rust_use(out, node, "", body.trim().trim_end_matches(';'));
            }
        }
        "import_statement" if out.path.ends_with(".py") => {
            for item in raw.trim_start_matches("import ").split(',') {
                let parts: Vec<_> = item.split_whitespace().collect();
                if let Some(module) = parts.first() {
                    let alias = if parts.len() == 3 {
                        parts[2]
                    } else {
                        module.split('.').next().unwrap()
                    };
                    add(out, node, alias, Some(format!("{module}::*")), None, false);
                }
            }
        }
        "import_from_statement" => {
            if let Some((module, items)) = raw.trim_start_matches("from ").split_once(" import ") {
                import_items(
                    out,
                    node,
                    &format!("{}::", module.trim()),
                    items.trim().trim_start_matches('(').trim_end_matches(')'),
                );
            }
        }
        "import_statement" => {
            if let Some(src) = node.child_by_field_name("source") {
                let module = text(src, source).trim_matches(['\'', '"']);
                let clause = raw
                    .strip_prefix("import ")
                    .unwrap_or(raw)
                    .split(" from ")
                    .next()
                    .unwrap_or("")
                    .trim();
                if let Some(open) = clause.find('{') {
                    if let Some(close) = clause.rfind('}') {
                        import_items(out, node, &format!("{module}::"), &clause[open + 1..close]);
                    }
                }
                if clause.starts_with('*') {
                    if let Some(alias) = clause.split(" as ").nth(1) {
                        add(out, node, alias, Some(format!("{module}::*")), None, false);
                    }
                } else {
                    let default = clause.split(',').next().unwrap_or("").trim();
                    if !default.starts_with('{') {
                        add(
                            out,
                            node,
                            default,
                            Some(format!("{module}::default")),
                            None,
                            false,
                        );
                    }
                }
            }
        }
        "parameter"
        | "typed_parameter"
        | "required_parameter"
        | "optional_parameter"
        | "default_parameter"
        | "typed_default_parameter"
        | "assignment_pattern"
        | "rest_pattern"
        | "list_splat_pattern"
        | "dictionary_splat_pattern"
        | "let_declaration"
        | "variable_declarator" => {
            let pattern = node
                .child_by_field_name("pattern")
                .or_else(|| node.child_by_field_name("name"))
                .or_else(|| node.child_by_field_name("left"))
                .or_else(|| {
                    if matches!(
                        node.kind(),
                        "rest_pattern" | "list_splat_pattern" | "dictionary_splat_pattern"
                    ) {
                        node.named_child(0)
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    if node.kind() == "typed_parameter" {
                        node.named_child(0)
                    } else {
                        None
                    }
                });
            if let Some(pattern) = pattern {
                let ty = node
                    .child_by_field_name("type")
                    .map(|n| text(n, source).trim_start_matches(':').trim().to_string());
                let value = node.child_by_field_name("value");
                let inferred = value
                    .and_then(|v| {
                        if v.kind() == "new_expression" {
                            v.child_by_field_name("constructor")
                        } else if matches!(v.kind(), "call" | "call_expression") {
                            v.child_by_field_name("function")
                        } else {
                            None
                        }
                    })
                    .map(|n| text(n, source).to_string())
                    .filter(|s| {
                        s.rsplit([':', '.'])
                            .next()
                            .is_some_and(|n| n.chars().next().is_some_and(char::is_uppercase))
                    });
                add(
                    out,
                    node,
                    text(pattern, source),
                    None,
                    ty.or(inferred),
                    node.kind().contains("parameter"),
                );
            }
        }
        "assignment" | "assignment_expression" => {
            if let (Some(left), Some(right)) = (
                node.child_by_field_name("left"),
                node.child_by_field_name("right"),
            ) {
                let ty = node
                    .child_by_field_name("type")
                    .map(|n| text(n, source).to_string())
                    .or_else(|| {
                        right
                            .child_by_field_name("function")
                            .map(|n| text(n, source).to_string())
                            .filter(|s| {
                                s.rsplit([':', '.']).next().is_some_and(|n| {
                                    n.chars().next().is_some_and(char::is_uppercase)
                                })
                            })
                    });
                add(out, node, text(left, source), None, ty, false);
            }
        }
        _ => {}
    }
    // Untyped Python/JS parameters still shadow imported/global names.
    if matches!(node.kind(), "parameters" | "formal_parameters") {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "identifier" {
                add(
                    out,
                    child,
                    text(child, source),
                    None,
                    if text(child, source) == "self" {
                        Some("Self".into())
                    } else {
                        None
                    },
                    true,
                );
            }
        }
    }
}
/// Import declaration names refer through the imported local binding. Source
/// module tokens and original names of aliased imports must not resolve as an
/// unrelated lexical symbol in the importing document.
pub(super) fn import_local_at(
    doc: &IndexedDocument,
    statement: Node,
    node: Node,
    source: &str,
) -> bool {
    let local = text(node, source);
    if !doc
        .bindings
        .iter()
        .any(|b| b.local == local && b.target.is_some() && b.declared == statement.start_byte())
    {
        return false;
    }
    let mut stack = vec![statement];
    let mut last = None;
    let mut visited = 0;
    while let Some(n) = stack.pop() {
        visited += 1;
        if visited > 4096 {
            return false;
        }
        if matches!(n.kind(), "identifier" | "type_identifier") && text(n, source) == local {
            last = Some(last.map_or(n.start_byte(), |v: usize| v.max(n.start_byte())));
        }
        let mut cursor = n.walk();
        stack.extend(n.named_children(&mut cursor));
    }
    last == Some(node.start_byte())
}
fn normalized(path: &Path) -> String {
    let mut result = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            _ => result.push(c.as_os_str()),
        }
    }
    result.to_string_lossy().replace('\\', "/")
}
fn module_paths(current: &str, module: &str) -> Vec<String> {
    let current = Path::new(current);
    let parent = current.parent().unwrap_or(Path::new(""));
    if current.extension().is_some_and(|e| e == "rs") {
        let parts: Vec<_> = module.split("::").collect();
        let (mut base, skip) = if parts.first() == Some(&"crate") {
            let root = current
                .ancestors()
                .find(|p| p.file_name().is_some_and(|n| n == "src"));
            let Some(root) = root else { return vec![] };
            (root.to_path_buf(), 1)
        } else if parts.first() == Some(&"super") {
            (parent.parent().unwrap_or(parent).to_path_buf(), 1)
        } else if parts.first() == Some(&"self") {
            (parent.to_path_buf(), 1)
        } else {
            (parent.to_path_buf(), 0)
        };
        for p in parts.iter().skip(skip) {
            base.push(p);
        }
        return vec![
            normalized(&base.with_extension("rs")),
            normalized(&base.join("mod.rs")),
        ];
    }
    if current.extension().is_some_and(|e| e == "py") {
        let dots = module.chars().take_while(|c| *c == '.').count();
        let mut base = parent.to_path_buf();
        for _ in 1..dots {
            base = base.parent().unwrap_or(&base).to_path_buf();
        }
        // Absolute Python imports are tried from each enclosing package root;
        // only exact source paths qualify, never arbitrary suffix matches.
        let roots = if dots > 0 {
            vec![base]
        } else {
            parent.ancestors().map(Path::to_path_buf).collect()
        };
        return roots
            .into_iter()
            .flat_map(|root| {
                let path = root.join(module.trim_start_matches('.').replace('.', "/"));
                [
                    normalized(&path.with_extension("py")),
                    normalized(&path.join("__init__.py")),
                ]
            })
            .collect();
    }
    // Package imports need package/tsconfig metadata; never guess their target.
    if !module.starts_with('.') {
        return vec![];
    }
    let base = parent.join(module);
    let mut paths = vec![normalized(&base)];
    for ext in ["ts", "tsx", "js", "jsx", "mjs", "cjs"] {
        paths.push(normalized(&base.with_extension(ext)));
        paths.push(normalized(&base.join(format!("index.{ext}"))));
    }
    paths
}
pub(super) fn matching<'a>(symbols: &'a [Symbol], path: &str, name: &str) -> Vec<&'a Symbol> {
    symbols
        .iter()
        .filter(|s| s.path == path && s.qualified_name == name)
        .collect()
}
fn rust_imported<'a>(
    symbols: &'a [Symbol],
    documents: &HashMap<String, IndexedDocument>,
    allow_methods: bool,
    current: &str,
    module: &str,
    name: &str,
) -> Vec<&'a Symbol> {
    let path = Path::new(current);
    let Some(root) = path
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "src"))
    else {
        return vec![];
    };
    let relative = path.strip_prefix(root).unwrap();
    let mut scope = relative
        .with_extension("")
        .components()
        .map(|p| p.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    if scope
        .last()
        .is_some_and(|s| matches!(s.as_str(), "mod" | "lib" | "main"))
    {
        scope.pop();
    }
    let mut parts = module.split("::").peekable();
    let mut absolute = match parts.peek().copied() {
        Some("self" | "super") => scope,
        _ => Vec::new(),
    };
    for part in parts {
        match part {
            "crate" => absolute.clear(),
            "self" => {}
            "super" => {
                if absolute.pop().is_none() {
                    return vec![];
                }
            }
            _ => absolute.push(part.into()),
        }
    }
    absolute.extend(name.split("::").map(str::to_string));
    let mut result = Vec::new();
    for split in 0..absolute.len() {
        let module = root.join(absolute[..split].join("/"));
        let qualified = absolute[split..].join("::");
        let files = if split == 0 {
            vec![root.join("lib.rs"), root.join("main.rs")]
        } else {
            vec![module.with_extension("rs"), module.join("mod.rs")]
        };
        for file in files {
            result.extend(if allow_methods {
                super::inheritance::lookup(symbols, documents, &normalized(&file), &qualified)
            } else {
                matching(symbols, &normalized(&file), &qualified)
            });
        }
    }
    result
}
fn effective_type<'a>(doc: &'a IndexedDocument, at: usize, initial: &'a str) -> Option<&'a str> {
    let mut ty = initial;
    let mut seen = HashSet::new();
    for _ in 0..16 {
        ty = ty.trim_start_matches('&').trim_start_matches("mut ").trim();
        if !seen.insert(ty) {
            return None;
        }
        if let Some(alias) = doc
            .bindings
            .iter()
            .filter(|b| {
                b.local == ty
                    && b.ty.is_some()
                    && b.target.is_none()
                    && b.start <= at
                    && at < b.end
                    && b.declared <= at
            })
            .max_by_key(|b| (b.start, b.declared))
        {
            ty = alias.ty.as_deref()?;
        } else {
            return Some(ty);
        }
    }
    None
}
fn imported_with_methods<'a>(
    symbols: &'a [Symbol],
    documents: &HashMap<String, IndexedDocument>,
    path: &str,
    target: &str,
    member: Option<&str>,
    allow_methods: bool,
) -> Vec<&'a Symbol> {
    let Some((module, item)) = target.rsplit_once("::") else {
        return vec![];
    };
    let name = if item == "*" {
        member.unwrap_or("").to_string()
    } else if let Some(member) = member {
        format!("{item}::{member}")
    } else {
        item.into()
    };
    if path.ends_with(".rs") {
        return rust_imported(symbols, documents, allow_methods, path, module, &name);
    }
    let paths = module_paths(path, module);
    let js = matches!(
        Path::new(path).extension().and_then(|e| e.to_str()),
        Some("js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx")
    );
    let mut matches = Vec::new();
    for p in paths {
        if js {
            let (export, tail) = name.split_once("::").unwrap_or((&name, ""));
            for document in documents.values().filter(|d| d.path == p) {
                if let Some(actual) = document.exports.get(export) {
                    let qualified = if tail.is_empty() {
                        actual.clone()
                    } else {
                        format!("{actual}::{tail}")
                    };
                    matches.extend(if allow_methods {
                        super::inheritance::lookup(symbols, documents, &p, &qualified)
                    } else {
                        matching(symbols, &p, &qualified)
                    });
                }
            }
        } else {
            matches.extend(if allow_methods {
                super::inheritance::lookup(symbols, documents, &p, &name)
            } else {
                matching(symbols, &p, &name)
            });
        }
    }
    if matches.is_empty() && path.ends_with(".py") {
        if let Some((namespace, leaf)) = name.rsplit_once("::") {
            let module = if module.chars().all(|c| c == '.') {
                format!("{module}{}", namespace.replace("::", "."))
            } else {
                format!("{module}.{}", namespace.replace("::", "."))
            };
            for p in module_paths(path, &module) {
                matches.extend(matching(symbols, &p, leaf));
            }
        }
    }
    matches
}
pub(super) fn imported<'a>(
    symbols: &'a [Symbol],
    documents: &HashMap<String, IndexedDocument>,
    path: &str,
    target: &str,
    member: Option<&str>,
) -> Vec<&'a Symbol> {
    imported_with_methods(symbols, documents, path, target, member, true)
}
pub(super) fn imported_type<'a>(
    symbols: &'a [Symbol],
    documents: &HashMap<String, IndexedDocument>,
    path: &str,
    target: &str,
    member: Option<&str>,
) -> Vec<&'a Symbol> {
    imported_with_methods(symbols, documents, path, target, member, false)
}
pub(super) fn import_target<'a>(
    doc: &'a IndexedDocument,
    name: &str,
    at: usize,
) -> Option<&'a str> {
    doc.bindings
        .iter()
        .filter(|b| {
            b.local == name && b.target.is_some() && b.start <= at && at < b.end && b.declared <= at
        })
        .max_by_key(|b| (b.start, b.declared))
        .and_then(|b| b.target.as_deref())
}
pub(super) fn references(
    symbols: &[Symbol],
    documents: &HashMap<String, IndexedDocument>,
) -> Vec<CallEdge> {
    let mut result = vec![];
    for doc in documents.values() {
        for r in &doc.references {
            if doc.shell.is_some() {
                result.push(super::shell::resolve(symbols, documents, doc, r));
                continue;
            }
            if doc.c_family.is_some() {
                result.push(super::c_family::resolve(symbols, documents, doc, r));
                continue;
            }
            if doc.java.is_some() {
                result.push(super::java::resolve(symbols, documents, doc, r));
                continue;
            }
            if doc.path.ends_with(".go") {
                result.push(super::go::resolve(symbols, documents, doc, r));
                continue;
            }
            if !doc.binding_limitations.is_empty() {
                result.push(CallEdge {
                    reference: r.clone(),
                    candidates: vec![],
                    resolution: "unresolved".into(),
                    provenance: "derived",
                    resolution_basis: doc.binding_limitations.join(","),
                });
                continue;
            }
            let caller = symbols.iter().find(|s| Some(&s.id) == r.caller.as_ref());
            let expr = r.expression.replace("::<", "<");
            let expr = expr.split('<').next().unwrap_or(&expr);
            let segments: Vec<_> = expr.split("::").flat_map(|s| s.split('.')).collect();
            let head = segments[0];
            let binding = doc
                .bindings
                .iter()
                .filter(|b| {
                    b.local == head
                        && b.start <= r.start_byte
                        && r.start_byte < b.end
                        && (b.hoisted || b.declared <= r.start_byte)
                })
                .max_by_key(|b| (b.start, b.declared));
            let mut candidates: Vec<&Symbol> = vec![];
            let mut reason = "unresolved";
            if matches!(head, "super()" | "super")
                && segments.len() == 2
                && r.expression.contains('.')
            {
                if let Some(parent) =
                    caller.and_then(|s| s.qualified_name.rsplit_once("::").map(|p| p.0))
                {
                    candidates = super::inheritance::methods(
                        symbols,
                        documents,
                        &doc.path,
                        &format!("{parent}::{}", r.name),
                        true,
                    );
                    reason = "super_method";
                }
            } else if let Some(binding) = binding {
                if binding.hoisted && r.start_byte < binding.declared {
                    reason = "uninitialized_binding";
                } else if let Some(target) = &binding.target {
                    let member = if segments.len() > 1 {
                        Some(segments[1..].join("::"))
                    } else {
                        None
                    };
                    candidates = imported(symbols, documents, &doc.path, target, member.as_deref());
                    reason = "import";
                } else if let Some(ty) = &binding.ty {
                    if segments.len() == 2 {
                        let ty = effective_type(doc, r.start_byte, ty)
                            .unwrap_or("__unresolved_type_alias__");
                        let owner_type =
                            caller.and_then(|s| s.qualified_name.rsplit_once("::").map(|p| p.0));
                        let ty = if ty == "Self" {
                            owner_type.unwrap_or(ty)
                        } else {
                            ty
                        };
                        let parts: Vec<_> = ty.split("::").flat_map(|s| s.split('.')).collect();
                        let head = parts[0];
                        if let Some(import) = doc
                            .bindings
                            .iter()
                            .filter(|b| {
                                b.local == head
                                    && b.target.is_some()
                                    && b.start <= r.start_byte
                                    && r.start_byte < b.end
                                    && b.declared <= r.start_byte
                            })
                            .max_by_key(|b| (b.start, b.declared))
                        {
                            let member = parts
                                .iter()
                                .skip(1)
                                .copied()
                                .chain(std::iter::once(r.name.as_str()))
                                .collect::<Vec<_>>()
                                .join("::");
                            candidates = imported(
                                symbols,
                                documents,
                                &doc.path,
                                import.target.as_ref().unwrap(),
                                Some(&member),
                            );
                        } else {
                            candidates = super::inheritance::lookup(
                                symbols,
                                documents,
                                &doc.path,
                                &format!("{ty}::{}", r.name),
                            );
                            if candidates.is_empty() && parts.len() > 1 {
                                candidates = imported(
                                    symbols,
                                    documents,
                                    &doc.path,
                                    &format!("{ty}::{}", r.name),
                                    None,
                                );
                            }
                        }
                        reason = "receiver_type";
                    }
                }
                if binding.target.is_none()
                    && (binding.ty.is_none() || !r.is_call)
                    && segments.len() == 1
                    && binding.declared <= r.start_byte
                {
                    candidates = symbols
                        .iter()
                        .filter(|s| {
                            s.document_id == r.document_id
                                && s.name == head
                                && s.start_byte == binding.declared
                        })
                        .collect();
                    if !candidates.is_empty() {
                        reason = "lexical";
                    }
                }
                // A local unknown binding shadows all similarly named global functions.
                if candidates.is_empty() && reason != "uninitialized_binding" {
                    reason = "shadowed_or_external";
                }
            } else if matches!(head, "self" | "this" | "Self") && segments.len() == 2 {
                if let Some(parent) =
                    caller.and_then(|s| s.qualified_name.rsplit_once("::").map(|p| p.0))
                {
                    candidates = super::inheritance::lookup(
                        symbols,
                        documents,
                        &doc.path,
                        &format!("{parent}::{}", r.name),
                    );
                    reason = "receiver_type";
                }
            } else if segments.len() > 1 {
                candidates =
                    super::inheritance::lookup(symbols, documents, &doc.path, &segments.join("::"));
                reason = "qualified";
                if candidates.is_empty() {
                    let module = segments[..segments.len() - 1].join("::");
                    candidates = imported(
                        symbols,
                        documents,
                        &doc.path,
                        &format!("{module}::{}", r.name),
                        None,
                    );
                    reason = "module_path";
                }
            } else {
                let mut scopes = caller
                    .map(|s| {
                        s.qualified_name
                            .split("::")
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                loop {
                    let name = if scopes.is_empty() {
                        r.name.clone()
                    } else {
                        format!("{}::{}", scopes.join("::"), r.name)
                    };
                    // Type bodies do not introduce bare method bindings in these languages.
                    // A bare work() is not equivalent to self.work()/this.work().
                    let lexical_scope = scopes.is_empty()
                        || symbols.iter().any(|s| {
                            s.path == doc.path
                                && s.qualified_name == scopes.join("::")
                                && !matches!(
                                    s.kind.as_str(),
                                    "class_definition"
                                        | "class_declaration"
                                        | "struct_item"
                                        | "enum_item"
                                        | "trait_item"
                                        | "interface_declaration"
                                )
                        });
                    candidates = if lexical_scope {
                        matching(symbols, &doc.path, &name)
                    } else {
                        vec![]
                    };
                    if !candidates.is_empty() || scopes.is_empty() {
                        break;
                    }
                    scopes.pop();
                }
                if !candidates.is_empty() {
                    reason = "lexical";
                }
            }
            // Do not cross project boundaries or fall back to unrelated same-name files.
            candidates.retain(|s| {
                documents
                    .get(&s.document_id)
                    .is_some_and(|d| d.project == doc.project)
            });
            candidates.sort_by(|a, b| a.id.cmp(&b.id));
            candidates.dedup_by(|a, b| a.id == b.id);
            let resolution = match candidates.len() {
                0 => "unresolved",
                1 => "resolved_static",
                _ => "ambiguous",
            };
            result.push(CallEdge {
                reference: r.clone(),
                candidates: candidates.iter().map(|s| s.id.clone()).collect(),
                resolution: resolution.into(),
                provenance: "derived",
                resolution_basis: reason.into(),
            });
        }
    }
    result.sort_by(|a, b| {
        (&a.reference.document_id, a.reference.start_byte)
            .cmp(&(&b.reference.document_id, b.reference.start_byte))
    });
    result
}

#[cfg(test)]
pub(super) fn calls(
    symbols: &[Symbol],
    documents: &HashMap<String, IndexedDocument>,
) -> Vec<CallEdge> {
    references(symbols, documents)
        .into_iter()
        .filter(|r| r.reference.is_call)
        .collect()
}
