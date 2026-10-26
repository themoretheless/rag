//! Responsive image syntax candidates; never a browser source-selection result.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub syntax: String,
    pub descriptors: Vec<String>,
    pub descriptors_truncated: bool,
    pub descriptors_hash: String,
    pub type_hint: Option<String>,
    pub source_anchor: String,
    pub source_end_byte: usize,
    pub descriptor_validation: String,
    pub selection_proven: bool,
}
fn choice(
    syntax: &str,
    descriptors: &[&str],
    type_hint: Option<String>,
    anchor: &str,
    end: usize,
    validation: &str,
) -> Choice {
    Choice {
        syntax: syntax.into(),
        descriptors: descriptors
            .iter()
            .map(|s| s.chars().take(128).collect())
            .collect(),
        descriptors_truncated: descriptors.iter().any(|s| s.chars().count() > 128),
        descriptors_hash: content_hash(&serde_json::to_string(descriptors).unwrap()),
        type_hint,
        source_anchor: anchor.into(),
        source_end_byte: end,
        descriptor_validation: validation.into(),
        selection_proven: false,
    }
}
fn limitation(out: &mut IndexedDocument, code: &str) {
    if !out.binding_limitations.iter().any(|v| v == code) {
        out.binding_limitations.push(code.into());
    }
}
fn whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0x0c)
}
struct Candidate<'a> {
    url: &'a str,
    descriptors: Vec<&'a str>,
    overflow: bool,
}
struct Srcset<'a> {
    input: &'a str,
    position: usize,
}
impl<'a> Iterator for Srcset<'a> {
    type Item = Candidate<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        let bytes = self.input.as_bytes();
        while self.position < bytes.len()
            && (whitespace(bytes[self.position]) || bytes[self.position] == b',')
        {
            self.position += 1;
        }
        if self.position == bytes.len() {
            return None;
        }
        let start = self.position;
        while self.position < bytes.len() && !whitespace(bytes[self.position]) {
            self.position += 1;
        }
        let mut url = &self.input[start..self.position];
        let mut descriptors = Vec::new();
        let mut overflow = false;
        if url.ends_with(',') {
            url = url.trim_end_matches(',');
        } else {
            let mut start = None;
            // 0=in descriptor, 1=in parentheses, 2=after descriptor.
            let mut state = 0;
            loop {
                let b = bytes.get(self.position).copied();
                if b.is_none() {
                    if let Some(start) = start {
                        if descriptors.len() < 4 {
                            descriptors.push(&self.input[start..self.position]);
                        } else {
                            overflow = true;
                        }
                    }
                    break;
                }
                let b = b.unwrap();
                if state == 2 {
                    if whitespace(b) {
                        self.position += 1;
                        continue;
                    }
                    state = 0;
                    continue;
                }
                if state == 0 && (whitespace(b) || b == b',') {
                    if let Some(start) = start.take() {
                        if descriptors.len() < 4 {
                            descriptors.push(&self.input[start..self.position]);
                        } else {
                            overflow = true;
                        }
                    }
                    self.position += 1;
                    if b == b',' {
                        break;
                    }
                    state = 2;
                    continue;
                }
                if start.is_none() {
                    start = Some(self.position);
                }
                if b == b'(' && state == 0 {
                    state = 1;
                } else if b == b')' && state == 1 {
                    state = 0;
                }
                self.position += 1;
            }
        }
        Some(Candidate {
            url,
            descriptors,
            overflow,
        })
    }
}
fn positive_integer(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.bytes().any(|b| b != b'0')
}
fn html_float(value: &str) -> bool {
    use std::sync::OnceLock;
    static FLOAT: OnceLock<regex::Regex> = OnceLock::new();
    FLOAT
        .get_or_init(|| {
            regex::Regex::new(r"^-?(?:[0-9]+(?:\.[0-9]+)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?$").unwrap()
        })
        .is_match(value)
        && value
            .parse::<f64>()
            .is_ok_and(|v| v.is_finite() && v >= 0.0)
}
fn valid_descriptors(candidate: &Candidate<'_>) -> bool {
    if candidate.overflow {
        return false;
    }
    let (mut width, mut density, mut height) = (false, false, false);
    for descriptor in &candidate.descriptors {
        if let Some(value) = descriptor.strip_suffix('w') {
            if width || density || !positive_integer(value) {
                return false;
            }
            width = true;
        } else if let Some(value) = descriptor.strip_suffix('x') {
            if width || density || height || !html_float(value) {
                return false;
            }
            density = true;
        } else if let Some(value) = descriptor.strip_suffix('h') {
            if height || density || !positive_integer(value) {
                return false;
            }
            height = true;
        } else {
            return false;
        }
    }
    !height || width
}
pub(super) fn markup(node: Node<'_>, tag: &str, source: &str, out: &mut IndexedDocument) {
    let name = match tag {
        "img" | "source" => "srcset",
        "link" => "imagesrcset",
        _ => return,
    };
    let Some((offset, end, value)) = attribute(node, name, source) else {
        return;
    };
    for candidate in (Srcset {
        input: &value,
        position: 0,
    }) {
        if !valid_descriptors(&candidate) {
            limitation(out, "SRCSET_INVALID_OR_UNREPRESENTABLE_DESCRIPTOR_OMITTED");
            continue;
        }
        let capped = out.file_resources.len() >= MAX_RESOURCE_FACTS;
        record(
            out,
            source,
            offset,
            candidate.url,
            &format!("html:{tag}:{name}"),
            Some(choice(
                "html_srcset",
                &candidate.descriptors,
                None,
                "html_attribute_value",
                end,
                "whatwg_descriptor_parser",
            )),
        );
        if capped {
            break;
        }
    }
}
fn function(node: Node<'_>, source: &str) -> Option<String> {
    (node.kind() == "call_expression")
        .then(|| {
            children(node)
                .into_iter()
                .find(|n| n.kind() == "function_name")
                .map(|n| text(n, source).to_ascii_lowercase())
        })
        .flatten()
}
fn is_set(node: Node<'_>, source: &str) -> bool {
    function(node, source).is_some_and(|n| matches!(n.as_str(), "image-set" | "-webkit-image-set"))
}
fn in_set(node: Node<'_>, source: &str) -> bool {
    let mut ancestor = node.parent();
    while let Some(p) = ancestor {
        if is_set(p, source) {
            return true;
        }
        if p.kind() == "declaration" {
            break;
        }
        ancestor = p.parent();
    }
    false
}
fn in_declaration(node: Node<'_>) -> bool {
    let mut ancestor = node.parent();
    while let Some(p) = ancestor {
        if p.kind() == "declaration" {
            return true;
        }
        if p.kind() == "import_statement" {
            break;
        }
        ancestor = p.parent();
    }
    false
}
fn literal_url<'a, 'tree>(node: Node<'tree>, source: &'a str) -> Option<(&'a str, Node<'tree>)> {
    if let Some(value) = plain_string(node, source) {
        return Some((value, node));
    }
    if function(node, source).as_deref() != Some("url") {
        return None;
    }
    let args = children(node)
        .into_iter()
        .find(|n| n.kind() == "arguments")?;
    let values: Vec<_> = children(args)
        .into_iter()
        .filter(|n| n.kind() != "comment")
        .collect();
    if values.len() != 1 {
        return None;
    }
    let value = values[0];
    plain_string(value, source)
        .or_else(|| {
            (value.kind() == "plain_value" && !text(value, source).contains('\\'))
                .then(|| text(value, source))
        })
        .map(|url| (url, value))
}
fn type_hint(node: Node<'_>, source: &str) -> Option<String> {
    if function(node, source).as_deref() != Some("type") {
        return None;
    }
    let args = children(node)
        .into_iter()
        .find(|n| n.kind() == "arguments")?;
    let values: Vec<_> = children(args)
        .into_iter()
        .filter(|n| n.kind() != "comment")
        .collect();
    if values.len() != 1 {
        return None;
    }
    let value = plain_string(values[0], source)?;
    (value.len() <= 256).then(|| value.into())
}
fn resolution(node: Node<'_>, source: &str) -> bool {
    if !matches!(node.kind(), "integer_value" | "float_value") {
        return false;
    }
    let raw = text(node, source).to_ascii_lowercase();
    for unit in ["dppx", "dpcm", "dpi", "x"] {
        if let Some(value) = raw.strip_suffix(unit) {
            return value
                .parse::<f64>()
                .is_ok_and(|v| v.is_finite() && v >= 0.0);
        }
    }
    false
}
/// True means this call or its descendants belong to image-set handling; the
/// generic URL collector must not independently invent links for rejected options.
pub(super) fn css(
    node: Node<'_>,
    fragment: &str,
    full: &str,
    base: usize,
    out: &mut IndexedDocument,
) -> bool {
    if node.kind() != "call_expression" {
        return false;
    }
    if in_set(node, fragment) {
        return true;
    }
    if !is_set(node, fragment) {
        return false;
    }
    if !in_declaration(node) {
        return true;
    }
    let Some(args) = children(node).into_iter().find(|n| n.kind() == "arguments") else {
        return true;
    };
    let mut cursor = args.walk();
    let mut groups = vec![Vec::new()];
    for child in args.children(&mut cursor) {
        if child.kind() == "," {
            groups.push(Vec::new());
        } else if child.is_named() && child.kind() != "comment" {
            groups.last_mut().unwrap().push(child);
        }
    }
    for group in groups {
        let Some(first) = group.first().copied() else {
            limitation(out, "CSS_IMAGE_SET_OPTION_UNVERIFIED");
            continue;
        };
        let Some((url, literal)) = literal_url(first, fragment) else {
            limitation(out, "CSS_IMAGE_SET_GENERATED_OR_DYNAMIC_IMAGE_UNVERIFIED");
            continue;
        };
        let (mut density, mut mime) = (None, None);
        let mut valid = true;
        let mut validation = "literal_resolution_and_type_shape_only";
        for part in group.iter().skip(1) {
            if resolution(*part, fragment) && density.is_none() {
                density = Some(text(*part, fragment));
            } else if mime.is_none() && function(*part, fragment).as_deref() == Some("type") {
                if let Some(value) = type_hint(*part, fragment) {
                    mime = Some(value);
                } else {
                    valid = false;
                }
            } else if density.is_none()
                && function(*part, fragment).is_some_and(|n| {
                    matches!(
                        n.as_str(),
                        "var" | "calc" | "min" | "max" | "clamp" | "attr" | "env"
                    )
                })
            {
                density = Some(text(*part, fragment));
                validation = "resolution_expression_unverified";
            } else {
                valid = false;
            }
        }
        if !valid {
            limitation(out, "CSS_IMAGE_SET_OPTION_UNVERIFIED");
            continue;
        }
        let descriptors: Vec<_> = density.into_iter().collect();
        let capped = out.file_resources.len() >= MAX_RESOURCE_FACTS;
        record(
            out,
            full,
            base + literal.start_byte(),
            url,
            "css:image-set",
            Some(choice(
                "css_image_set",
                &descriptors,
                mime,
                "css_image_literal",
                base + literal.end_byte(),
                validation,
            )),
        );
        if capped {
            break;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parsed(path: &str, source: &str) -> IndexedDocument {
        let d = parse("doc", path, source, 1).unwrap();
        assert!(!d.parse_errors, "{path}: {source}");
        d
    }
    #[test]
    fn srcset_tokenizer_preserves_url_commas_and_skips_invalid_descriptor_groups() {
        let source="data:image/svg+xml,a,b 1x, wide.svg 640w 480h, zero.svg 0x, exponent.svg 1e+2x, bad.svg 1x 2x, plus.svg +1x, empty.svg 0w, unknown.svg calc(1,2), final.svg 2x";
        let got: Vec<_> = (Srcset {
            input: source,
            position: 0,
        })
        .filter(valid_descriptors)
        .map(|v| v.url)
        .collect();
        assert_eq!(
            got,
            [
                "data:image/svg+xml,a,b",
                "wide.svg",
                "zero.svg",
                "exponent.svg",
                "final.svg"
            ]
        );
        let got: Vec<_> = (Srcset {
            input: "a.svg,b.svg 2x, c.svg, d.svg 1x",
            position: 0,
        })
        .map(|v| v.url)
        .collect();
        assert_eq!(got, ["a.svg,b.svg", "c.svg", "d.svg"]);
    }
    #[test]
    fn html_srcset_sources_and_preloads_capture_choices_and_original_attribute_span() {
        let source="<picture><source srcset='small.svg 400w, large.svg 800w'><img srcset=\"plain.svg?x=1&amp;y=2 1x, dense.svg 2x\"></picture><link rel='preload' as='image' imagesrcset='preload.svg 1x'>";
        let doc = parsed("/p/view.html", source);
        assert_eq!(doc.file_resources.len(), 5);
        let dense = doc
            .file_resources
            .iter()
            .find(|s| s.target == "dense.svg")
            .unwrap();
        let evidence = dense.choice.as_ref().unwrap();
        assert_eq!(evidence.descriptors, ["2x"]);
        assert_eq!(evidence.source_anchor, "html_attribute_value");
        assert!(!evidence.selection_proven);
        assert_eq!(
            &source[dense.start_byte..evidence.source_end_byte],
            "\"plain.svg?x=1&amp;y=2 1x, dense.svg 2x\""
        );
        assert!(doc
            .file_resources
            .iter()
            .any(|s| s.target == "plain.svg?x=1&y=2"));
        assert!(doc
            .file_resources
            .iter()
            .any(|s| s.reference_kind == "html:link:imagesrcset"));
    }
    #[test]
    fn invalid_or_duplicate_srcset_attributes_and_template_urls_never_bind() {
        let doc=parsed("/p/view.html","<img srcset='valid.svg 1x, invalid.svg -1x, both.svg 2x 300w, solo.svg 40h, parens.svg future(a,b)'><img srcset='a.svg 1x' SRCSET='b.svg 2x'><img srcset='{{dynamic}} 1x'>");
        assert_eq!(doc.file_resources.len(), 2);
        assert!(doc
            .binding_limitations
            .iter()
            .any(|s| s == "SRCSET_INVALID_OR_UNREPRESENTABLE_DESCRIPTOR_OMITTED"));
        let mut documents = HashMap::from([("doc".into(), doc)]);
        for (id, path) in [("valid", "/p/valid.svg"), ("invalid", "/p/invalid.svg")] {
            // Use indexable targets to test path binding independently of parser coverage.
            let mut target = parsed("/p/empty.css", "");
            target.path = path.into();
            documents.insert(id.into(), target);
        }
        let edges = edges(&documents);
        assert_eq!(
            edges.iter().filter(|e| !e.candidates.is_empty()).count(),
            1,
            "{edges:?}"
        );
    }
    #[test]
    fn css_image_sets_keep_string_and_url_images_without_type_hint_false_links() {
        let source=".a{background:image-set('small.svg' 1x type('image/svg+xml'), url('large.svg') 2x)} .b{background:-webkit-image-set('print.svg' 192DPI)} .c{background:image-set('computed.svg' calc(1dppx + 1dppx))}";
        let doc = parsed("/p/main.css", source);
        assert_eq!(doc.file_resources.len(), 4, "{:?}", doc.file_resources);
        assert!(doc
            .file_resources
            .iter()
            .all(|s| s.reference_kind == "css:image-set"));
        let first = doc
            .file_resources
            .iter()
            .find(|s| s.target == "small.svg")
            .unwrap();
        assert_eq!(
            first.choice.as_ref().unwrap().type_hint.as_deref(),
            Some("image/svg+xml")
        );
        let computed = doc
            .file_resources
            .iter()
            .find(|s| s.target == "computed.svg")
            .unwrap();
        assert_eq!(
            computed.choice.as_ref().unwrap().descriptor_validation,
            "resolution_expression_unverified"
        );
        assert_eq!(
            &source[first.start_byte..first.choice.as_ref().unwrap().source_end_byte],
            "'small.svg'"
        );
    }
    #[test]
    fn rejected_image_set_options_do_not_leak_generic_urls_or_condition_links() {
        let doc=parsed("/p/main.css",".a{background:image-set(url('negative.svg') -1x, url('duplicate.svg') 1x 2x, image-set(url('nested.svg') 2x) 1x)} @supports (background:image-set(url('condition.svg') 1x)) {.b{background:url('real.svg')}}");
        assert_eq!(doc.file_resources.len(), 1, "{:?}", doc.file_resources);
        assert_eq!(doc.file_resources[0].target, "real.svg");
    }
    #[test]
    fn descriptors_are_bounded_but_full_descriptor_hash_and_inline_offsets_are_preserved() {
        let width = "1".repeat(400);
        let source=format!("<img srcset='a.svg {width}w'>\n<style>\n.a{{background:image-set('b.svg' 2x)}}\n</style>");
        let doc = parsed("/p/view.html", &source);
        let a = doc
            .file_resources
            .iter()
            .find(|s| s.target == "a.svg")
            .unwrap();
        let choice = a.choice.as_ref().unwrap();
        assert!(choice.descriptors_truncated);
        assert_eq!(choice.descriptors[0].len(), 128);
        let b = doc
            .file_resources
            .iter()
            .find(|s| s.target == "b.svg")
            .unwrap();
        assert_eq!(b.line, 3);
        assert_eq!(
            &source[b.start_byte..b.choice.as_ref().unwrap().source_end_byte],
            "'b.svg'"
        );
        let other = parsed("/p/view.html", &format!("<img srcset='a.svg {width}2w'>"));
        assert_ne!(
            choice.descriptors_hash,
            other.file_resources[0]
                .choice
                .as_ref()
                .unwrap()
                .descriptors_hash
        );
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::models::Document;
    #[test]
    fn responsive_choices_drive_impact_liveness_and_immutable_descriptor_diffs() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(&temp.path().join("responsive.duckdb")).unwrap();
        let doc = |id: &str, path: &str, source: &str| Document {
            id: id.into(),
            uri: format!("file://{id}"),
            source_file: Some(path.into()),
            content: source.into(),
            title: path.into(),
            ..Default::default()
        };
        let mut page = doc("page", "/p/page.html", "<img srcset='a.svg 1x, b.svg 2x'>");
        for d in [
            page.clone(),
            doc(
                "css",
                "/p/main.css",
                ".a{background:image-set('a.svg' 1x type('image/svg+xml'), url('b.svg') 2x)}",
            ),
            doc("a", "/p/a.svg", "<svg/>"),
            doc("b", "/p/b.svg", "<svg/>"),
            doc("mime", "/p/image/svg+xml", "not an image"),
        ] {
            store.upsert_document(&d).unwrap();
        }
        store.index_code().unwrap();
        let snapshot = store.code_snapshot().unwrap();
        let q = CodeQuery {
            impact_file: Some("/p/b.svg".into()),
            ..Default::default()
        };
        let impact = snapshot.query("impact", &q);
        assert_eq!(impact["items"].as_array().unwrap().len(), 2, "{impact}");
        assert!(impact["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["via"]["choice"]["selection_proven"] == false));
        assert!(snapshot.query(
            "impact",
            &CodeQuery {
                impact_file: Some("/p/image/svg+xml".into()),
                ..Default::default()
            }
        )["items"]
            .as_array()
            .unwrap()
            .is_empty());
        let dead = store.code_dead_files(&CodeQuery::default()).unwrap();
        for id in ["a", "b"] {
            assert!(
                dead["protected"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["file"]["document_id"] == id),
                "{dead}"
            );
        }
        page.content = "<img srcset='a.svg 1x, b.svg 3x'>".into();
        store.upsert_document(&page).unwrap();
        store.index_code().unwrap();
        let q = history::DiffQuery {
            before: 1,
            after: Some(2),
            project: None,
            limit: None,
        };
        let diff = store.code_index_diff(&q).unwrap();
        assert_eq!(diff["responsive_resource_relations_available"], true);
        assert!(
            diff["relations_removed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["url"] == "b.svg" && e["choice"]["descriptors"][0] == "2x"),
            "{diff}"
        );
        assert!(
            diff["relations_added"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["url"] == "b.svg" && e["choice"]["descriptors"][0] == "3x"),
            "{diff}"
        );
        page.content = "\n\n<img srcset='a.svg 1x, b.svg 3x'>".into();
        store.upsert_document(&page).unwrap();
        store.index_code().unwrap();
        assert_eq!(store.code_index_diff(&q).unwrap(), diff);
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
        let c = store.lock().unwrap();
        let raw: String = c
            .query_row(
                "SELECT graph_json FROM code_index_revisions WHERE id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut old: serde_json::Value = serde_json::from_str(&raw).unwrap();
        old.as_object_mut()
            .unwrap()
            .remove("responsive_resources_format");
        old["format_version"] = serde_json::json!(36);
        let mut old_url = old["file_resources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["reference_kind"] == "css:image-set" && e["target"] == "b.svg")
            .unwrap()
            .clone();
        old_url["reference_kind"] = serde_json::json!("css:url");
        old_url.as_object_mut().unwrap().remove("choice");
        old["file_resources"] = serde_json::json!([old_url]);
        c.execute(
            "UPDATE code_index_revisions SET graph_json=? WHERE id=1",
            [old.to_string()],
        )
        .unwrap();
        drop(c);
        let legacy = store.code_index_diff(&q).unwrap();
        assert_eq!(legacy["responsive_resource_relations_available"], false);
        assert_eq!(legacy["css_url_classification_comparable"], false);
        assert_eq!(legacy["relations_added_total"], 0, "{legacy}");
        assert_eq!(legacy["relations_removed_total"], 0, "{legacy}");
    }
}
