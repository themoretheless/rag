//! PHP route registrations from AST, without inferring facade/handler identity.
use super::*;

fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|n| n.kind() != "comment")
        .collect()
}
fn literal<'a>(node: Node, source: &'a str) -> Option<&'a str> {
    if !matches!(node.kind(), "string" | "encapsed_string") {
        return None;
    }
    let raw = text(node, source);
    let quote = raw.as_bytes().first()?;
    if !matches!(quote, b'\'' | b'"') || raw.as_bytes().last()? != quote {
        return None;
    }
    let value = raw.get(1..raw.len().checked_sub(1)?)?;
    (!value.contains(['\\', '$']) && !value.chars().any(char::is_control)).then_some(value)
}
fn arguments(node: Node) -> Option<Vec<Node>> {
    children(node)
        .into_iter()
        .map(|n| {
            if n.kind() != "argument" || n.child_by_field_name("name").is_some() {
                return None;
            }
            let items = children(n);
            (items.len() == 1).then(|| items[0])
        })
        .collect()
}
fn method_list(node: Node, source: &str) -> Option<Vec<String>> {
    if node.kind() != "array_creation_expression" {
        return None;
    }
    let mut methods = std::collections::BTreeSet::new();
    for item in children(node) {
        let elements = children(item);
        if item.kind() != "array_element_initializer" || elements.len() != 1 {
            return None;
        }
        let method = literal(elements[0], source)?.to_ascii_uppercase();
        if !matches!(
            method.as_str(),
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "TRACE" | "CONNECT"
        ) {
            return None;
        }
        methods.insert(method);
    }
    (!methods.is_empty()).then(|| methods.into_iter().collect())
}
fn handler(node: Option<Node>, source: &str) -> String {
    let Some(node) = node else {
        return "?missing_php_handler".into();
    };
    if let Some(value) = literal(node, source).filter(|v| v.len() <= 512) {
        return value.into();
    }
    if node.kind() == "array_creation_expression" {
        let elements = children(node);
        if elements.len() == 2 {
            let class = children(elements[0]);
            let method = children(elements[1]);
            if class.len() == 1 && method.len() == 1 {
                if let Some(method) = literal(method[0], source) {
                    let class_node = class[0];
                    let class_parts = children(class_node);
                    if class_node.kind() == "class_constant_access_expression"
                        && class_parts.len() == 2
                        && matches!(class_parts[0].kind(), "name" | "qualified_name")
                        && text(class_parts[1], source).eq_ignore_ascii_case("class")
                        && text(class_parts[0], source).len() + method.len() <= 512
                    {
                        return format!("{}::{method}", text(class_parts[0], source));
                    }
                }
            }
        }
    }
    if matches!(node.kind(), "anonymous_function" | "arrow_function") {
        "?anonymous_php_handler".into()
    } else {
        "?dynamic_php_handler".into()
    }
}
pub(super) fn collect(node: Node, source: &str, out: &mut IndexedDocument) {
    let receiver = match node.kind() {
        "scoped_call_expression" => node.child_by_field_name("scope").filter(|n| {
            matches!(n.kind(), "name" | "qualified_name")
                && matches!(
                    text(*n, source)
                        .trim_start_matches('\\')
                        .to_ascii_lowercase()
                        .as_str(),
                    "route" | "router" | "illuminate\\support\\facades\\route"
                )
        }),
        "member_call_expression" => node.child_by_field_name("object").filter(|n| {
            n.kind() == "variable_name"
                && matches!(text(*n, source), "$app" | "$router" | "$server")
        }),
        _ => return,
    };
    if receiver.is_none() {
        return;
    }
    let Some(name) = node
        .child_by_field_name("name")
        .filter(|n| n.kind() == "name")
    else {
        return;
    };
    let name = text(name, source).to_ascii_lowercase();
    let method = match name.as_str() {
        "get" => "GET",
        "post" => "POST",
        "put" => "PUT",
        "patch" => "PATCH",
        "delete" => "DELETE",
        "head" => "HEAD",
        "options" => "OPTIONS",
        "trace" => "TRACE",
        "connect" => "CONNECT",
        "any" | "all" | "match" => "ANY",
        _ => return,
    };
    let Some(args) = node.child_by_field_name("arguments").and_then(arguments) else {
        return;
    };
    let path_index = usize::from(name == "match");
    let Some(path) = args
        .get(path_index)
        .and_then(|n| literal(*n, source))
        .filter(|s| s.starts_with('/') && !s.starts_with("//") && s.len() <= 4096)
    else {
        return;
    };
    let methods = if name == "match" {
        args.first()
            .and_then(|n| method_list(*n, source))
            .unwrap_or_else(|| vec!["ANY".into()])
    } else {
        vec![method.into()]
    };
    let handler = handler(args.get(path_index + 1).copied(), source);
    for method in methods {
        out.routes.push(analysis::RouteSpec {
            handler: handler.clone(),
            binding_unverified: true,
            route: path.into(),
            method,
            line: node.start_position().row + 1,
            start: node.start_byte(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn php_static_member_methods_and_match_arrays_are_extracted() {
        let doc = parse("a", "/p/routes.php", "<?php Route::get('/read', 'show'); $app->post('/write', function () { return 1; }); Route::match(['POST', 'get', 'GET'], '/multi', [UserController::class, 'store']); \\Illuminate\\Support\\Facades\\Route::delete('/remove', 'remove');", 1).unwrap();
        assert!(!doc.parse_errors);
        assert_eq!(doc.routes.len(), 5, "{:?}", doc.routes);
        assert!(doc.routes.iter().any(|r| r.route == "/multi"
            && r.method == "GET"
            && r.handler == "UserController::store"));
        assert!(doc.routes.iter().all(|r| r.binding_unverified));
        assert!(doc
            .routes
            .iter()
            .any(|r| r.route == "/write" && r.handler == "?anonymous_php_handler"));
    }
    #[test]
    fn php_comments_strings_dynamic_paths_and_unrelated_receivers_are_excluded() {
        let doc = parse("a", "/p/routes.php", "<?php // Route::get('/comment', 'show');\n$s = \"Route::get('/string')\"; Other::get('/other'); $client->get('/client'); Route::get('/prefix' . $id, 'show'); Route::get(\"/users/$id\", 'show'); Route::$verb('/dynamic'); Route::get(uri:'/named', action:'show');", 1).unwrap();
        assert!(doc.routes.is_empty(), "{:?}", doc.routes);
    }
    #[test]
    fn php_unknown_method_lists_preserve_unknown_candidates() {
        let doc = parse("a", "/p/routes.php", "<?php Route::match($verbs, '/dynamic', $handler); Route::match(['GET', ...$verbs], '/spread', $handler); Route::match([], '/empty', 'show'); Route::any('/any', 'show');", 1).unwrap();
        assert_eq!(doc.routes.len(), 4, "{:?}", doc.routes);
        assert!(doc.routes.iter().all(|r| r.method == "ANY"));
    }
}
