//! HTML style declaration lists with original-attribute evidence anchors.
use super::*;
use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, ParseError, Parser as CssParser, ParserInput,
    ParserState, QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, Token,
};

const MAX_ATTRIBUTE_BYTES: usize = 256 * 1024;
const PREFIX: &str = ".inline{";
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineSource {
    pub source_anchor: String,
    pub source_end_byte: usize,
    pub property: String,
    pub property_truncated: bool,
    pub property_hash: String,
    pub decoded_declaration_start: usize,
    pub decoded_declaration_end: usize,
    pub decoded_attribute_hash: String,
    pub css_reference_kind: String,
    pub element_base_unresolved: bool,
}
struct Declaration {
    name: String,
    start: usize,
    end: usize,
}
struct Declarations;
fn consume<'i>(
    input: &mut CssParser<'i, '_>,
    depth: usize,
) -> std::result::Result<(), ParseError<'i, ()>> {
    if depth >= 128 {
        return Err(input.new_custom_error(()));
    }
    while let Ok(token) = input.next_including_whitespace_and_comments() {
        let token = token.clone();
        if token.is_parse_error() {
            return Err(input.new_unexpected_token_error(token));
        }
        if matches!(
            token,
            Token::Function(_)
                | Token::ParenthesisBlock
                | Token::SquareBracketBlock
                | Token::CurlyBracketBlock
        ) {
            input.parse_nested_block(|nested| consume(nested, depth + 1))?;
        }
    }
    Ok(())
}
impl<'i> DeclarationParser<'i> for Declarations {
    type Declaration = Declaration;
    type Error = ();
    fn parse_value<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut CssParser<'i, 't>,
        start: &ParserState,
    ) -> std::result::Result<Declaration, ParseError<'i, ()>> {
        consume(input, 0)?;
        let name = if name.starts_with("--") {
            name.to_string()
        } else {
            name.to_ascii_lowercase()
        };
        Ok(Declaration {
            name,
            start: start.position().byte_index(),
            end: input.position().byte_index(),
        })
    }
}
impl<'i> AtRuleParser<'i> for Declarations {
    type Prelude = ();
    type AtRule = Declaration;
    type Error = ();
}
impl<'i> QualifiedRuleParser<'i> for Declarations {
    type Prelude = ();
    type QualifiedRule = Declaration;
    type Error = ();
}
impl<'i> RuleBodyItemParser<'i, Declaration, ()> for Declarations {
    fn parse_declarations(&self) -> bool {
        true
    }
    fn parse_qualified(&self) -> bool {
        false
    }
}
fn limitation(out: &mut IndexedDocument, code: &str) {
    if !out.binding_limitations.iter().any(|c| c == code) {
        out.binding_limitations.push(code.into());
    }
}
fn element_base(node: Node<'_>, source: &str) -> bool {
    let mut ancestor = Some(node);
    while let Some(node) = ancestor {
        let tag = if matches!(node.kind(), "start_tag" | "self_closing_tag") {
            Some(node)
        } else if matches!(node.kind(), "element" | "script_element" | "style_element") {
            children(node).into_iter().find(|c| c.kind() == "start_tag")
        } else {
            None
        };
        if tag.is_some_and(|tag| {
            children(tag)
                .into_iter()
                .filter(|c| c.kind() == "attribute")
                .any(|attr| {
                    children(attr).into_iter().any(|c| {
                        c.kind() == "attribute_name"
                            && text(c, source).eq_ignore_ascii_case("xml:base")
                    })
                })
        }) {
            return true;
        }
        ancestor = node.parent();
    }
    false
}
pub(super) fn collect(node: Node<'_>, source: &str, out: &mut IndexedDocument) {
    let Some((offset, end, value)) = attribute(node, "style", source) else {
        return;
    };
    if value.len() > MAX_ATTRIBUTE_BYTES {
        limitation(out, "INLINE_STYLE_ATTRIBUTE_BYTE_BUDGET_EXCEEDED");
        return;
    }
    if value.contains('\\') {
        limitation(out, "INLINE_STYLE_CSS_ESCAPES_UNVERIFIED");
    }
    let element_base_unresolved = element_base(node, source);
    if element_base_unresolved {
        limitation(out, "INLINE_STYLE_ELEMENT_BASE_UNRESOLVED");
    }
    let Ok(mut temporary) = parse("inline-style", "/inline.css", "", 0) else {
        limitation(out, "INLINE_STYLE_PARSER_UNAVAILABLE");
        return;
    };
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_css::LANGUAGE.into())
        .is_err()
    {
        limitation(out, "INLINE_STYLE_PARSER_UNAVAILABLE");
        return;
    }
    let hash = content_hash(&value);
    let mut input = ParserInput::new(&value);
    let mut input = CssParser::new(&mut input);
    let mut declarations = Declarations;
    for declaration in RuleBodyParser::new(&mut input, &mut declarations) {
        let Ok(declaration) = declaration else {
            limitation(out, "INLINE_STYLE_DECLARATION_SYNTAX_UNVERIFIED");
            continue;
        };
        let fragment = &value[declaration.start..declaration.end];
        let wrapped = format!("{PREFIX}{fragment};}}");
        let Some(tree) = parser.parse(&wrapped, None) else {
            limitation(out, "INLINE_STYLE_PARSER_UNAVAILABLE");
            continue;
        };
        if tree.root_node().has_error() {
            limitation(out, "INLINE_STYLE_DECLARATION_GRAMMAR_UNVERIFIED");
            continue;
        }
        temporary.file_resources.clear();
        temporary.binding_limitations.clear();
        temporary.styles = styles::StyleDocument::default();
        let mut stack = vec![(tree.root_node(), 0)];
        while let Some((node, depth)) = stack.pop() {
            if depth >= 256 {
                limitation(out, "INLINE_STYLE_TREE_DEPTH_EXCEEDED");
                continue;
            }
            css(node, &wrapped, &wrapped, 0, &mut temporary);
            stack.extend(children(node).into_iter().map(|child| (child, depth + 1)));
        }
        for code in &temporary.binding_limitations {
            limitation(out, code);
        }
        for spec in &temporary.file_resources {
            // Synthetic CSS offsets are never exposed as HTML offsets. Entity
            // expansion prevents an identity mapping; anchor the whole original
            // attribute and retain separate decoded declaration coordinates.
            if spec.start_byte < PREFIX.len() || spec.start_byte >= PREFIX.len() + fragment.len() {
                continue;
            }
            let mut choice = spec.choice.clone();
            if let Some(choice) = &mut choice {
                choice.source_anchor = "html_style_attribute_value".into();
                choice.source_end_byte = end;
            }
            let kind = if spec.reference_kind == "css:image-set" {
                "html:style:image-set"
            } else {
                "html:style:url"
            };
            let count = out.file_resources.len();
            record(out, source, offset, &spec.target, kind, choice);
            if out.file_resources.len() == count {
                if count >= MAX_RESOURCE_FACTS {
                    return;
                }
                continue;
            }
            let next = out.file_resources.last_mut().unwrap();
            next.base_unresolved |= element_base_unresolved;
            next.inline = Some(InlineSource {
                source_anchor: "html_style_attribute_value".into(),
                source_end_byte: end,
                property: declaration.name.chars().take(128).collect(),
                property_truncated: declaration.name.chars().count() > 128,
                property_hash: content_hash(&declaration.name),
                decoded_declaration_start: declaration.start,
                decoded_declaration_end: declaration.end,
                decoded_attribute_hash: hash.clone(),
                css_reference_kind: spec.reference_kind.clone(),
                element_base_unresolved,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parsed(source: &str) -> IndexedDocument {
        let out = parse("page", "/p/page.html", source, 1).unwrap();
        assert!(!out.parse_errors, "{source}");
        out
    }
    #[test]
    fn inline_urls_keep_decoded_values_and_original_multibyte_attribute_anchor() {
        let source="<p>Привет</p>\n<div style=\"background:url(&quot;a.svg?x=1&amp;y=2&quot;); cursor:url('cursor.svg'),auto; content:'url(fake.svg)'\"></div>";
        let out = parsed(source);
        assert!(out.symbols.is_empty() && out.references.is_empty());
        assert_eq!(
            styles::snapshot(&HashMap::from([("page".into(), out.clone())])).fact_count(),
            0
        );
        assert_eq!(out.file_resources.len(), 2, "{:?}", out.file_resources);
        let a = out
            .file_resources
            .iter()
            .find(|s| s.target == "a.svg?x=1&y=2")
            .unwrap();
        let inline = a.inline.as_ref().unwrap();
        assert_eq!(a.reference_kind, "html:style:url");
        assert_eq!(a.line, 2);
        assert_eq!(inline.property, "background");
        assert_eq!(inline.css_reference_kind, "css:url");
        assert_eq!(&source[a.start_byte..a.start_byte + 1], "\"");
        assert!(source[a.start_byte..inline.source_end_byte].contains("&quot;"));
        let (_, _, value) = attribute_from_source(source);
        assert_eq!(inline.decoded_attribute_hash, content_hash(&value));
        assert!(
            value[inline.decoded_declaration_start..inline.decoded_declaration_end]
                .starts_with("background:")
        );
    }
    fn attribute_from_source(source: &str) -> (usize, usize, String) {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_html::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        let mut stack = vec![tree.root_node()];
        while let Some(n) = stack.pop() {
            if matches!(n.kind(), "start_tag" | "self_closing_tag") {
                if let Some(v) = attribute(n, "style", source) {
                    return v;
                }
            }
            stack.extend(children(n));
        }
        panic!("missing style")
    }
    #[test]
    fn inline_responsive_images_keep_choice_evidence_without_synthetic_offsets() {
        let source="<div style=\"background-image:image-set('a.svg' 1x type('image/svg+xml'),url('b.svg') 2x)\"></div>";
        let out = parsed(source);
        assert_eq!(out.file_resources.len(), 2, "{:?}", out.file_resources);
        for spec in &out.file_resources {
            let inline = spec.inline.as_ref().unwrap();
            let choice = spec.choice.as_ref().unwrap();
            assert_eq!(spec.reference_kind, "html:style:image-set");
            assert_eq!(inline.property, "background-image");
            assert_eq!(choice.source_anchor, "html_style_attribute_value");
            assert_eq!(choice.source_end_byte, inline.source_end_byte);
            assert_eq!(&source[spec.start_byte..spec.start_byte + 1], "\"");
            assert!(!choice.selection_proven);
        }
        assert!(out
            .file_resources
            .iter()
            .all(|s| s.target != "image/svg+xml"));
    }
    #[test]
    fn declaration_list_recovery_does_not_treat_rules_or_at_rules_as_inline_resources() {
        let out=parsed("<div style=\"};background:url('after.svg'); .bad{background:url('fake.svg')}; @import 'phantom.css'; cursor:url('cursor.svg'),auto; broken tokens; mask-image:url('last.svg')\"></div><img src='ordinary.svg'>");
        let mut targets: Vec<_> = out
            .file_resources
            .iter()
            .map(|s| s.target.as_str())
            .collect();
        targets.sort();
        assert_eq!(
            targets,
            ["after.svg", "cursor.svg", "last.svg", "ordinary.svg"]
        );
        assert!(out
            .binding_limitations
            .iter()
            .any(|c| c == "INLINE_STYLE_DECLARATION_SYNTAX_UNVERIFIED"));
    }
    #[test]
    fn duplicate_template_and_base_contexts_do_not_choose_arbitrary_targets() {
        let out=parsed("<p style=\"background:url('duplicate.svg')\" STYLE=\"background:url('other.svg')\"></p><p :style=\"dynamic\"></p><svg xml:base='/elsewhere/'><g style=\"fill:url('paint.svg')\"></g></svg><p style=\"background:url('normal.svg')\"></p>");
        assert_eq!(out.file_resources.len(), 2, "{:?}", out.file_resources);
        assert!(
            out.file_resources
                .iter()
                .find(|s| s.target == "paint.svg")
                .unwrap()
                .base_unresolved
        );
        assert!(
            !out.file_resources
                .iter()
                .find(|s| s.target == "normal.svg")
                .unwrap()
                .base_unresolved
        );
        let base = parsed("<p style=\"background:url('a.svg')\"></p><base href='/deploy/'>");
        assert!(base.file_resources.iter().all(|s| s.base_unresolved));
        let dynamic = parsed("<p style=\"background:url('{{asset}}')\"></p>");
        let mut docs = HashMap::new();
        docs.insert("page".into(), dynamic);
        assert!(edges(&docs).iter().all(|e| e.candidates.is_empty()));
    }
    #[test]
    fn byte_depth_and_property_evidence_limits_are_explicit_and_leave_neighbors_usable() {
        let large = format!(
            "<p style=\"{}background:url('over.svg')\"></p><img src='ordinary.svg'>",
            "color:red;".repeat(MAX_ATTRIBUTE_BYTES / 10 + 1)
        );
        let out = parsed(&large);
        assert!(out
            .binding_limitations
            .iter()
            .any(|c| c == "INLINE_STYLE_ATTRIBUTE_BYTE_BUDGET_EXCEEDED"));
        assert_eq!(out.file_resources.len(), 1);
        let deep = format!(
            "<p style=\"background:{}url('deep.svg'){}; mask-image:url('valid.svg')\"></p>",
            "var(".repeat(150),
            ")".repeat(150)
        );
        let out = parsed(&deep);
        assert_eq!(out.file_resources.len(), 1, "{:?}", out.file_resources);
        assert_eq!(out.file_resources[0].target, "valid.svg");
        let property = "a".repeat(200);
        let out = parsed(&format!("<p style=\"{property}:url('bounded.svg')\"></p>"));
        let inline = out.file_resources[0].inline.as_ref().unwrap();
        assert!(inline.property_truncated);
        assert_eq!(inline.property.len(), 128);
        assert_eq!(inline.property_hash, content_hash(&property));
    }
    #[test]
    fn inline_resource_impact_liveness_refresh_and_frozen_property_diffs() {
        use crate::models::Document;
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("inline.duckdb")).unwrap();
        let doc = |id: &str, path: &str, body: &str| Document {
            id: id.into(),
            uri: format!("file://{id}"),
            source_file: Some(path.into()),
            title: path.into(),
            content: body.into(),
            ..Default::default()
        };
        let mut page=doc("page","/p/page.html","<div style=\"background:url(&quot;a.svg&quot;);mask-image:image-set('a.svg' 1x,'b.svg' 2x)\"></div>");
        for d in [
            page.clone(),
            doc("a", "/p/a.svg", "<svg/>"),
            doc("b", "/p/b.svg", "<svg/>"),
        ] {
            store.upsert_document(&d).unwrap();
        }
        store.index_code().unwrap();
        let q = CodeQuery {
            impact_file: Some("/p/b.svg".into()),
            ..Default::default()
        };
        let result = store.code_snapshot().unwrap().query("impact", &q);
        assert_eq!(result["items"][0]["path"], "/p/page.html", "{result}");
        assert_eq!(
            result["items"][0]["via"]["inline"]["property"],
            "mask-image"
        );
        let dead = store.code_dead_files(&CodeQuery::default()).unwrap();
        assert!(
            dead["protected"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["file"]["document_id"] == "b"),
            "{dead}"
        );
        page.content = page.content.replace("2x", "3x");
        store.upsert_document(&page).unwrap();
        assert!(store.code_snapshot().unwrap().query("impact", &q)["items"]
            .as_array()
            .unwrap()
            .is_empty());
        store.index_code().unwrap();
        let dq = history::DiffQuery {
            before: 1,
            after: Some(2),
            project: None,
            limit: None,
        };
        let diff = store.code_index_diff(&dq).unwrap();
        assert_eq!(diff["inline_style_relations_available"], true);
        assert!(
            diff["relations_added"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["choice"]["descriptors"][0] == "3x"),
            "{diff}"
        );
        page.content = format!("\n\n{}", page.content.replace("&quot;", "&#34;"));
        store.upsert_document(&page).unwrap();
        store.index_code().unwrap();
        assert_eq!(store.code_index_diff(&dq).unwrap(), diff);
        let moved = store
            .code_index_diff(&history::DiffQuery {
                before: 2,
                after: Some(3),
                project: None,
                limit: None,
            })
            .unwrap();
        assert_eq!(moved["relations_added_total"], 0);
        assert_eq!(moved["relations_removed_total"], 0);
        page.content = page.content.replace("background:", "list-style-image:");
        store.upsert_document(&page).unwrap();
        store.index_code().unwrap();
        let roles = store
            .code_index_diff(&history::DiffQuery {
                before: 3,
                after: Some(4),
                project: None,
                limit: None,
            })
            .unwrap();
        assert!(
            roles["relations_added"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["inline"]["property"] == "list-style-image"),
            "{roles}"
        );
        let c = store.lock().unwrap();
        let raw: String = c
            .query_row(
                "SELECT graph_json FROM code_index_revisions WHERE id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut graph: serde_json::Value = serde_json::from_str(&raw).unwrap();
        graph
            .as_object_mut()
            .unwrap()
            .remove("inline_style_resources_format");
        c.execute(
            "UPDATE code_index_revisions SET graph_json=? WHERE id=1",
            [graph.to_string()],
        )
        .unwrap();
        drop(c);
        let legacy = store.code_index_diff(&dq).unwrap();
        assert_eq!(legacy["inline_style_relations_available"], false);
        assert_eq!(legacy["relations_added_total"], 0);
        assert_eq!(legacy["relations_removed_total"], 0);
    }
}
