//! Literal local HTML/CSS file candidates; no browser, network or loader execution.
use super::*;
use std::path::Path;
mod inline_style;
mod responsive;
const MAX_RESOURCE_FACTS: usize = 50_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Spec {
    target: String,
    reference_kind: String,
    line: usize,
    start_byte: usize,
    base_unresolved: bool,
    #[serde(default)]
    choice: Option<responsive::Choice>,
    #[serde(default)]
    inline: Option<inline_style::InlineSource>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ResourceEdge {
    pub document_id: String,
    pub path: String,
    pub project: Option<String>,
    pub revision: i64,
    pub content_hash: String,
    pub target: String,
    pub reference_kind: String,
    pub line: usize,
    pub start_byte: usize,
    pub candidates: Vec<String>,
    pub resolution: &'static str,
    pub resolution_basis: &'static str,
    pub runtime_resource_proven: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub choice: Option<responsive::Choice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline: Option<inline_style::InlineSource>,
}
fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut c = node.walk();
    node.named_children(&mut c).collect()
}
fn add(out: &mut IndexedDocument, full: &str, offset: usize, target: &str, kind: &str) {
    record(out, full, offset, target, kind, None);
}
fn record(
    out: &mut IndexedDocument,
    full: &str,
    offset: usize,
    target: &str,
    kind: &str,
    choice: Option<responsive::Choice>,
) {
    // Retain unsupported URL forms as unresolved evidence, but never copy a
    // potentially huge data: URL or malformed source body into the graph.
    if target.is_empty() || target.len() > 4096 || out.file_resources.len() >= MAX_RESOURCE_FACTS {
        if out.file_resources.len() >= MAX_RESOURCE_FACTS
            && !out
                .binding_limitations
                .iter()
                .any(|v| v == "FILE_RESOURCE_FACT_BUDGET_EXCEEDED")
        {
            out.binding_limitations
                .push("FILE_RESOURCE_FACT_BUDGET_EXCEEDED".into());
        }
        return;
    }
    let (line, _) = styles::position(out, full, offset);
    out.file_resources.push(Spec {
        target: target.into(),
        reference_kind: kind.into(),
        line,
        start_byte: offset,
        base_unresolved: out.resource_base_unresolved,
        choice,
        inline: None,
    });
}
fn attribute(node: Node<'_>, name: &str, source: &str) -> Option<(usize, usize, String)> {
    let attrs: Vec<_> = children(node)
        .into_iter()
        .filter(|n| n.kind() == "attribute")
        .filter(|n| {
            children(*n).iter().any(|c| {
                c.kind() == "attribute_name" && text(*c, source).eq_ignore_ascii_case(name)
            })
        })
        .collect();
    if attrs.len() != 1 {
        return None;
    }
    let value = children(attrs[0])
        .into_iter()
        .find(|n| matches!(n.kind(), "attribute_value" | "quoted_attribute_value"))?;
    let raw = text(value, source);
    let raw = if value.kind() == "quoted_attribute_value" {
        let quote = *raw.as_bytes().first()?;
        if !matches!(quote, b'\'' | b'"') || raw.as_bytes().last() != Some(&quote) {
            return None;
        }
        raw.get(1..raw.len().checked_sub(1)?)?
    } else {
        raw
    };
    Some((
        value.start_byte(),
        value.end_byte(),
        html_escape::decode_html_entities(raw).into_owned(),
    ))
}
pub(super) fn markup(root: Node<'_>, source: &str, out: &mut IndexedDocument) {
    let mut stack = vec![(root, 0)];
    let mut tags = Vec::new();
    while let Some((node, depth)) = stack.pop() {
        if depth >= 256 {
            continue;
        }
        if matches!(node.kind(), "start_tag" | "self_closing_tag") {
            let tag = children(node)
                .into_iter()
                .find(|n| n.kind() == "tag_name")
                .map(|n| text(n, source).to_ascii_lowercase());
            if let Some(tag) = tag {
                tags.push((node, tag));
            }
        }
        stack.extend(children(node).into_iter().map(|n| (n, depth + 1)));
    }
    // A base anywhere in the document can change every relative URL, including
    // earlier attributes and inline CSS. Do not guess document deployment URLs.
    out.resource_base_unresolved = tags.iter().any(|(n, tag)| {
        tag == "base"
            && children(*n).iter().any(|a| {
                a.kind() == "attribute"
                    && children(*a).iter().any(|c| {
                        c.kind() == "attribute_name"
                            && text(*c, source).eq_ignore_ascii_case("href")
                    })
            })
    });
    for (node, tag) in tags {
        inline_style::collect(node, source, out);
        responsive::markup(node, &tag, source, out);
        let names: &[&str] = match tag.as_str() {
            "link" | "a" => &["href"],
            "script" | "iframe" | "img" | "source" | "audio" | "embed" => &["src"],
            "video" => &["src", "poster"],
            "input" => &["src", "formaction"],
            "object" => &["data"],
            "form" => &["action"],
            "button" => &["formaction"],
            _ => continue,
        };
        for name in names {
            if let Some((offset, _, value)) = attribute(node, name, source) {
                add(out, source, offset, &value, &format!("html:{tag}:{name}"));
            }
        }
    }
}
fn plain_string<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    if node.kind() != "string_value" {
        return None;
    }
    let raw = text(node, source);
    let quote = *raw.as_bytes().first()?;
    if !matches!(quote, b'\'' | b'"') || raw.as_bytes().last() != Some(&quote) {
        return None;
    }
    let value = raw.get(1..raw.len().checked_sub(1)?)?;
    (!value.contains('\\')).then_some(value)
}
pub(super) fn css(
    node: Node<'_>,
    fragment: &str,
    full: &str,
    base: usize,
    out: &mut IndexedDocument,
) {
    if responsive::css(node, fragment, full, base, out) {
        return;
    }
    if node.kind() == "import_statement" {
        // Only the first syntactic argument is the import URL; media/supports
        // conditions and their string operands are not imported files.
        if let Some(value) = children(node).into_iter().find(|n| n.kind() != "comment") {
            if let Some(target) = plain_string(value, fragment) {
                add(out, full, base + value.start_byte(), target, "css:import");
            }
        }
    }
    if node.kind() == "call_expression" {
        let cs = children(node);
        let Some(name) = cs.iter().find(|n| n.kind() == "function_name") else {
            return;
        };
        if !text(*name, fragment).eq_ignore_ascii_case("url") {
            return;
        }
        let Some(args) = cs.iter().find(|n| n.kind() == "arguments") else {
            return;
        };
        let values: Vec<_> = children(*args)
            .into_iter()
            .filter(|n| n.kind() != "comment")
            .collect();
        if values.len() != 1 {
            return;
        }
        let value = values[0];
        let target = plain_string(value, fragment).or_else(|| {
            (value.kind() == "plain_value" && !text(value, fragment).contains('\\'))
                .then(|| text(value, fragment))
        });
        if let Some(target) = target {
            let import = node.parent().is_some_and(|p| {
                p.kind() == "import_statement"
                    && children(p).into_iter().find(|n| n.kind() != "comment") == Some(node)
            });
            if !import {
                let mut ancestor = node.parent();
                let mut declaration = false;
                while let Some(p) = ancestor {
                    if p.kind() == "declaration" {
                        declaration = true;
                        break;
                    }
                    if p.kind() == "import_statement" {
                        break;
                    }
                    ancestor = p.parent();
                }
                if !declaration {
                    return;
                }
            }
            add(
                out,
                full,
                base + value.start_byte(),
                target,
                if import { "css:import" } else { "css:url" },
            );
        }
    }
}
fn local_path(doc: &IndexedDocument, spec: &Spec) -> std::result::Result<String, &'static str> {
    if spec.base_unresolved {
        return Err("html_base_url_unresolved");
    }
    let value = &spec.target;
    if value.starts_with('/') {
        return Err("web_root_or_origin_unresolved");
    }
    let path = value.split(['?', '#']).next().unwrap_or("");
    if path.is_empty()
        || path.contains(['\\', '%', ':', '{', '}', '<', '>', '$', '`'])
        || path.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err("unsupported_url_or_encoding");
    }
    if matches!(path.rsplit('/').next(), Some("" | "." | "..")) {
        return Err("directory_url_unresolved");
    }
    let parent = Path::new(&doc.path)
        .parent()
        .ok_or("source_parent_unavailable")?;
    Ok(resolve::normalized(&parent.join(path)))
}
#[cfg(test)]
pub(super) fn edges(documents: &HashMap<String, IndexedDocument>) -> Vec<ResourceEdge> {
    edges_with_targets(documents, &[])
}
pub(super) fn edges_with_targets(
    documents: &HashMap<String, IndexedDocument>,
    targets: &[resource_targets::ResourceTarget],
) -> Vec<ResourceEdge> {
    // Pre-index identities once instead of scanning the corpus for every URL.
    let mut paths: HashMap<(Option<&str>, &str), Vec<&str>> = HashMap::new();
    for (id, doc) in documents.iter().filter(|(_, d)| !d.parse_errors) {
        paths
            .entry((doc.project.as_deref(), doc.path.as_str()))
            .or_default()
            .push(id);
    }
    for target in targets.iter().filter(|t| t.fresh()) {
        paths
            .entry((target.project.as_deref(), target.path.as_str()))
            .or_default()
            .push(&target.document_id);
    }
    let mut out = Vec::new();
    for (id, doc) in documents.iter().filter(|(_, d)| !d.parse_errors) {
        for spec in &doc.file_resources {
            let path = local_path(doc, spec);
            let mut candidates: Vec<_> = path
                .as_ref()
                .ok()
                .and_then(|p| paths.get(&(doc.project.as_deref(), p.as_str())))
                .into_iter()
                .flatten()
                .filter(|target| **target != id.as_str())
                .map(|id| (*id).to_string())
                .collect();
            candidates.sort();
            candidates.dedup();
            out.push(ResourceEdge {
                document_id: id.clone(),
                path: doc.path.clone(),
                project: doc.project.clone(),
                revision: doc.revision,
                content_hash: doc.hash.clone(),
                target: spec.target.clone(),
                reference_kind: spec.reference_kind.clone(),
                line: spec.line,
                start_byte: spec.start_byte,
                resolution: match candidates.len() {
                    0 => "unresolved",
                    1 => "resolved_static_candidate",
                    _ => "ambiguous",
                },
                resolution_basis: path
                    .err()
                    .unwrap_or("captured_same_project_relative_url_path"),
                candidates,
                runtime_resource_proven: false,
                choice: spec.choice.clone(),
                inline: spec.inline.clone(),
            });
        }
    }
    out.sort_by(|a, b| {
        (&a.document_id, a.start_byte, &a.reference_kind, &a.target).cmp(&(
            &b.document_id,
            b.start_byte,
            &b.reference_kind,
            &b.target,
        ))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn docs(files: &[(&str, &str, &str)]) -> HashMap<String, IndexedDocument> {
        files
            .iter()
            .enumerate()
            .map(|(i, (path, project, source))| {
                let mut d = parse(&i.to_string(), path, source, 1).unwrap();
                assert!(!d.parse_errors, "{path}");
                d.project = Some((*project).into());
                (i.to_string(), d)
            })
            .collect()
    }
    #[test]
    fn html_and_css_literals_keep_resource_edges_without_symbols() {
        let html="<link href='main.css?x=1&amp;y=2'><script src='./app.js#v'></script><iframe src='other.html'></iframe>";
        let documents = docs(&[
            ("/p/index.html", "p", html),
            (
                "/p/main.css",
                "p",
                "@import './base.css' screen; .a {background: URL(\"./asset.css?x#v\");}",
            ),
            ("/p/base.css", "p", ".base { color: red; }"),
            ("/p/asset.css", "p", ""),
            ("/p/app.js", "p", "console.log('PRIVATE_BODY');"),
            ("/p/other.html", "p", "<p>other</p>"),
        ]);
        let edges = edges(&documents);
        assert_eq!(edges.len(), 5, "{edges:?}");
        assert!(edges
            .iter()
            .any(|e| e.reference_kind == "css:import" && e.candidates == ["2"]));
        assert!(edges
            .iter()
            .any(|e| e.reference_kind == "css:url" && e.candidates == ["3"]));
        let link = edges
            .iter()
            .find(|e| e.reference_kind == "html:link:href")
            .unwrap();
        assert_eq!(link.candidates, ["1"]);
        assert_eq!(link.target, "main.css?x=1&y=2");
        assert_eq!(&html[link.start_byte..link.start_byte + 1], "'");
        assert!(edges.iter().all(|e| !e.runtime_resource_proven));
    }
    #[test]
    fn html_base_affects_earlier_attributes_and_inline_css_with_full_offsets() {
        let source="<link href='base.css'>\n<base href='/deployed/'>\n<style>\n@import 'base.css';\n.a{background:url('base.css')}\n</style>";
        let documents = docs(&[("/p/index.html", "p", source), ("/p/base.css", "p", "")]);
        let edges = edges(&documents);
        assert_eq!(edges.len(), 3, "{edges:?}");
        assert!(edges
            .iter()
            .all(|e| e.candidates.is_empty() && e.resolution_basis == "html_base_url_unresolved"));
        let css = edges
            .iter()
            .find(|e| e.reference_kind == "css:import")
            .unwrap();
        assert_eq!(css.line, 4);
        assert_eq!(&source[css.start_byte..css.start_byte + 10], "'base.css'");
    }
    #[test]
    fn url_unknowns_duplicates_comments_and_css_conditions_never_guess_files() {
        let documents=docs(&[("/p/index.html","p","<!-- <script src='base.css'></script> --><link href='base.css' href='other.css'><link href='/base.css'><link href='https://x/base.css'><a href='base%2ecss'>encoded</a><a href='#base.css'>fragment</a><a href='base.css/'>directory</a>"),("/p/base.css","p",""),("/p/style.css","p","/* @import 'base.css'; */ @import 'base.css' screen; @supports (background: url('condition.css')) {.a{background:url('base.css')}} @namespace url('namespace.css'); .b{content:'base.css'; background:url(var(--asset));}")]);
        let edges = edges(&documents);
        assert_eq!(
            edges.iter().filter(|e| e.document_id == "0").count(),
            5,
            "{edges:?}"
        );
        assert!(edges
            .iter()
            .filter(|e| e.document_id == "0")
            .all(|e| e.candidates.is_empty()));
        let css: Vec<_> = edges.iter().filter(|e| e.document_id == "2").collect();
        assert_eq!(css.len(), 2, "{css:?}");
        assert!(css.iter().all(|e| e.candidates == ["1"]));
    }
    #[test]
    fn html_form_actions_and_unquoted_css_import_urls_keep_exact_targets() {
        let documents = docs(&[
            ("/p/forms.html", "p", "<form action='handler.php?save=1'><button formaction='handler.php'>save</button><input formaction='handler.php'></form>"),
            ("/p/handler.php", "p", "<?php function submit() {}"),
            ("/p/main.css", "p", "@import url(base.css) print; .a{background:url(base.css)}"),
            ("/p/base.css", "p", ""),
            ("/p/quoted.html", "p", r#"<link href="'handler.php'">"#),
        ]);
        let edges = edges(&documents);
        assert_eq!(edges.len(), 6, "{edges:?}");
        assert!(edges
            .iter()
            .filter(|e| e.document_id == "4")
            .all(|e| e.candidates.is_empty()));
        for kind in [
            "html:form:action",
            "html:button:formaction",
            "html:input:formaction",
        ] {
            assert!(
                edges
                    .iter()
                    .any(|e| e.reference_kind == kind && e.candidates == ["1"]),
                "{edges:?}"
            );
        }
        for kind in ["css:import", "css:url"] {
            assert!(
                edges
                    .iter()
                    .any(|e| e.reference_kind == kind && e.candidates == ["3"]),
                "{edges:?}"
            );
        }
    }
    #[test]
    fn resource_ambiguity_project_and_parse_errors_are_preserved() {
        let mut documents = docs(&[
            ("/p/index.html", "p", "<link href='base.css'>"),
            ("/p/base.css", "p", ""),
            ("/p/base.css", "p", ""),
            ("/p/base.css", "other", ""),
        ]);
        let edge = &edges(&documents)[0];
        assert_eq!(edge.candidates, ["1", "2"]);
        assert_eq!(edge.resolution, "ambiguous");
        documents.get_mut("1").unwrap().parse_errors = true;
        assert_eq!(edges(&documents)[0].candidates, ["2"]);
        documents.get_mut("0").unwrap().parse_errors = true;
        assert!(edges(&documents).is_empty());
    }
    #[test]
    fn resource_impact_dead_files_refresh_and_frozen_legacy_diff() {
        use crate::models::Document;
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("resources.duckdb")).unwrap();
        let doc = |id: &str, path: &str, body: &str| Document {
            id: id.into(),
            uri: format!("file://{path}"),
            source_file: Some(path.into()),
            title: path.into(),
            content: body.into(),
            ..Default::default()
        };
        let mut page = doc("page", "/p/index.html", "<link href='main.css'>");
        store.upsert_document(&page).unwrap();
        store
            .upsert_document(&doc("main", "/p/main.css", "@import 'base.css';"))
            .unwrap();
        store
            .upsert_document(&doc("base", "/p/base.css", ".base{color:red}"))
            .unwrap();
        store.index_code().unwrap();
        let q = CodeQuery {
            impact_file: Some("/p/base.css".into()),
            ..Default::default()
        };
        let snapshot = store.code_snapshot().unwrap();
        let impact = snapshot.query("impact", &q);
        assert_eq!(impact["items"][0]["path"], "/p/main.css", "{impact}");
        assert_eq!(impact["items"][1]["path"], "/p/index.html");
        assert_eq!(impact["items"][1]["depth"], 2);
        assert_eq!(impact["items"][0]["via_kind"], "file_resource_candidate");
        let dead = store.code_dead_files(&CodeQuery::default()).unwrap();
        assert!(
            !dead["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["file"]["path"] == "/p/main.css"),
            "{dead}"
        );
        page.content = "<p>removed</p>".into();
        store.upsert_document(&page).unwrap();
        assert_eq!(
            store.code_snapshot().unwrap().query("impact", &q)["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        store.index_code().unwrap();
        let dq = history::DiffQuery {
            before: 1,
            after: Some(2),
            project: None,
            limit: None,
        };
        let diff = store.code_index_diff(&dq).unwrap();
        assert_eq!(diff["file_resource_relations_available"], true);
        assert!(
            diff["relations_removed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["kind"] == "file_resource_candidate"),
            "{diff}"
        );
        page.content = "<link href='base.css'>".into();
        store.upsert_document(&page).unwrap();
        store.index_code().unwrap();
        assert_eq!(store.code_index_diff(&dq).unwrap(), diff);
        let c = store.lock().unwrap();
        let raw: String = c
            .query_row(
                "SELECT graph_json FROM code_index_revisions WHERE id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut graph: serde_json::Value = serde_json::from_str(&raw).unwrap();
        graph.as_object_mut().unwrap().remove("file_resources");
        c.execute(
            "UPDATE code_index_revisions SET graph_json=? WHERE id=1",
            [graph.to_string()],
        )
        .unwrap();
        drop(c);
        let legacy = store.code_index_diff(&dq).unwrap();
        assert_eq!(legacy["file_resource_relations_available"], false);
        assert!(!legacy["relations_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "file_resource_candidate"));
    }
}
