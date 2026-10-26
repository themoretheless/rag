//! Literal markup and fetch URL evidence, separate from handler registrations.
use super::*;
mod clients;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct EndpointSpec {
    pub target: String,
    pub reference_kind: String,
    pub line: usize,
    pub start_byte: usize,
    #[serde(default)]
    pub request_method: Option<String>,
    #[serde(default)]
    pub request_method_basis: Option<String>,
}

pub(super) fn collect_markup(node: Node, source: &str, out: &mut IndexedDocument) {
    if !matches!(node.kind(), "start_tag" | "self_closing_tag") {
        return;
    }
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    let Some(tag) = children.iter().find(|n| n.kind() == "tag_name") else {
        return;
    };
    let tag = text(*tag, source).to_ascii_lowercase();
    let attribute = match tag.as_str() {
        "form" => "action",
        "button" | "input" => "formaction",
        "a" | "link" => "href",
        "script" | "img" | "iframe" | "source" | "video" | "audio" => "src",
        _ => return,
    };
    let attrs: Vec<_> = children
        .iter()
        .filter(|n| n.kind() == "attribute")
        .filter(|n| {
            let mut cursor = n.walk();
            let matched = n.named_children(&mut cursor).any(|child| {
                child.kind() == "attribute_name"
                    && text(child, source).eq_ignore_ascii_case(attribute)
            });
            matched
        })
        .collect();
    // Do not pick an arbitrary winner for duplicate attributes.
    if attrs.len() != 1 {
        return;
    }
    let attr = attrs[0];
    let mut cursor = attr.walk();
    let Some(value) = attr
        .named_children(&mut cursor)
        .find(|n| matches!(n.kind(), "attribute_value" | "quoted_attribute_value"))
    else {
        return;
    };
    let raw = text(value, source).trim_matches(['\'', '"']);
    let target = html_escape::decode_html_entities(raw);
    // Preserve literal URL spelling, including query/fragment. No filesystem or
    // origin resolution, template evaluation, network access or path collapsing.
    if !target.starts_with('/')
        || target.starts_with("//")
        || target.len() > 4096
        || target.chars().any(|c| c.is_whitespace() || c.is_control())
        || target.contains(['{', '}', '$', '<', '>', '\\'])
    {
        return;
    }
    out.endpoints.push(EndpointSpec {
        target: target.into_owned(),
        reference_kind: format!("html:{tag}:{attribute}"),
        line: attr.start_position().row + 1,
        start_byte: attr.start_byte(),
        request_method: None,
        request_method_basis: None,
    });
}

/// Browser-like executable-script subset, without executing or fetching it.
pub(super) fn javascript_script(node: Node, source: &str) -> bool {
    let mut cursor = node.walk();
    let Some(tag) = node
        .named_children(&mut cursor)
        .find(|n| n.kind() == "start_tag")
    else {
        return false;
    };
    let mut cursor = tag.walk();
    let mut script_type = None;
    for attr in tag
        .named_children(&mut cursor)
        .filter(|n| n.kind() == "attribute")
    {
        let mut cursor = attr.walk();
        let children: Vec<_> = attr.named_children(&mut cursor).collect();
        let Some(name) = children.iter().find(|n| n.kind() == "attribute_name") else {
            continue;
        };
        let name = text(*name, source);
        if name.eq_ignore_ascii_case("src") {
            return false;
        }
        if name.eq_ignore_ascii_case("type") {
            if script_type.is_some() {
                return false;
            }
            let value = children
                .iter()
                .find(|n| matches!(n.kind(), "attribute_value" | "quoted_attribute_value"));
            script_type = Some(
                value
                    .map(|n| {
                        html_escape::decode_html_entities(
                            text(*n, source).trim_matches(['\'', '"']),
                        )
                        .trim()
                        .to_ascii_lowercase()
                    })
                    .unwrap_or_default(),
            );
        }
    }
    script_type.is_none_or(|t| {
        matches!(
            t.as_str(),
            "" | "module"
                | "text/javascript"
                | "application/javascript"
                | "text/ecmascript"
                | "application/ecmascript"
        )
    })
}

