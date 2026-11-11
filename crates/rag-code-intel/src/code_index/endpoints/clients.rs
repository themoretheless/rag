//! Name-shaped client syntax only. Never resolves libraries, defaults or interceptors.
use super::*;
#[derive(Default)]
struct Options {
    url: Option<String>,
    url_seen: bool,
    url_unknown: bool,
    method: Option<String>,
    method_unknown: bool,
    method_keys: std::collections::HashSet<String>,
}
fn method(value: &str) -> Option<String> {
    let value = value.to_ascii_uppercase();
    matches!(
        value.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS" | "TRACE" | "CONNECT"
    )
    .then_some(value)
}
fn options(node: Node, source: &str, jquery: bool) -> Options {
    let mut out = Options::default();
    if node.kind() != "object" {
        out.url_unknown = true;
        out.method_unknown = true;
        return out;
    }
    let mut cursor = node.walk();
    for property in node.named_children(&mut cursor) {
        if property.kind() == "comment" {
            continue;
        }
        let key = property.child_by_field_name("key").and_then(|k| {
            if k.kind() == "property_identifier" {
                Some(text(k, source))
            } else {
                plain_js_string(k, source)
            }
        });
        let Some(key) = key else {
            out.url_unknown = true;
            out.method_unknown = true;
            continue;
        };
        // A known unrelated method/getter cannot replace url/method. A relevant
        // getter/shorthand is not a literal pair and remains unverified.
        let value = (property.kind() == "pair")
            .then(|| property.child_by_field_name("value"))
            .flatten()
            .and_then(|v| plain_js_string(v, source));
        if key == "url" {
            if out.url_seen {
                out.url_unknown = true;
            }
            out.url_seen = true;
            out.url = value.map(str::to_string);
            out.url_unknown |= value.is_none();
        }
        if key == "method" || jquery && key == "type" {
            if !out.method_keys.insert(key.into()) {
                out.method_unknown = true;
            }
            let parsed = value.and_then(method);
            if parsed.is_none()
                || out
                    .method
                    .as_ref()
                    .zip(parsed.as_ref())
                    .is_some_and(|(a, b)| a != b)
            {
                out.method_unknown = true;
            }
            if parsed.is_some() {
                out.method = parsed;
            }
        }
    }
    out
}
fn member<'a>(node: Node, source: &'a str) -> Option<(&'a str, &'a str)> {
    if node.kind() != "member_expression" {
        return None;
    }
    let object = node.child_by_field_name("object")?;
    let property = node.child_by_field_name("property")?;
    (object.kind() == "identifier" && property.kind() == "property_identifier")
        .then(|| (text(object, source), text(property, source)))
}
pub(super) fn collect(
    node: Node,
    source: &str,
    line: usize,
    start: usize,
    out: &mut IndexedDocument,
) {
    if node.kind() != "call_expression" {
        return;
    }
    let Some(function) = node.child_by_field_name("function") else {
        return;
    };
    let (jquery, operation) = if let Some((object, operation)) = member(function, source) {
        if matches!(object, "$" | "jQuery") {
            (true, operation)
        } else if object == "axios" {
            (false, operation)
        } else {
            return;
        }
    } else if function.kind() == "identifier" && text(function, source) == "axios" {
        (false, "call")
    } else {
        return;
    };
    let alias = if jquery {
        match operation {
            "get" | "getJSON" | "getScript" => Some("GET"),
            "post" => Some("POST"),
            "ajax" => None,
            _ => return,
        }
    } else {
        match operation {
            "get" => Some("GET"),
            "post" | "postForm" => Some("POST"),
            "put" | "putForm" => Some("PUT"),
            "patch" | "patchForm" => Some("PATCH"),
            "delete" => Some("DELETE"),
            "head" => Some("HEAD"),
            "options" => Some("OPTIONS"),
            "request" | "call" => None,
            _ => return,
        }
    };
    let Some(arguments) = node.child_by_field_name("arguments") else {
        return;
    };
    let mut cursor = arguments.walk();
    let args: Vec<_> = arguments
        .named_children(&mut cursor)
        .filter(|n| n.kind() != "comment")
        .collect();
    let Some(first) = args.first().copied() else {
        return;
    };
    let (target, config) = if first.kind() == "object" {
        if jquery && !matches!(operation, "get" | "post" | "ajax")
            || !jquery && !matches!(operation, "call" | "request")
        {
            return;
        }
        let opts = options(first, source, jquery);
        let Some(target) = opts.url.clone().filter(|_| !opts.url_unknown) else {
            return;
        };
        (target, opts)
    } else {
        if !jquery && operation == "request" {
            return;
        }
        let Some(target) = plain_js_string(first, source) else {
            return;
        };
        let config_index = if jquery {
            (operation == "ajax").then_some(1)
        } else if matches!(
            operation,
            "post" | "put" | "patch" | "postForm" | "putForm" | "patchForm"
        ) {
            Some(2)
        } else {
            Some(1)
        };
        let opts = config_index
            .and_then(|i| args.get(i))
            .map(|n| options(*n, source, jquery))
            .unwrap_or_default();
        // Unknown/duplicate config URL fields could replace the positional URL.
        if opts.url_unknown || opts.url.as_ref().is_some_and(|u| u != target) {
            return;
        }
        (target.to_string(), opts)
    };
    if !target.starts_with('/')
        || target.starts_with("//")
        || target.len() > 4096
        || target.chars().any(|c| c.is_whitespace() || c.is_control())
        || target.contains(['{', '}', '<', '>'])
    {
        return;
    }
    let (request_method, request_method_basis) = if config.method_unknown
        || config
            .method
            .as_deref()
            .zip(alias)
            .is_some_and(|(a, b)| a != b)
    {
        (None, None)
    } else if let Some(m) = config.method {
        (Some(m), Some("explicit_options".into()))
    } else {
        (
            alias.map(str::to_string),
            alias.map(|_| "client_method_name_candidate".into()),
        )
    };
    out.endpoints.push(EndpointSpec {
        target,
        reference_kind: format!("js:{}:{operation}", if jquery { "jquery" } else { "axios" }),
        line,
        start_byte: start,
        request_method,
        request_method_basis,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn extract(source: &str) -> Vec<EndpointSpec> {
        let doc = parse("client", "client.ts", source, 1).unwrap();
        assert!(!doc.parse_errors);
        doc.endpoints
    }
    #[test]
    fn client_endpoints_jquery_literal_and_settings_forms() {
        let rows=extract("$.get('/get'); jQuery.post('/post', {method:'DELETE'}); $.getJSON('/json'); $.getScript('/script'); $.ajax({url:'/ajax',type:'patch'}); $.ajax('/two',{method:'PUT'}); $.get({url:'/settings'});");
        assert_eq!(rows.len(), 7, "{rows:?}");
        assert_eq!(rows[0].reference_kind, "js:jquery:get");
        assert_eq!(rows[0].request_method.as_deref(), Some("GET"));
        assert_eq!(rows[1].request_method.as_deref(), Some("POST"));
        assert_eq!(rows[4].request_method.as_deref(), Some("PATCH"));
        assert_eq!(
            rows[4].request_method_basis.as_deref(),
            Some("explicit_options")
        );
        assert_eq!(rows[6].target, "/settings");
        assert_eq!(
            rows[0].request_method_basis.as_deref(),
            Some("client_method_name_candidate")
        );
    }
    #[test]
    fn client_endpoints_axios_configs_aliases_and_data_positions() {
        let rows=extract("axios('/one',{method:'post'}); axios({url:'/object',method:'DELETE'}); axios.request({url:'/request',method:'HEAD'}); axios.get('/alias'); axios.post('/body',{url:'/not-target',method:'DELETE'}); axios.patchForm('/form',data,{method:'PATCH'});");
        assert_eq!(rows.len(), 6, "{rows:?}");
        assert_eq!(rows[0].request_method.as_deref(), Some("POST"));
        assert_eq!(rows[2].request_method.as_deref(), Some("HEAD"));
        assert_eq!(
            rows[3].request_method_basis.as_deref(),
            Some("client_method_name_candidate")
        );
        assert_eq!(rows[4].target, "/body");
        assert_eq!(rows[4].request_method.as_deref(), Some("POST"));
    }
    #[test]
    fn client_endpoints_uncertain_method_does_not_erase_known_url() {
        let rows=extract("$.ajax({url:'/missing'}); $.ajax({url:'/conflict',method:'GET',type:'POST'}); $.ajax({url:'/dynamic',method:verb}); $.ajax({url:'/duplicate',method:'GET',method:'GET'}); axios.get('/alias-conflict',{method:'POST'});");
        assert_eq!(rows.len(), 5, "{rows:?}");
        assert!(rows
            .iter()
            .all(|r| r.request_method.is_none() && r.request_method_basis.is_none()));
    }
    #[test]
    fn client_endpoints_uncertain_url_and_unrelated_methods_are_excluded() {
        let rows=extract("$.ajax({url:'/a',url:'/b'}); $.ajax({url:'/spread',...opts}); axios('/url',config); axios.get('/url',{url:'/override'}); $.get({url:dynamic}); $.get('/a'+id); $.get(`/a/${id}`); $.get('//external'); $.get('relative'); client.get('/custom'); const txt=\"$.get('/string')\"; // $.get('/comment')\naxios.request('/unsupported'); $.getJSON({url:'/unsupported'}); $.ajax({['url']:'/computed'});");
        assert!(rows.is_empty(), "{rows:?}");
    }
    #[test]
    fn client_endpoints_inline_scripts_keep_original_offsets_and_no_inert_data() {
        let source="<p>α</p>\n<script>\n$.get('/inline'); axios.post('/save',{});\n</script><script type='application/json'>$.get('/json')</script><script src='/external.js'>axios.get('/ignored')</script>";
        let doc = parse("html", "client.html", source, 1).unwrap();
        let rows: Vec<_> = doc
            .endpoints
            .iter()
            .filter(|r| r.reference_kind.starts_with("js:"))
            .collect();
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[0].line, 3);
        assert_eq!(&source[rows[0].start_byte..rows[0].start_byte + 5], "$.get");
        assert_eq!(
            &source[rows[1].start_byte..rows[1].start_byte + 10],
            "axios.post"
        );
    }
    #[test]
    fn client_endpoints_selector_size_and_language_boundaries() {
        let source = format!(
            "$.get('/{}'); $.get('/{}');",
            "a".repeat(4095),
            "a".repeat(4096)
        );
        assert_eq!(extract(&source).len(), 1);
        assert!(parse("py", "client.py", "jQuery.get('/python')", 1)
            .unwrap()
            .endpoints
            .is_empty());
    }
    #[test]
    fn client_endpoints_do_not_invent_server_registrations_or_handlers() {
        let doc=parse("client","client.ts","function callback(){}; $.get('/client',callback); axios.post('/save',{method:'DELETE'}); axios.get('/unknown',config); app.post('/server',callback);",1).unwrap();
        assert_eq!(doc.routes.len(), 1, "{:?}", doc.routes);
        assert_eq!(doc.routes[0].route, "/server");
        assert_eq!(doc.routes[0].handler, "callback");
        assert_eq!(doc.endpoints.len(), 2);
    }
}
