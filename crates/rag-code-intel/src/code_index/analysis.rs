//! Syntactic entrypoints/routes, conservative reachability and structural call slices.
use super::*;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct RouteSpec {
    pub handler: String,
    #[serde(default)]
    pub binding_unverified: bool,
    pub route: String,
    pub method: String,
    pub line: usize,
    #[serde(default)]
    pub start: usize,
}
#[derive(Debug, Serialize)]
pub struct EntryPoint {
    pub document_id: String,
    pub start_byte: usize,
    pub project: Option<String>,
    pub revision: i64,
    pub content_hash: String,
    pub symbol_id: Option<String>,
    pub resolution: String,
    pub handler_expression: Option<String>,
    pub kind: String,
    pub route: Option<String>,
    pub method: Option<String>,
    pub path: String,
    pub line: usize,
    pub provenance: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_method_basis: Option<String>,
}
fn route_literal(node: Node, source: &str) -> Option<String> {
    if !matches!(
        node.kind(),
        "string" | "string_literal" | "raw_string_literal"
    ) {
        return None;
    }
    let s = text(node, source);
    let start = s.find(['\'', '"'])?;
    let quote = s.as_bytes()[start] as char;
    if s[..start].contains(['f', 'b']) {
        return None;
    }
    let end = s.rfind(quote)?;
    if end <= start {
        return None;
    }
    let value = &s[start + 1..end];
    if value.starts_with('/') && !value.contains(['$', '\\']) {
        Some(value.into())
    } else {
        None
    }
}
fn function_name(node: Node, source: &str) -> Option<String> {
    if matches!(
        node.kind(),
        "identifier" | "scoped_identifier" | "attribute" | "member_expression"
    ) {
        Some(text(node, source).replace('.', "::"))
    } else {
        None
    }
}
// Only literal Python decorator method collections are trusted. Dynamic members,
// splats and invalid/empty collections retain the conservative ANY candidate.
fn decorator_methods(args: &[Node], source: &str) -> Option<Vec<String>> {
    let keywords: Vec<_> = args
        .iter()
        .filter(|n| {
            n.kind() == "keyword_argument"
                && n.child_by_field_name("name")
                    .is_some_and(|n| text(n, source) == "methods")
        })
        .collect();
    if keywords.len() != 1 {
        return None;
    }
    let value = keywords[0].child_by_field_name("value")?;
    if !matches!(value.kind(), "list" | "tuple" | "set") {
        return None;
    }
    let mut cursor = value.walk();
    let mut methods = std::collections::BTreeSet::new();
    for item in value.named_children(&mut cursor) {
        if item.kind() == "comment" {
            continue;
        }
        if item.kind() != "string" {
            return None;
        }
        let raw = text(item, source);
        // No interpolation, escapes, concatenation or prefix-dependent decoding.
        let quote = raw.as_bytes().first().copied()?;
        if !matches!(quote, b'\'' | b'"') || raw.as_bytes().last() != Some(&quote) {
            return None;
        }
        let method = raw.get(1..raw.len().checked_sub(1)?)?.to_ascii_uppercase();
        if !matches!(
            method.as_str(),
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "TRACE" | "CONNECT"
        ) {
            return None;
        }
        methods.insert(method);
        if methods.len() > 16 {
            return None;
        }
    }
    (!methods.is_empty()).then(|| methods.into_iter().collect())
}
fn collect_route(node: Node, source: &str, handler: Option<&str>, out: &mut IndexedDocument) {
    let Some(function) = node.child_by_field_name("function") else {
        return;
    };
    // Known client-name syntax is not also an Express-style registration.
    // This exclusion does not prove that the client identifier is library-bound.
    if handler.is_none()
        && function.kind() == "member_expression"
        && function.child_by_field_name("object").is_some_and(|n| {
            n.kind() == "identifier" && matches!(text(n, source), "$" | "jQuery" | "axios")
        })
    {
        return;
    }
    let Some(name) = last_identifier(function).map(|n| text(n, source)) else {
        return;
    };
    let Some(args) = node.child_by_field_name("arguments") else {
        return;
    };
    let mut cursor = args.walk();
    let args: Vec<_> = args.named_children(&mut cursor).collect();
    let Some(path) = args.first().and_then(|n| route_literal(*n, source)) else {
        return;
    };
    let method = match name {
        "get" => "GET",
        "post" => "POST",
        "put" => "PUT",
        "delete" => "DELETE",
        "patch" => "PATCH",
        "head" => "HEAD",
        "options" => "OPTIONS",
        "all" | "route" => "ANY",
        "api_route" if handler.is_some() => "ANY",
        _ => return,
    };
    let line = node.start_position().row + 1;
    if let Some(handler) = handler {
        let methods = if matches!(name, "route" | "api_route") {
            decorator_methods(&args, source).unwrap_or_else(|| vec!["ANY".into()])
        } else {
            vec![method.into()]
        };
        for method in methods {
            out.routes.push(RouteSpec {
                binding_unverified: false,
                handler: handler.into(),
                route: path.clone(),
                method,
                line,
                start: node.start_byte(),
            });
        }
        return;
    }
    // Express-style registration with a named handler, or Axum .route(path,get(handler)).
    let before = out.routes.len();
    for arg in args.iter().skip(1) {
        if let Some(handler) = function_name(*arg, source) {
            out.routes.push(RouteSpec {
                binding_unverified: false,
                handler,
                route: path.clone(),
                method: method.into(),
                line,
                start: node.start_byte(),
            });
        } else if name == "route" && arg.kind() == "call_expression" {
            if let (Some(f), Some(a)) = (
                arg.child_by_field_name("function"),
                arg.child_by_field_name("arguments"),
            ) {
                let verb = last_identifier(f).map(|n| text(n, source)).unwrap_or("");
                let verb = match verb {
                    "get" => "GET",
                    "post" => "POST",
                    "put" => "PUT",
                    "delete" => "DELETE",
                    "patch" => "PATCH",
                    _ => continue,
                };
                if let Some(handler) = a.named_child(0).and_then(|n| function_name(n, source)) {
                    out.routes.push(RouteSpec {
                        binding_unverified: false,
                        handler,
                        route: path.clone(),
                        method: verb.into(),
                        line,
                        start: node.start_byte(),
                    });
                }
            }
        }
    }
    if out.routes.len() == before && args.len() > 1 {
        out.routes.push(RouteSpec {
            binding_unverified: false,
            handler: "?anonymous_or_dynamic_handler".into(),
            route: path,
            method: method.into(),
            line,
            start: node.start_byte(),
        });
    }
}
pub(super) fn collect(node: Node, source: &str, out: &mut IndexedDocument) {
    if matches!(out.path.rsplit('.').next(), Some("php" | "phtml")) {
        super::php_routes::collect(node, source, out);
    }
    if node.kind() == "function_item" {
        let mut previous = node.prev_named_sibling();
        while let Some(attribute) = previous {
            if matches!(attribute.kind(), "line_comment" | "block_comment") {
                previous = attribute.prev_named_sibling();
                continue;
            }
            if attribute.kind() != "attribute_item" {
                break;
            }
            let name = attribute
                .named_child(0)
                .and_then(|n| n.named_child(0))
                .map(|n| text(n, source).split_whitespace().collect::<String>());
            if name.as_deref().is_some_and(|n| {
                matches!(
                    n.trim_start_matches("::"),
                    "test" | "tokio::test" | "async_std::test"
                )
            }) {
                out.test_definitions.insert(node.start_byte());
            }
            if name.as_deref() == Some("ignore") {
                out.ignored_test_definitions.insert(node.start_byte());
            }
            previous = attribute.prev_named_sibling();
        }
    }
    if node.kind() == "function_definition" {
        if let (Some(parent), Some(name)) = (node.parent(), node.child_by_field_name("name")) {
            if parent.kind() == "decorated_definition" {
                let mut cursor = parent.walk();
                for decorator in parent
                    .named_children(&mut cursor)
                    .filter(|n| n.kind() == "decorator")
                {
                    if let Some(call) = decorator.named_child(0) {
                        // Match the actual handler symbol by source span; never a same-name sibling.
                        collect_route(
                            call,
                            source,
                            Some(&format!("@{}:{}", node.start_byte(), text(name, source))),
                            out,
                        );
                    }
                }
            }
        }
    }
    if matches!(node.kind(), "call_expression" | "call") {
        // Python decorators are handled once with the definition's exact span.
        // Generic registration would turn keyword arguments into fake handlers.
        let mut parent = node.parent();
        while let Some(ancestor) = parent {
            if ancestor.kind() == "decorator" {
                return;
            }
            if matches!(ancestor.kind(), "block" | "module") {
                break;
            }
            parent = ancestor.parent();
        }
        collect_route(node, source, None, out);
    }
}
pub(super) fn entrypoints(
    symbols: &[Symbol],
    docs: &HashMap<String, IndexedDocument>,
) -> Vec<EntryPoint> {
    let mut entries = Vec::new();
    for (id, doc) in docs {
        let exports: HashSet<_> = doc.exports.values().map(String::as_str).collect();
        for symbol in symbols.iter().filter(|s| s.document_id == *id) {
            let owner = symbol.qualified_name.split("::").next().unwrap_or("");
            let kind = if doc.powershell.is_some() {
                super::powershell::entrypoint(doc, symbol)
            } else if doc.perl.is_some() {
                super::perl::entrypoint(doc, symbol)
            } else if doc.csharp.is_some() {
                super::csharp::entrypoint(doc, symbol)
            } else if doc.ruby.is_some() {
                super::ruby::entrypoint(doc, symbol)
            } else if doc.shell.is_some() {
                super::shell::entrypoint(doc, symbol)
            } else if doc.c_family.is_some() {
                super::c_family::entrypoint(doc, symbol)
            } else if doc.java.is_some() {
                super::java::entrypoint(doc, symbol)
            } else if doc.path.ends_with(".go") {
                super::go::entrypoint(doc, symbol)
            } else if doc.test_definitions.contains(&symbol.start_byte) {
                Some("test_function")
            } else if symbol.name == "main" && !symbol.qualified_name.contains("::") {
                Some("conventional_main")
            } else if exports.contains(symbol.qualified_name.as_str()) || exports.contains(owner) {
                Some("exported_api")
            } else if symbol.signature.starts_with("pub fn ")
                || symbol.signature.starts_with("pub async fn ")
            {
                Some("public_api")
            } else if symbol.name.starts_with("test_") && doc.path.ends_with(".py") {
                Some("test_function")
            } else {
                None
            };
            if let Some(kind) = kind {
                entries.push(EntryPoint {
                    document_id: id.clone(),
                    project: doc.project.clone(),
                    revision: doc.revision,
                    content_hash: doc.hash.clone(),
                    start_byte: symbol.start_byte,
                    symbol_id: Some(symbol.id.clone()),
                    resolution: "resolved_static".into(),
                    handler_expression: None,
                    kind: kind.into(),
                    route: None,
                    method: None,
                    path: symbol.path.clone(),
                    line: symbol.start_line,
                    provenance: "derived",
                    reference_kind: None,
                    request_method: None,
                    request_method_basis: None,
                });
            }
        }
        for endpoint in &doc.endpoints {
            entries.push(EntryPoint {
                document_id: id.clone(),
                project: doc.project.clone(),
                revision: doc.revision,
                content_hash: doc.hash.clone(),
                start_byte: endpoint.start_byte,
                symbol_id: None,
                resolution: "unbound_reference".into(),
                handler_expression: None,
                kind: "http_endpoint_reference".into(),
                route: Some(endpoint.target.clone()),
                method: None,
                path: doc.path.clone(),
                line: endpoint.line,
                provenance: "syntax",
                reference_kind: Some(endpoint.reference_kind.clone()),
                request_method: endpoint.request_method.clone(),
                request_method_basis: endpoint.request_method_basis.clone(),
            });
        }
        for route in &doc.routes {
            let mut targets: Vec<_> = if route.binding_unverified {
                vec![]
            } else if let Some(span) = route.handler.strip_prefix('@') {
                let start = span.split(':').next().and_then(|s| s.parse::<usize>().ok());
                symbols
                    .iter()
                    .filter(|s| s.document_id == *id && Some(s.start_byte) == start)
                    .collect()
            } else {
                let parts: Vec<_> = route.handler.split("::").collect();
                if let Some(target) = resolve::import_target(doc, parts[0], route.start) {
                    resolve::imported(
                        symbols,
                        docs,
                        &doc.path,
                        target,
                        if parts.len() > 1 {
                            Some(parts[1])
                        } else {
                            None
                        },
                    )
                } else {
                    resolve::matching(symbols, &doc.path, &route.handler)
                }
            };
            targets.retain(|s| {
                docs.get(&s.document_id)
                    .is_some_and(|d| d.project == doc.project)
            });
            if targets.is_empty() {
                entries.push(EntryPoint {
                    document_id: id.clone(),
                    project: doc.project.clone(),
                    revision: doc.revision,
                    content_hash: doc.hash.clone(),
                    start_byte: route.start,
                    symbol_id: None,
                    resolution: "unresolved".into(),
                    handler_expression: Some(route.handler.clone()),
                    kind: "http_route_candidate".into(),
                    route: Some(route.route.clone()),
                    method: Some(route.method.clone()),
                    path: doc.path.clone(),
                    line: route.line,
                    provenance: "derived",
                    reference_kind: None,
                    request_method: None,
                    request_method_basis: None,
                });
            }
            let resolution = if targets.len() == 1 {
                "resolved_static"
            } else {
                "ambiguous"
            };
            for symbol in targets {
                entries.push(EntryPoint {
                    document_id: id.clone(),
                    project: doc.project.clone(),
                    revision: doc.revision,
                    content_hash: doc.hash.clone(),
                    start_byte: route.start,
                    symbol_id: Some(symbol.id.clone()),
                    resolution: resolution.into(),
                    handler_expression: Some(route.handler.clone()),
                    kind: "http_route_candidate".into(),
                    route: Some(route.route.clone()),
                    method: Some(route.method.clone()),
                    path: doc.path.clone(),
                    line: route.line,
                    provenance: "derived",
                    reference_kind: None,
                    request_method: None,
                    request_method_basis: None,
                });
            }
        }
    }
    entries.sort_by(|a, b| {
        (
            &a.path,
            &a.document_id,
            a.line,
            a.start_byte,
            &a.symbol_id,
            &a.route,
            &a.method,
            &a.reference_kind,
            &a.request_method,
        )
            .cmp(&(
                &b.path,
                &b.document_id,
                b.line,
                b.start_byte,
                &b.symbol_id,
                &b.route,
                &b.method,
                &b.reference_kind,
                &b.request_method,
            ))
    });
    entries.dedup_by(|a, b| {
        a.document_id == b.document_id
            && a.start_byte == b.start_byte
            && a.symbol_id == b.symbol_id
            && a.kind == b.kind
            && a.route == b.route
            && a.method == b.method
            && a.reference_kind == b.reference_kind
            && a.request_method == b.request_method
            && a.path == b.path
            && a.line == b.line
    });
    entries
}
pub(super) fn callable(s: &Symbol) -> bool {
    matches!(
        s.kind.as_str(),
        "function_statement"
            | "class_method_definition"
            | "subroutine_declaration_statement"
            | "method_declaration_statement"
            | "anonymous_subroutine_expression"
            | "anonymous_method_expression"
            | "phaser_statement"
            | "local_function_statement"
            | "destructor_declaration"
            | "operator_declaration"
            | "method"
            | "singleton_method"
            | "function_item"
            | "function_definition"
            | "function_declaration"
            | "generator_function_declaration"
            | "method_definition"
            | "method_declaration"
            | "constructor_declaration"
            | "compact_constructor_declaration"
            | "variable_declarator"
    )
}
impl CodeSnapshot {
    pub(super) fn analysis_query(&self, op: &str, q: &CodeQuery) -> Option<serde_json::Value> {
        if !matches!(op, "entrypoints" | "routes" | "dead_code" | "slice") {
            return None;
        }
        if op == "dead_code" && q.dead_code_view.as_deref().is_some_and(|v| v != "routines") {
            return Some(
                serde_json::json!({"error":"file dead-code view requires Store::code_dead_files; allowed views are routines and files","items":[]}),
            );
        }
        let limit = q.limit.unwrap_or(50).clamp(1, 500);
        let selected: HashSet<_> = self
            .symbols
            .iter()
            .filter(|s| {
                q.path.as_ref().is_none_or(|p| p == &s.path)
                    && q.symbol
                        .as_ref()
                        .is_none_or(|n| n == &s.id || n == &s.name || n == &s.qualified_name)
            })
            .map(|s| s.id.as_str())
            .collect();
        let incomplete = self.binding_diagnostics.iter().any(|d| {
            matches!(
                d["code"].as_str(),
                Some(
                    "LUA_TABLE_IMPORT_ENV_AND_RUNTIME_BINDING_UNVERIFIED"
                        | "RUBY_RUNTIME_MIXINS_VISIBILITY_METAPROGRAMMING_AND_LOAD_ORDER_UNVERIFIED"
                        | "CSHARP_BUILD_OVERLOAD_RUNTIME_SEMANTICS_UNVERIFIED"
                        | "PERL_INC_EXPORTS_SYMBOL_TABLE_MRO_AND_RUNTIME_UNVERIFIED"
                        | "POWERSHELL_SESSION_ALIAS_SCOPE_EXPORT_AND_RUNTIME_UNVERIFIED"
                )
            )
        }) || !self.stale_documents.is_empty()
            || !self.unindexed_documents.is_empty()
            || !self.parse_errors.is_empty()
            || self.calls.iter().any(|c| c.resolution != "resolved_static")
            || self.inheritance.iter().any(|e| {
                e.resolution != "resolved_static" || e.hierarchy_status != "complete_static"
            });
        let (items, total, truncated, extra) = if matches!(op, "entrypoints" | "routes") {
            let entries: Vec<_> = self
                .entrypoints
                .iter()
                .filter(|e| {
                    if op == "routes" {
                        e.route.is_some()
                            && q.path.as_ref().is_none_or(|p| p == &e.path)
                            && (q.symbol.is_none()
                                || e.symbol_id
                                    .as_deref()
                                    .is_some_and(|id| selected.contains(id)))
                    } else {
                        e.symbol_id
                            .as_deref()
                            .is_some_and(|id| selected.contains(id))
                    }
                })
                .collect();
            (
                entries
                    .iter()
                    .take(limit)
                    .map(|e| serde_json::to_value(e).unwrap())
                    .collect::<Vec<_>>(),
                entries.len(),
                entries.len() > limit,
                serde_json::json!({
                    "route_semantics":"static_registrations_and_separate_markup_url_references",
                    "method_semantics":"explicit_syntax_or_ANY_unknown",
                    "implicit_head_options_inferred":false,
                    "framework_identity_verified":false,
                    "runtime_registration_verified":false,
                    "endpoint_references_prove_handler":false,
                    "endpoint_targets":"root_relative_literal_markup_fetch_jquery_axios_urls",
                    "fetch_identity_verified":false,
                    "client_library_identity_verified":false,
                    "client_method_alias_semantics":"name_shaped_candidate_not_runtime_method",
                    "client_defaults_interceptors_and_ajax_setup_resolved":false,
                    "fetch_default_method_inferred":false,
                    "php_handler_binding_verified":false,
                    "route_path_semantics":"declared_literal_not_effective_mount_path",
                    "php_group_prefix_resolved":false,
                    "endpoint_urls_fetched":false
                }),
            )
        } else if op == "dead_code" {
            dead_code::candidates(self, &selected, limit)
        } else {
            let direction = q.direction.as_deref().unwrap_or("back");
            if !matches!(direction, "back" | "forward" | "both") {
                return Some(
                    serde_json::json!({"error":"INVALID_DIRECTION","allowed":["back","forward","both"]}),
                );
            }
            if q.symbol.is_none() {
                return Some(serde_json::json!({"error":"SYMBOL_REQUIRED"}));
            }
            let mut seeds: Vec<_> = selected.iter().copied().collect();
            seeds.sort_unstable();
            let mut ids: HashSet<String> =
                seeds.into_iter().take(limit).map(str::to_string).collect();
            let mut frontier = ids.clone();
            let mut truncated = selected.len() > limit;
            for _ in 0..q.depth.unwrap_or(3).clamp(1, 10) {
                let mut next = HashSet::new();
                for call in &self.calls {
                    let Some(caller) = &call.reference.caller else {
                        continue;
                    };
                    for target in &call.candidates {
                        let edge = if matches!(direction, "forward" | "both")
                            && frontier.contains(caller)
                        {
                            Some(target)
                        } else if matches!(direction, "back" | "both") && frontier.contains(target)
                        {
                            Some(caller)
                        } else {
                            None
                        };
                        if let Some(edge) = edge {
                            if !ids.contains(edge) {
                                if ids.len() < limit {
                                    ids.insert(edge.clone());
                                    next.insert(edge.clone());
                                } else {
                                    truncated = true;
                                }
                            }
                        }
                    }
                }
                if next.is_empty() {
                    break;
                }
                frontier = next;
            }
            let nodes: Vec<_> = self
                .symbols
                .iter()
                .filter(|s| ids.contains(&s.id))
                .collect();
            let mut edges = Vec::new();
            let mut edges_total = 0;
            let mut depth_limited = false;
            for call in &self.calls {
                let Some(caller) = &call.reference.caller else {
                    continue;
                };
                for target in &call.candidates {
                    if (matches!(direction, "forward" | "both")
                        && ids.contains(caller)
                        && !ids.contains(target))
                        || (matches!(direction, "back" | "both")
                            && ids.contains(target)
                            && !ids.contains(caller))
                    {
                        depth_limited = true;
                    }
                    if ids.contains(caller) && ids.contains(target) {
                        edges_total += 1;
                        if edges.len() < 2000 {
                            edges.push(serde_json::json!({"source":caller,"target":target,"path":call.reference.path,"line":call.reference.line,"provenance":call.provenance,"resolution":call.resolution}));
                        }
                    }
                }
            }
            let items=nodes.iter().map(|s|serde_json::json!({"id":s.id,"name":s.qualified_name,"path":s.path,"start_line":s.start_line,"end_line":s.end_line})).collect();
            (
                items,
                nodes.len(),
                truncated || depth_limited || edges_total > 2000,
                serde_json::json!({"edges":edges,"direction":direction,"structural_only":true,"edge_limit":2000,"edges_total":edges_total,"edges_truncated":edges_total>2000,"depth_limited":depth_limited}),
            )
        };
        let incomplete = incomplete
            || (op == "dead_code" && extra["unresolved_name_roots"].as_u64().unwrap_or(0) > 0);
        Some(
            serde_json::json!({"items":items,"total":total,"truncated":truncated,"detail":extra,"analysis_incomplete":incomplete,"static_analysis_only":true,"stale_documents":self.stale_documents,"unindexed_documents":self.unindexed_documents,"parse_errors":self.parse_errors}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(files: &[(&str, &str)]) -> CodeSnapshot {
        let mut docs = HashMap::new();
        let mut out = CodeSnapshot::default();
        for (i, (path, source)) in files.iter().enumerate() {
            let doc = parse(&i.to_string(), path, source, 1).unwrap();
            assert!(!doc.parse_errors, "{path}");
            out.symbols.extend(doc.symbols.clone());
            out.references.extend(doc.references.clone());
            docs.insert(i.to_string(), doc);
        }
        out.bound_references = resolve::references(&out.symbols, &docs);
        out.calls = out
            .bound_references
            .iter()
            .filter(|r| r.reference.is_call)
            .cloned()
            .collect();
        out.inheritance = inheritance::edges(&out.symbols, &docs);
        out.entrypoints = entrypoints(&out.symbols, &docs);
        out
    }
    #[test]
    fn routes_retain_decorators_imported_handlers_and_unknown_handlers() {
        let snap=snapshot(&[
            ("/p/main.py","@app.get('/users/{id}')\ndef user():\n    return 1\n"),
            ("/p/main.js","import {handler as serve} from './handlers'; app.get('/health',serve); app.post('/unknown',missing); app.get('/anon', () => 'body');"),
            ("/p/handlers.js","export function handler() {}"),
            ("/p/src/main.rs","fn handler() {} fn main() { let app = Router::new().route(r#\"/rust\"#, get(handler)); }"),
        ]);
        let routes = snap.query("routes", &CodeQuery::default());
        let items = routes["items"].as_array().unwrap();
        assert_eq!(items.len(), 5, "{routes}");
        assert!(items
            .iter()
            .any(|r| r["route"] == "/users/{id}" && r["resolution"] == "resolved_static"));
        assert!(
            items
                .iter()
                .any(|r| r["route"] == "/health"
                    && r["symbol_id"].as_str().unwrap().starts_with("2:"))
        );
        assert!(items
            .iter()
            .any(|r| r["route"] == "/unknown" && r["symbol_id"].is_null()));
        assert!(items
            .iter()
            .any(|r| r["route"] == "/anon" && r["resolution"] == "unresolved"));
        assert!(items
            .iter()
            .any(|r| r["route"] == "/rust" && r["method"] == "GET"));
        let scoped = snap.query(
            "routes",
            &CodeQuery {
                path: Some("/p/main.js".into()),
                ..Default::default()
            },
        );
        assert_eq!(scoped["total"], 3);
        let roots = snap.query("entrypoints", &CodeQuery::default());
        assert!(roots["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["symbol_id"].is_string()));
    }
    #[test]
    fn python_decorator_methods_are_literal_deduplicated_and_span_bound() {
        let snap = snapshot(&[("/p/routes.py", "@app.route('/multi', methods=['post', 'GET', 'GET'])\ndef handler(): pass\n@app.api_route('/api', methods=('PATCH', 'DELETE'))\ndef handler(): pass\n@app.route('/dynamic', methods=['GET', verb])\ndef dynamic(): pass\n@app.route('/empty', methods=[])\ndef empty(): pass\n@app.route('/escaped', methods=['G\\x45T'])\ndef escaped(): pass\n@app.get('/fixed', response_model=Result)\ndef fixed(): pass\n")]);
        let routes = snap.query("routes", &CodeQuery::default());
        let items = routes["items"].as_array().unwrap();
        assert_eq!(items.len(), 8, "{routes}");
        let methods = |path: &str| -> Vec<&str> {
            items
                .iter()
                .filter(|r| r["route"] == path)
                .map(|r| r["method"].as_str().unwrap())
                .collect()
        };
        assert_eq!(methods("/multi"), vec!["GET", "POST"]);
        assert_eq!(methods("/api"), vec!["DELETE", "PATCH"]);
        for path in ["/dynamic", "/empty", "/escaped"] {
            assert_eq!(methods(path), vec!["ANY"]);
        }
        assert_eq!(methods("/fixed"), vec!["GET"]);
        assert!(
            items.iter().all(|r| r["resolution"] == "resolved_static"),
            "{routes}"
        );
        let first = items.iter().find(|r| r["route"] == "/multi").unwrap();
        let second = items.iter().find(|r| r["route"] == "/api").unwrap();
        assert_ne!(first["symbol_id"], second["symbol_id"]);
    }
    #[test]
    fn python_route_methods_do_not_partially_trust_dynamic_collections() {
        let snap = snapshot(&[("/p/routes.py", "@app.api_route('/set', methods={'post', 'GET'})\ndef a(): pass\n@app.route('/splat', methods=['GET', *extra])\ndef b(): pass\n@app.route('/scalar', methods='POST')\ndef c(): pass\n@app.route('/invalid', methods=['GET', 'BOGUS'])\ndef d(): pass\n@app.route('/fstring', methods=[f'GET'])\ndef e(): pass\n@app.route('/concat', methods=['G' 'ET'])\ndef f(): pass\n@app.route('/default')\ndef g(): pass\n")]);
        let result = snap.query("routes", &CodeQuery::default());
        let items = result["items"].as_array().unwrap();
        assert_eq!(items.len(), 8, "{result}");
        for path in [
            "/splat", "/scalar", "/invalid", "/fstring", "/concat", "/default",
        ] {
            let rows: Vec<_> = items.iter().filter(|r| r["route"] == path).collect();
            assert_eq!(rows.len(), 1, "{result}");
            assert_eq!(rows[0]["method"], "ANY", "{result}");
        }
        assert_eq!(result["detail"]["runtime_registration_verified"], false);
    }
    #[test]
    fn markup_endpoints_do_not_bind_handlers_or_create_liveness_roots() {
        let snap = snapshot(&[
            (
                "/p/view.html",
                "<form action='/save'><input formaction='/other'></form><img src='/save'>",
            ),
            ("/p/api.py", "def save(): pass\n"),
        ]);
        let result = snap.query("routes", &CodeQuery::default());
        assert_eq!(result["total"], 3, "{result}");
        assert!(result["items"].as_array().unwrap().iter().all(|r| r["kind"]
            == "http_endpoint_reference"
            && r["symbol_id"].is_null()
            && r["method"].is_null()
            && r["resolution"] == "unbound_reference"));
        let roots = snap.query("entrypoints", &CodeQuery::default());
        assert_eq!(roots["total"], 0, "{roots}");
        let dead = snap.query("dead_code", &CodeQuery::default());
        assert_eq!(dead["total"], 1, "{dead}");
        let scoped = snap.query(
            "routes",
            &CodeQuery {
                path: Some("/p/api.py".into()),
                ..Default::default()
            },
        );
        assert_eq!(scoped["total"], 0);
    }
    #[test]
    fn markup_occurrences_and_same_path_document_identities_are_distinct() {
        let snap = snapshot(&[
            ("/p/a.html", "<img src='/same'><img src='/same'>"),
            ("/p/a.html", "<img src='/same'>"),
        ]);
        let result = snap.query("routes", &CodeQuery::default());
        assert_eq!(result["total"], 3, "{result}");
        let ids: HashSet<_> = result["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["document_id"].as_str().unwrap(),
                    r["start_byte"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(ids.len(), 3);
    }
    #[test]
    fn php_route_inventory_does_not_guess_handler_bindings() {
        let snap = snapshot(&[("/p/routes.php", "<?php function show() { return 1; } Route::get('/read', 'show'); Route::match(['GET','POST'], '/multi', [Controller::class, 'save']);")]);
        let routes = snap.query("routes", &CodeQuery::default());
        assert_eq!(routes["total"], 3, "{routes}");
        assert!(routes["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["symbol_id"].is_null() && r["resolution"] == "unresolved"));
        assert_eq!(routes["detail"]["php_handler_binding_verified"], false);
        let roots = snap.query("entrypoints", &CodeQuery::default());
        assert_eq!(roots["total"], 0, "{roots}");
        let dead = snap.query("dead_code", &CodeQuery::default());
        assert_eq!(dead["total"], 1, "{dead}");
    }
    #[test]
    fn reachability_dead_candidates_and_callbacks() {
        let snap=snapshot(&[("/p/a.rs","fn main() { live(); let f=callback; } fn live() { nested(); } fn nested() {} fn unused() {} fn callback() {} #[test] fn verifies_behavior() { nested(); }")]);
        let dead = snap.query("dead_code", &CodeQuery::default());
        let items = dead["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "{dead}");
        assert_eq!(items[0]["symbol"]["name"], "unused");
        assert_eq!(items[0]["deletion_safe"], false);
        assert_eq!(dead["detail"]["external_usage_unknown"], true);
    }
    #[test]
    fn structural_slices_are_bounded_directional_deterministic_and_body_free() {
        let snap = snapshot(&[(
            "/p/a.rs",
            "fn main() { a(); } fn a() { b(); } fn b() { c(); } fn c() { a(); let secret_body=1; }",
        )]);
        let q = CodeQuery {
            symbol: Some("a".into()),
            direction: Some("forward".into()),
            depth: Some(1),
            ..Default::default()
        };
        let slice = snap.query("slice", &q);
        assert_eq!(slice["items"].as_array().unwrap().len(), 2);
        assert_eq!(slice["detail"]["depth_limited"], true);
        assert!(!slice.to_string().contains("secret_body"));
        let back = snap.query(
            "slice",
            &CodeQuery {
                direction: Some("back".into()),
                depth: Some(1),
                ..q
            },
        );
        assert_eq!(back["items"].as_array().unwrap().len(), 3);
        let q = CodeQuery {
            symbol: Some("a".into()),
            direction: Some("both".into()),
            limit: Some(1),
            ..Default::default()
        };
        let bounded = snap.query("slice", &q);
        assert_eq!(bounded["items"].as_array().unwrap().len(), 1);
        assert_eq!(bounded["truncated"], true);
        assert_eq!(bounded, snap.query("slice", &q));
        assert_eq!(
            snap.query("slice", &CodeQuery::default())["error"],
            "SYMBOL_REQUIRED"
        );
        assert_eq!(
            snap.query(
                "slice",
                &CodeQuery {
                    symbol: Some("a".into()),
                    direction: Some("bad".into()),
                    ..Default::default()
                }
            )["error"],
            "INVALID_DIRECTION"
        );
    }
}