fn plain_js_string<'a>(node: Node, source: &'a str) -> Option<&'a str> {
    if !matches!(node.kind(), "string" | "template_string") {
        return None;
    }
    let raw = text(node, source);
    let quote = raw.as_bytes().first()?;
    if !matches!(quote, b'\'' | b'"' | b'`') || raw.as_bytes().last()? != quote {
        return None;
    }
    let value = raw.get(1..raw.len().checked_sub(1)?)?;
    (!value.contains(['\\', '$', '`'])).then_some(value)
}
fn explicit_fetch_method(options: Node, source: &str) -> Option<String> {
    if options.kind() != "object" {
        return None;
    }
    let mut cursor = options.walk();
    let mut method = None;
    for property in options.named_children(&mut cursor) {
        if property.kind() == "comment" {
            continue;
        }
        // A spread/computed/shorthand member could override the method.
        if property.kind() != "pair" {
            return None;
        }
        let key = property.child_by_field_name("key")?;
        let key = if key.kind() == "property_identifier" {
            text(key, source)
        } else {
            plain_js_string(key, source)?
        };
        if key == "method" {
            if method.is_some() {
                return None;
            }
            let value = plain_js_string(property.child_by_field_name("value")?, source)?
                .to_ascii_uppercase();
            if !matches!(
                value.as_str(),
                "GET"
                    | "POST"
                    | "PUT"
                    | "PATCH"
                    | "DELETE"
                    | "HEAD"
                    | "OPTIONS"
                    | "TRACE"
                    | "CONNECT"
            ) {
                return None;
            }
            method = Some(value);
        }
    }
    method
}
/// Only syntactic fetch candidates; the identifier may be shadowed/imported.
/// `line` and `start` already refer to the enclosing source for embedded JS.
pub(super) fn collect_fetch(
    node: Node,
    source: &str,
    line: usize,
    start: usize,
    out: &mut IndexedDocument,
) {
    clients::collect(node, source, line, start, out);
    if node.kind() != "call_expression" {
        return;
    }
    let Some(function) = node.child_by_field_name("function") else {
        return;
    };
    let matched = function.kind() == "identifier" && text(function, source) == "fetch"
        || function.kind() == "member_expression"
            && function.child_by_field_name("object").is_some_and(|n| {
                n.kind() == "identifier"
                    && matches!(text(n, source), "window" | "globalThis" | "self")
            })
            && function
                .child_by_field_name("property")
                .is_some_and(|n| text(n, source) == "fetch");
    if !matched {
        return;
    }
    let Some(args) = node.child_by_field_name("arguments") else {
        return;
    };
    let mut cursor = args.walk();
    let args: Vec<_> = args
        .named_children(&mut cursor)
        .filter(|n| n.kind() != "comment")
        .collect();
    let Some(target) = args.first().and_then(|n| plain_js_string(*n, source)) else {
        return;
    };
    if !target.starts_with('/')
        || target.starts_with("//")
        || target.len() > 4096
        || target.chars().any(|c| c.is_whitespace() || c.is_control())
        || target.contains(['{', '}', '<', '>'])
    {
        return;
    }
    out.endpoints.push(EndpointSpec {
        target: target.into(),
        reference_kind: "js:fetch".into(),
        line,
        start_byte: start,
        request_method: args.get(1).and_then(|n| explicit_fetch_method(*n, source)),
        request_method_basis: args
            .get(1)
            .and_then(|n| explicit_fetch_method(*n, source))
            .map(|_| "explicit_options".into()),
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markup_targets_are_decoded_and_keep_evidence_kinds() {
        let doc = parse("a", "/p/a.html", "<!-- <img src='/comment'> -->\n<FORM ACTION='/save?a=1&amp;b=2'><input formaction=/override></FORM><script src='/app.js'></script><img src='/img.png'/><a href='/docs#intro'>docs</a><link href='/main.css'>", 1).unwrap();
        assert_eq!(doc.endpoints.len(), 6, "{:?}", doc.endpoints);
        assert!(doc.endpoints.iter().any(|e| e.target == "/save?a=1&b=2"
            && e.reference_kind == "html:form:action"
            && e.line == 2));
        assert!(doc
            .endpoints
            .iter()
            .any(|e| e.target == "/app.js" && e.reference_kind == "html:script:src"));
        assert!(doc
            .endpoints
            .iter()
            .any(|e| e.target == "/override" && e.reference_kind == "html:input:formaction"));
        assert!(!doc.endpoints.iter().any(|e| e.target == "/comment"));
    }
    #[test]
    fn dynamic_external_duplicate_and_script_text_targets_are_excluded() {
        let doc = parse("a", "/p/a.html", "<img src='//external/x'><img src='https://external/x'><img src='/a' SRC='/b'><img src='/{dynamic}'><img src='/&#123;dynamic&#125;'><img src='/bad&#10;path'><img :src='x'><img src='relative.png'><a href='#anchor'>x</a><div src='/not-a-url-tag'></div><script>const s = \"<img src='/script-text'>\";</script>", 1).unwrap();
        assert!(doc.endpoints.is_empty(), "{:?}", doc.endpoints);
    }
    #[test]
    fn fetch_literals_and_methods_are_static_candidates() {
        let doc = parse("a", "/p/a.ts", "fetch('/read'); window.fetch(\"/write?a=1&amp;b=2\", {method:'post', body: payload}); globalThis.fetch(`/fixed`, {'method':'PATCH'}); self.fetch('/x', {...options, method: 'GET'}); fetch('/dup', {method:'GET', method:'POST'});", 1).unwrap();
        assert_eq!(doc.endpoints.len(), 5, "{:?}", doc.endpoints);
        assert_eq!(doc.endpoints[0].request_method, None);
        assert_eq!(doc.endpoints[1].request_method.as_deref(), Some("POST"));
        assert_eq!(doc.endpoints[1].target, "/write?a=1&amp;b=2");
        assert_eq!(doc.endpoints[2].request_method.as_deref(), Some("PATCH"));
        assert_eq!(doc.endpoints[3].request_method, None);
        assert_eq!(doc.endpoints[4].request_method, None);
    }
    #[test]
    fn fetch_dynamic_urls_comments_and_other_clients_are_not_extracted() {
        let doc = parse("a", "/p/a.js", "// fetch('/comment')\nconst s = \"fetch('/string')\"; fetch(url); fetch('/a' + id); fetch(`/users/${id}`); fetch('https://external/x'); fetch('//external/x'); client.fetch('/custom'); window['fetch']('/computed'); fetch('/escaped\\x2fpath');", 1).unwrap();
        assert!(doc.endpoints.is_empty(), "{:?}", doc.endpoints);
        let py = parse("p", "/p/a.py", "fetch('/python')", 1).unwrap();
        assert!(py.endpoints.is_empty());
    }
    #[test]
    fn embedded_fetch_offsets_and_script_types_are_preserved() {
        let source = "<html>\n<script>\nfetch('/inline', {method: 'DELETE'});\n</script><script type='application/json'>fetch('/json')</script><script src='/external.js'>fetch('/ignored')</script><script type='module'>fetch('/module')</script></html>";
        let doc = parse("a", "/p/a.html", source, 1).unwrap();
        let js: Vec<_> = doc
            .endpoints
            .iter()
            .filter(|e| e.reference_kind == "js:fetch")
            .collect();
        assert_eq!(js.len(), 2, "{:?}", doc.endpoints);
        assert_eq!(js[0].line, 3);
        assert_eq!(&source[js[0].start_byte..js[0].start_byte + 5], "fetch");
        assert_eq!(js[0].request_method.as_deref(), Some("DELETE"));
        assert_eq!(js[1].target, "/module");
        assert!(doc
            .endpoints
            .iter()
            .any(|e| e.target == "/external.js" && e.reference_kind == "html:script:src"));
    }
}
