//! Syntax-based CSS definitions and static markup/DOM class applications.
//! Name matching is evidence in a corpus, not proof of a browser cascade or liveness.
use super::*;
use rmcp::schemars;
use std::collections::BTreeMap;

const MAX_FACTS: usize = 50_000;
fn short(s: &str) -> String {
    s.chars().take(512).collect()
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct StyleDocument {
    facts: Vec<Fact>,
    #[serde(skip)]
    line_starts: Vec<usize>,
    diagnostics: Vec<Problem>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Fact {
    name: String,
    kind: String,
    line: usize,
    column: usize,
    start_byte: usize,
    syntax: String,
    selector: Option<String>,
    conditions: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Problem {
    line: usize,
    code: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct Evidence {
    #[serde(flatten)]
    fact: Fact,
    document_id: String,
    path: String,
    project: Option<String>,
    revision: i64,
    content_hash: String,
}
#[derive(Debug, Default, Serialize)]
pub struct StyleSnapshot {
    facts: Vec<Evidence>,
    diagnostics: Vec<serde_json::Value>,
    documents: Vec<serde_json::Value>,
}
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ClassQuery {
    /// Exact CSS class token. One optional leading dot is accepted.
    pub name: Option<String>,
    /// all, missing (uses without definitions), unused (definitions without uses).
    pub mode: Option<String>,
    pub project: Option<String>,
    /// Maximum classes and total evidence sites returned (1..500, default 200).
    pub limit: Option<usize>,
}
fn position(out: &mut IndexedDocument, source: &str, offset: usize) -> (usize, usize) {
    if out.styles.line_starts.is_empty() {
        out.styles.line_starts.push(0);
        out.styles.line_starts.extend(
            source
                .bytes()
                .enumerate()
                .filter_map(|(i, b)| (b == b'\n').then_some(i + 1)),
        );
    }
    let row = out
        .styles
        .line_starts
        .partition_point(|p| *p <= offset)
        .saturating_sub(1);
    (row + 1, offset - out.styles.line_starts[row] + 1)
}
fn problem(out: &mut IndexedDocument, line: usize, code: &str) {
    if out.styles.diagnostics.len() >= 2000 {
        return;
    }
    if !out
        .styles
        .diagnostics
        .iter()
        .any(|p| p.line == line && p.code == code)
    {
        out.styles.diagnostics.push(Problem {
            line,
            code: code.into(),
        });
        if out.styles.diagnostics.len() == 1999 {
            out.styles.diagnostics.push(Problem {
                line,
                code: "STYLE_DIAGNOSTIC_BUDGET_EXCEEDED".into(),
            });
        }
    }
}
struct StyleContext {
    selector: Option<String>,
    conditions: Vec<String>,
}
fn add(
    out: &mut IndexedDocument,
    source: &str,
    offset: usize,
    name: &str,
    kind: &str,
    syntax: &str,
    context: StyleContext,
) {
    let StyleContext {
        selector,
        conditions,
    } = context;
    if name.is_empty() || name.chars().any(|c| c.is_ascii_whitespace() || c == '\0') {
        return;
    }
    let (line, column) = position(out, source, offset);
    if out.styles.facts.len() >= MAX_FACTS {
        problem(out, line, "CLASS_FACT_BUDGET_EXCEEDED");
        return;
    }
    if out
        .styles
        .facts
        .iter()
        .rev()
        .take(20)
        .any(|f| f.start_byte == offset && f.name == name && f.kind == kind)
    {
        return;
    }
    out.styles.facts.push(Fact {
        name: name.into(),
        kind: kind.into(),
        line,
        column,
        start_byte: offset,
        syntax: syntax.into(),
        selector,
        conditions,
    });
}
fn tokens(out: &mut IndexedDocument, full: &str, offset: usize, value: &str, syntax: &str) {
    for token in value.split_ascii_whitespace() {
        add(
            out,
            full,
            offset,
            token,
            "uses_class",
            syntax,
            StyleContext {
                selector: None,
                conditions: vec![],
            },
        );
    }
}
// CSS escape processing: hexadecimal escapes consume at most six digits plus one whitespace.
fn unescape_css(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let mut hex = String::new();
        while hex.len() < 6 && chars.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
            hex.push(chars.next().unwrap());
        }
        if !hex.is_empty() {
            if chars.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                chars.next();
            }
            out.push(
                u32::from_str_radix(&hex, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .filter(|c| *c != '\0')
                    .unwrap_or('\u{fffd}'),
            );
        } else if let Some(c) = chars.next() {
            out.push(c);
        }
    }
    out
}
fn children(node: Node) -> Vec<Node> {
    let mut c = node.walk();
    node.named_children(&mut c).collect()
}
fn css_walk(
    node: Node,
    fragment: &str,
    full: &str,
    base: usize,
    out: &mut IndexedDocument,
    depth: usize,
) {
    if depth >= 256 {
        problem(out, 1, "STYLE_TREE_DEPTH_EXCEEDED");
        return;
    }
    if node.kind() == "class_selector" {
        if let Some(name) = children(node)
            .into_iter()
            .find(|n| n.kind() == "class_name")
        {
            let mut parent = node.parent();
            let mut selector = None;
            let mut conditions = vec![];
            while let Some(p) = parent {
                if p.kind() == "rule_set" && selector.is_none() {
                    selector = children(p)
                        .into_iter()
                        .find(|n| n.kind() == "selectors")
                        .map(|n| short(text(n, fragment)));
                }
                if p.kind().ends_with("_statement") || p.kind() == "at_rule" {
                    let head = text(p, fragment).split('{').next().unwrap_or("").trim();
                    if head.starts_with('@') {
                        conditions.push(short(head));
                    }
                }
                parent = p.parent();
            }
            conditions.reverse();
            add(
                out,
                full,
                base + name.start_byte(),
                &unescape_css(text(name, fragment)),
                "defines_class",
                "css_selector",
                StyleContext {
                    selector,
                    conditions,
                },
            );
        }
    }
    if node.kind() == "import_statement" {
        problem(
            out,
            full[..base + node.start_byte()]
                .bytes()
                .filter(|b| *b == b'\n')
                .count()
                + 1,
            "EXTERNAL_STYLESHEET_LINKAGE_UNVERIFIED",
        );
    }
    for child in children(node) {
        css_walk(child, fragment, full, base, out, depth + 1);
    }
}
fn fragment_css(fragment: &str, full: &str, base: usize, out: &mut IndexedDocument) -> Result<()> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_css::LANGUAGE.into())
        .map_err(|e| AppError::config(e.to_string()))?;
    let tree = parser
        .parse(fragment, None)
        .ok_or_else(|| AppError::config("CSS parse cancelled"))?;
    if tree.root_node().has_error() {
        out.parse_errors = true;
        problem(out, 1, "CSS_PARSE_ERROR");
    }
    css_walk(tree.root_node(), fragment, full, base, out, 0);
    Ok(())
}
fn html_walk(node: Node, source: &str, out: &mut IndexedDocument, depth: usize) -> Result<()> {
    if depth >= 256 {
        problem(out, 1, "STYLE_TREE_DEPTH_EXCEEDED");
        return Ok(());
    }
    if node.kind() == "style_element" {
        let tag = children(node).into_iter().find(|n| n.kind() == "start_tag");
        let mut unsupported = false;
        if let Some(tag) = tag {
            for attr in children(tag)
                .into_iter()
                .filter(|n| n.kind() == "attribute")
            {
                let cs = children(attr);
                let name = cs.first().map(|n| text(*n, source)).unwrap_or("");
                let value = cs
                    .get(1)
                    .map(|n| text(*n, source).trim_matches(['\'', '"']))
                    .unwrap_or("");
                if name == "scoped" || name == "module" {
                    problem(out, attr.start_position().row + 1, "STYLE_SCOPE_UNRESOLVED");
                }
                if name == "lang" && value != "css" || name == "type" && value != "text/css" {
                    problem(
                        out,
                        attr.start_position().row + 1,
                        "STYLE_PREPROCESSOR_UNSUPPORTED",
                    );
                    unsupported = true;
                }
            }
        }
        if unsupported {
            return Ok(());
        }

        for raw in children(node)
            .into_iter()
            .filter(|n| n.kind() == "raw_text")
        {
            fragment_css(text(raw, source), source, raw.start_byte(), out)?;
        }
        return Ok(());
    }
    if node.kind() == "script_element" {
        // Embedded JavaScript uses the same AST collector; no calls/SQL are inferred here.
        for raw in children(node)
            .into_iter()
            .filter(|n| n.kind() == "raw_text")
        {
            let mut parser = Parser::new();
            parser
                .set_language(&tree_sitter_javascript::LANGUAGE.into())
                .map_err(|e| AppError::config(e.to_string()))?;
            let tree = parser
                .parse(text(raw, source), None)
                .ok_or_else(|| AppError::config("embedded JS parse cancelled"))?;
            if tree.root_node().has_error() {
                problem(out, raw.start_position().row + 1, "EMBEDDED_JS_PARSE_ERROR");
            }
            js_walk(
                tree.root_node(),
                text(raw, source),
                source,
                raw.start_byte(),
                out,
                0,
            );
        }
        return Ok(());
    }
    if node.kind() == "attribute" {
        let cs = children(node);
        let name = cs
            .iter()
            .find(|n| n.kind() == "attribute_name")
            .map(|n| text(*n, source))
            .unwrap_or("");
        let value = cs
            .iter()
            .find(|n| matches!(n.kind(), "attribute_value" | "quoted_attribute_value"));
        if name.eq_ignore_ascii_case("class") {
            if let Some(v) = value {
                let raw = text(*v, source).trim_matches(['\'', '"']);
                if raw.contains(['{', '}']) || raw.contains("<?") {
                    problem(
                        out,
                        node.start_position().row + 1,
                        "DYNAMIC_CLASS_EXPRESSION",
                    );
                } else {
                    tokens(
                        out,
                        source,
                        v.start_byte(),
                        &html_escape::decode_html_entities(raw),
                        "html_class",
                    );
                }
            }
        } else if let Some(class) = name.strip_prefix("class:") {
            add(
                out,
                source,
                node.start_byte(),
                class,
                "uses_class",
                "svelte_class_directive",
                StyleContext {
                    selector: None,
                    conditions: vec![],
                },
            );
            problem(
                out,
                node.start_position().row + 1,
                "CONDITIONAL_CLASS_APPLICATION",
            );
        } else if matches!(name, ":class" | "v-bind:class" | "[class]" | "[className]") {
            problem(
                out,
                node.start_position().row + 1,
                "DYNAMIC_CLASS_EXPRESSION",
            );
        }
        if name.eq_ignore_ascii_case("href")
            && node
                .parent()
                .is_some_and(|p| text(p, source).to_ascii_lowercase().starts_with("<link"))
        {
            problem(
                out,
                node.start_position().row + 1,
                "EXTERNAL_STYLESHEET_LINKAGE_UNVERIFIED",
            );
        }
    }
    for child in children(node) {
        html_walk(child, source, out, depth + 1)?;
    }
    Ok(())
}
pub(super) fn parse_markup(source: &str, out: &mut IndexedDocument) -> Result<()> {
    if out.path.to_ascii_lowercase().ends_with(".css") {
        if out.path.ends_with(".module.css") {
            problem(out, 1, "CSS_MODULE_SCOPE_UNRESOLVED");
        }
        return fragment_css(source, source, 0, out);
    }
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_html::LANGUAGE.into())
        .map_err(|e| AppError::config(e.to_string()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AppError::config("HTML parse cancelled"))?;
    if tree.root_node().has_error() {
        out.parse_errors = true;
        problem(out, 1, "MARKUP_PARSE_ERROR");
    }
    html_walk(tree.root_node(), source, out, 0)
}
fn static_js(node: Node, source: &str) -> Option<String> {
    if !matches!(node.kind(), "string" | "template_string") {
        return None;
    }
    if children(node)
        .iter()
        .any(|n| n.kind() == "template_substitution")
    {
        return None;
    }
    let s = text(node, source);
    let value = s.get(1..s.len().checked_sub(1)?)?;
    // Do not invent class names for unsupported JS escapes.
    if value.contains('\\') {
        return None;
    }
    Some(value.into())
}
fn js_walk(
    node: Node,
    fragment: &str,
    full: &str,
    base: usize,
    out: &mut IndexedDocument,
    depth: usize,
) {
    if depth >= 256 {
        problem(out, 1, "STYLE_TREE_DEPTH_EXCEEDED");
        return;
    }
    collect_js(node, fragment, full, base, out);
    for child in children(node) {
        js_walk(child, fragment, full, base, out, depth + 1);
    }
}
pub(super) fn collect(node: Node, source: &str, out: &mut IndexedDocument) {
    if matches!(
        out.path.rsplit('.').next(),
        Some("js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx")
    ) {
        collect_js(node, source, source, 0, out);
    }
}
fn collect_js(node: Node, fragment: &str, full: &str, base: usize, out: &mut IndexedDocument) {
    let (line, _) = position(out, full, base + node.start_byte());
    if node.kind() == "jsx_attribute" {
        let cs = children(node);
        let name = cs.first().map(|n| text(*n, fragment)).unwrap_or("");
        if matches!(name, "className" | "class") {
            let val = cs.get(1).copied().and_then(|v| {
                if v.kind() == "jsx_expression" {
                    children(v).first().copied()
                } else {
                    Some(v)
                }
            });
            if let Some((v, value)) = val.and_then(|v| static_js(v, fragment).map(|s| (v, s))) {
                let value = if cs.get(1).is_some_and(|n| n.kind() == "string") {
                    html_escape::decode_html_entities(&value).into_owned()
                } else {
                    value
                };
                tokens(out, full, base + v.start_byte(), &value, "jsx_class");
            } else {
                problem(out, line, "DYNAMIC_CLASS_EXPRESSION");
            }
        }
    }
    if node.kind() == "call_expression" {
        if let Some(fun) = node.child_by_field_name("function") {
            let name = text(fun, fragment);
            if let Some(args) = node.child_by_field_name("arguments") {
                let args = children(args);
                if name.ends_with(".setAttribute")
                    && args
                        .first()
                        .and_then(|n| static_js(*n, fragment))
                        .as_deref()
                        == Some("class")
                {
                    if let Some((n, value)) = args
                        .get(1)
                        .and_then(|n| static_js(*n, fragment).map(|v| (*n, v)))
                    {
                        tokens(
                            out,
                            full,
                            base + n.start_byte(),
                            &value,
                            "dom_set_attribute",
                        );
                    } else {
                        problem(out, line, "DYNAMIC_CLASS_EXPRESSION");
                    }
                }
                if [
                    ".classList.add",
                    ".classList.remove",
                    ".classList.toggle",
                    ".classList.contains",
                    ".classList.replace",
                ]
                .iter()
                .any(|s| name.ends_with(s))
                {
                    let count = if name.ends_with(".toggle") || name.ends_with(".contains") {
                        1
                    } else {
                        args.len()
                    };
                    for arg in args.into_iter().take(count) {
                        if let Some(value) = static_js(arg, fragment) {
                            // DOMTokenList APIs accept one token per argument, not a class string.
                            if value.chars().any(|c| c.is_ascii_whitespace()) {
                                problem(out, line, "INVALID_DOM_CLASS_TOKEN");
                            } else {
                                add(
                                    out,
                                    full,
                                    base + arg.start_byte(),
                                    &value,
                                    "uses_class",
                                    "dom_class_list",
                                    StyleContext {
                                        selector: None,
                                        conditions: vec![],
                                    },
                                );
                            }
                        } else {
                            problem(out, line, "DYNAMIC_CLASS_EXPRESSION");
                        }
                    }
                }
            }
        }
    }
    if node.kind() == "assignment_expression"
        && node
            .child_by_field_name("left")
            .is_some_and(|n| text(n, fragment).ends_with(".className"))
    {
        if let Some((n, value)) = node
            .child_by_field_name("right")
            .and_then(|n| static_js(n, fragment).map(|v| (n, v)))
        {
            tokens(out, full, base + n.start_byte(), &value, "dom_class_name");
        } else {
            problem(out, line, "DYNAMIC_CLASS_EXPRESSION");
        }
    }
}
pub(super) fn snapshot(documents: &HashMap<String, IndexedDocument>) -> StyleSnapshot {
    let mut out = StyleSnapshot::default();
    let mut docs: Vec<_> = documents.iter().collect();
    docs.sort_by_key(|(id, _)| *id);
    for (id, doc) in docs {
        out.documents
            .push(serde_json::json!({"document_id":id,"path":doc.path,"project":doc.project}));
        for f in &doc.styles.facts {
            out.facts.push(Evidence {
                fact: f.clone(),
                document_id: id.clone(),
                path: doc.path.clone(),
                project: doc.project.clone(),
                revision: doc.revision,
                content_hash: doc.hash.clone(),
            });
        }
        for p in &doc.styles.diagnostics {
            out.diagnostics.push(serde_json::json!({"document_id":id,"path":doc.path,"project":doc.project,"line":p.line,"code":p.code}));
        }
    }
    out.facts.sort_by(|a, b| {
        (&a.path, a.fact.start_byte, &a.fact.name, &a.fact.kind).cmp(&(
            &b.path,
            b.fact.start_byte,
            &b.fact.name,
            &b.fact.kind,
        ))
    });
    out
}
impl CodeSnapshot {
    pub fn class_usage(&self, q: &ClassQuery) -> Result<serde_json::Value> {
        let mode = q.mode.as_deref().unwrap_or("all");
        if !matches!(mode, "all" | "missing" | "unused") {
            return Err(AppError::config(
                "class mode must be all, missing or unused",
            ));
        }
        let name = q.name.as_deref().map(|s| s.strip_prefix('.').unwrap_or(s));
        if name.is_some_and(|s| {
            s.is_empty() || s.chars().any(|c| c.is_ascii_whitespace() || c == '\0')
        }) {
            return Err(AppError::config("name must be one nonempty class token"));
        }
        let limit = q.limit.unwrap_or(200).clamp(1, 500);
        type ClassEvidenceGroups<'a> =
            BTreeMap<(Option<&'a str>, &'a str), (Vec<&'a Evidence>, Vec<&'a Evidence>)>;
        let mut groups: ClassEvidenceGroups<'_> = BTreeMap::new();
        for f in &self.styles.facts {
            if q.project
                .as_ref()
                .is_some_and(|p| f.project.as_ref() != Some(p))
                || name.is_some_and(|n| n != f.fact.name)
            {
                continue;
            }
            let g = groups
                .entry((f.project.as_deref(), &f.fact.name))
                .or_default();
            if f.fact.kind == "defines_class" {
                g.0.push(f);
            } else {
                g.1.push(f);
            }
        }
        let mut items = vec![];
        let mut total = 0;
        let mut remaining = limit;
        let mut truncated = false;
        for ((project, name), (defined, used)) in groups {
            if mode == "missing" && (!defined.is_empty() || used.is_empty())
                || mode == "unused" && (defined.is_empty() || !used.is_empty())
            {
                continue;
            }
            total += 1;
            if items.len() >= limit {
                truncated = true;
                continue;
            }
            let ds: Vec<_> = defined.iter().take(remaining).copied().collect();
            remaining -= ds.len();
            let us: Vec<_> = used.iter().take(remaining).copied().collect();
            remaining -= us.len();
            let cut = ds.len() != defined.len() || us.len() != used.len();
            truncated |= cut;
            items.push(serde_json::json!({"name":name,"project":project,"defined_count":defined.len(),"used_count":used.len(),"defined":ds,"used":us,
                "status":if defined.is_empty(){"missing_in_indexed_scope"}else if used.is_empty(){"unused_in_indexed_scope"}else{"defined_and_used"},"evidence_truncated":cut}));
        }
        let diagnostics: Vec<_> = self
            .styles
            .diagnostics
            .iter()
            .filter(|d| q.project.as_ref().is_none_or(|p| d["project"] == *p))
            .collect();
        Ok(
            serde_json::json!({"index_rev":self.index_revision,"view":"fresh_subset_of_revision_inputs","mode":mode,"name":name,"project":q.project,"items":items,"total":total,"found":total>0,"truncated":truncated,
            "diagnostics":diagnostics.iter().take(limit).collect::<Vec<_>>(),"diagnostics_total":diagnostics.len(),"diagnostics_truncated":diagnostics.len()>limit,
            "stale_documents":self.stale_documents,"unindexed_documents":self.unindexed_documents,"parse_errors":self.parse_errors,
            "analysis_incomplete":!diagnostics.is_empty()||!self.stale_documents.is_empty()||!self.unindexed_documents.is_empty()||!self.parse_errors.is_empty(),
            "indexed_document_count":self.styles.documents.iter().filter(|d|q.project.as_ref().is_none_or(|p|d["project"]==*p)).count(),"matching":"exact_class_name_within_project_scope","not_checked":["browser_cascade","stylesheet_import_resolution","css_modules_scoping","runtime_class_generation","external_stylesheets","preprocessors"],"deletion_safe":false}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(files: &[(&str, &str, Option<&str>)]) -> CodeSnapshot {
        let mut docs = HashMap::new();
        for (i, (path, source, project)) in files.iter().enumerate() {
            let mut d = parse(&i.to_string(), path, source, 1).unwrap();
            d.project = project.map(str::to_string);
            docs.insert(i.to_string(), d);
        }
        CodeSnapshot {
            styles: super::snapshot(&docs),
            ..Default::default()
        }
    }
    fn query(name: Option<&str>, mode: &str) -> ClassQuery {
        ClassQuery {
            name: name.map(str::to_string),
            mode: Some(mode.into()),
            ..Default::default()
        }
    }
    #[test]
    fn css_ast_html_unicode_entities_escapes_media_and_false_tokens() {
        let s=snapshot(&[("main.css", "/* .fake {} */\n@media (width > 500px) {\n .real:hover, .sm\\:wide, .café { content: '.fake2'; }\n}\n[data-x='.fake3'] {color:red}\n.orphan{}",Some("app")),
        ("page.html","<!-- class='fake4' -->\n<div class=\"real sm:wide caf&#233; missing&#32;second\"></div>",Some("app"))]);
        let r = s.class_usage(&query(None, "all")).unwrap();
        assert_eq!(r["total"], 6, "{r}");
        for fake in ["fake", "fake2", "fake3", "fake4"] {
            assert_eq!(
                s.class_usage(&query(Some(fake), "all")).unwrap()["found"],
                false
            );
        }
        let real = s.class_usage(&query(Some(".real"), "all")).unwrap();
        assert_eq!(real["items"][0]["status"], "defined_and_used");
        assert_eq!(real["items"][0]["defined"][0]["line"], 3);
        assert_eq!(real["items"][0]["used"][0]["line"], 2);
        assert!(real["items"][0]["defined"][0]["conditions"][0]
            .as_str()
            .unwrap()
            .contains("@media"));
        assert_eq!(s.class_usage(&query(None, "missing")).unwrap()["total"], 2);
        assert_eq!(
            s.class_usage(&query(None, "unused")).unwrap()["items"][0]["name"],
            "orphan"
        );
    }
    #[test]
    fn jsx_and_dom_static_classes_dynamic_values_and_embedded_source_lines() {
        let s=snapshot(&[("app.tsx",r#"const x = <div className="alpha beta"/>;
const y = <div className={'gamma'} />;
const z = <div className={styles.dynamic} />;
el.classList.add('delta','epsilon'); el.classList.toggle('zeta', enabled);
el.setAttribute('class','eta theta'); el.className = `iota`;
el.classList.add('invalid token'); el.className = `prefix-${value}`;
const noise = "className='fake'"; // el.className='comment'
"#,None),
        ("page.html","<html>\n<style>\n.inline{}\n</style>\n<script>\nel.classList.add('inline');\n</script>\n<div class='{dynamic}'></div></html>",None)]);
        let r = s.class_usage(&query(None, "all")).unwrap();
        assert_eq!(r["total"], 10, "{r}");
        let inline = s.class_usage(&query(Some("inline"), "all")).unwrap();
        assert_eq!(inline["items"][0]["defined"][0]["line"], 3);
        assert_eq!(inline["items"][0]["used"][0]["line"], 6);
        assert!(r["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "DYNAMIC_CLASS_EXPRESSION"));
        assert!(r["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "INVALID_DOM_CLASS_TOKEN"));
        for n in ["fake", "comment", "enabled", "prefix-", "dynamic"] {
            assert_eq!(
                s.class_usage(&query(Some(n), "all")).unwrap()["found"],
                false
            );
        }
    }
    #[test]
    fn jsx_expression_strings_do_not_decode_html_entities() {
        let s = snapshot(&[(
            "p.tsx",
            r#"const x=<div className="a&amp;b"/>; const y=<div className={'c&amp;d'}/>;"#,
            None,
        )]);
        assert_eq!(
            s.class_usage(&query(Some("a&b"), "all")).unwrap()["found"],
            true
        );
        assert_eq!(
            s.class_usage(&query(Some("c&amp;d"), "all")).unwrap()["found"],
            true
        );
        assert_eq!(
            s.class_usage(&query(Some("c&d"), "all")).unwrap()["found"],
            false
        );
    }

    #[test]
    fn preprocessors_and_module_scopes_are_explicit_and_do_not_invent_css() {
        let s = snapshot(&[
            (
                "app.vue",
                r#"<template><div class="real"/></template><style lang="scss">.fake { &.child {} }</style><style scoped>.real{}</style>"#,
                None,
            ),
            ("a.module.css", ".module{}", None),
        ]);
        let r = s.class_usage(&query(None, "all")).unwrap();
        assert_eq!(
            s.class_usage(&query(Some("fake"), "all")).unwrap()["found"],
            false
        );
        for code in [
            "STYLE_PREPROCESSOR_UNSUPPORTED",
            "STYLE_SCOPE_UNRESOLVED",
            "CSS_MODULE_SCOPE_UNRESOLVED",
        ] {
            assert!(
                r["diagnostics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|d| d["code"] == code),
                "{r}"
            );
        }
    }

    #[test]
    fn class_matching_projects_are_isolated_and_evidence_budget_is_explicit() {
        let s = snapshot(&[
            ("a.css", ".shared{}", Some("a")),
            ("b.html", "<div class='shared other'></div>", Some("b")),
        ]);
        let mut q = query(Some("shared"), "all");
        let r = s.class_usage(&q).unwrap();
        assert_eq!(r["total"], 2, "{r}");
        q.project = Some("b".into());
        q.limit = Some(1);
        let r = s.class_usage(&q).unwrap();
        assert_eq!(r["items"][0]["status"], "missing_in_indexed_scope");
        let mut q = query(None, "all");
        q.limit = Some(1);
        let r = s.class_usage(&q).unwrap();
        assert_eq!(r["total"], 3);
        assert_eq!(r["truncated"], true);
        assert!(s.class_usage(&query(Some("two classes"), "all")).is_err());
        assert!(s.class_usage(&query(None, "delete")).is_err());
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::source_sync::{SourceSyncCommand, SourceSyncService};
    #[tokio::test]
    async fn markup_policy_sync_reuses_persists_and_omits_stale_facts() {
        let root = tempfile::tempdir().unwrap();
        let db = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("main.css"), ".live{} .unused{}").unwrap();
        let html = root.path().join("page.html");
        std::fs::write(
            &html,
            "<html><body><p class='live'>Visible.</p></body></html>",
        )
        .unwrap();
        let php = root.path().join("policy.php");
        std::fs::write(&php,"<?php function policy($role) { if ($role === 'admin') return true; return false; } function handler() { return policy('employee'); }").unwrap();
        let config = crate::Config {
            db_path: db.path().join("styles.duckdb"),
            ingest_roots: vec![root.path().into()],
            embedding_dims: 16,
            ..crate::Config::for_tests()
        };
        let store = Store::open(&config.db_path).unwrap();
        let embedder: std::sync::Arc<dyn crate::embeddings::EmbeddingProvider> =
            std::sync::Arc::new(crate::embeddings::MockEmbedder::new(16));
        let service = SourceSyncService::new(&store, &embedder, &config);
        let command = SourceSyncCommand::new(root.path().into());
        let report = service.sync(command.clone()).await.unwrap();
        assert!(report.errors.is_empty(), "{report:?}");
        assert_eq!(report.code_index.unwrap().indexed, 3);
        let q = ClassQuery {
            name: Some("live".into()),
            ..Default::default()
        };
        let p = crate::code_index::policy::PolicyQuery {
            symbol: "policy".into(),
            ..Default::default()
        };
        let snap = store.code_snapshot().unwrap();
        assert_eq!(
            snap.class_usage(&q).unwrap()["items"][0]["status"],
            "defined_and_used"
        );
        assert_eq!(snap.policy_trace(&p).unwrap()["default_behavior"], "deny");
        assert_eq!(
            service
                .sync(command.clone())
                .await
                .unwrap()
                .code_index
                .unwrap()
                .unchanged,
            3
        );
        std::fs::write(
            &html,
            "<html><body><p class='missing'>Visible.</p></body></html>",
        )
        .unwrap();
        std::fs::write(
            &php,
            "<?php function policy($role) { if ($role === 'blocked') return false; return true; }",
        )
        .unwrap();
        let report = service.sync(command).await.unwrap();
        assert!(report.errors.is_empty(), "{report:?}");
        let snap = store.code_snapshot().unwrap();
        assert_eq!(
            snap.class_usage(&q).unwrap()["items"][0]["status"],
            "unused_in_indexed_scope"
        );
        assert_eq!(snap.policy_trace(&p).unwrap()["default_behavior"], "allow");
        drop(store);
        let store = Store::open(&config.db_path).unwrap();
        assert_eq!(
            store.code_snapshot().unwrap().policy_trace(&p).unwrap()["default_behavior"],
            "allow"
        );
        let mut doc = store
            .list_documents()
            .unwrap()
            .into_iter()
            .find(|d| {
                d.source_file
                    .as_ref()
                    .is_some_and(|p| p.ends_with("policy.php"))
            })
            .unwrap();
        doc.content = "<?php function policy() {return false;}".into();
        store.upsert_document(&doc).unwrap();
        let r = store.code_snapshot().unwrap().policy_trace(&p).unwrap();
        assert_eq!(r["found"], false);
        assert!(!r["stale_documents"].as_array().unwrap().is_empty());
    }
}
