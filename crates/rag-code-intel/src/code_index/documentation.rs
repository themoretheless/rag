//! Source-bound documentation syntax candidates, not rendered/compiler-expanded docs.
use super::*;
const MAX_CHARS: usize = 4096;
const MAX_SEGMENTS: usize = 64;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentationSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Documentation {
    pub kind: String,
    /// Raw source segments joined with newlines; no escape/macro/tag evaluation.
    pub text: String,
    pub text_truncated: bool,
    pub raw_chars: usize,
    pub content_hash: String,
    pub segments: Vec<DocumentationSpan>,
    pub total_segments: usize,
    pub segments_truncated: bool,
}
fn children(n: Node<'_>) -> Vec<Node<'_>> {
    let mut c = n.walk();
    n.named_children(&mut c)
        .filter(|n| n.kind() != "comment")
        .collect()
}
fn python_string(n: Node, source: &str) -> bool {
    if n.kind() != "string" {
        return false;
    }
    let raw = text(n, source);
    let Some(quote) = raw.find(['\'', '"']) else {
        return false;
    };
    raw[..quote]
        .chars()
        .all(|c| matches!(c, 'r' | 'R' | 'u' | 'U'))
}
fn python<'a>(n: Node<'a>, source: &str) -> Vec<Node<'a>> {
    let Some(body) = n.child_by_field_name("body") else {
        return vec![];
    };
    let Some(first) = children(body).first().copied() else {
        return vec![];
    };
    if first.kind() != "expression_statement" {
        return vec![];
    }
    let Some(mut value) = children(first).first().copied() else {
        return vec![];
    };
    while value.kind() == "parenthesized_expression" {
        let c = children(value);
        if c.len() != 1 {
            return vec![];
        }
        value = c[0];
    }
    if python_string(value, source) {
        return vec![value];
    }
    if value.kind() == "concatenated_string" {
        let strings = children(value);
        if !strings.is_empty() && strings.iter().all(|n| python_string(*n, source)) {
            return strings;
        }
    }
    vec![]
}
fn leading<'a>(n: Node<'a>, source: &str, rust: bool) -> (bool, Vec<Node<'a>>) {
    let mut anchor = n;
    // Export and single variable declarations own their leading trivia.
    if n.kind() == "variable_declarator" {
        let Some(parent) = n.parent() else {
            return (false, vec![]);
        };
        if children(parent)
            .iter()
            .filter(|n| n.kind() == "variable_declarator")
            .count()
            != 1
        {
            return (false, vec![]);
        }
        anchor = parent;
    }
    if anchor
        .parent()
        .is_some_and(|p| p.kind() == "export_statement")
    {
        anchor = anchor.parent().unwrap()
    }
    let mut previous = anchor.prev_named_sibling();
    let mut docs = vec![];
    let mut examined = true;
    while let Some(p) = previous {
        let raw = text(p, source).trim();
        if rust && p.kind() == "attribute_item" {
            if raw
                .chars()
                .filter(|c| !c.is_whitespace())
                .take(5)
                .eq("#[doc".chars())
            {
                examined = false;
            }
        } else if rust
            && ((raw.starts_with("///") && !raw.starts_with("////"))
                || (raw.starts_with("/**") && !raw.starts_with("/***")))
        {
            docs.push(p);
        } else if !rust && matches!(p.kind(), "comment" | "block_comment") && raw.starts_with("/**")
        {
            // File/virtual-symbol tags cannot be attributed to this declaration.
            if ["@file", "@module", "@typedef", "@callback", "@namespace"]
                .iter()
                .any(|tag| raw.contains(tag))
            {
                return (false, vec![]);
            }
            docs.push(p);
            break;
        } else if rust && matches!(p.kind(), "line_comment" | "block_comment") {
            // Ordinary comments do not become docs or sever Rust outer attributes.
        } else if matches!(p.kind(), "decorator" | "annotation") {
        } else {
            break;
        }
        previous = p.prev_named_sibling();
    }
    docs.reverse();
    (examined, docs)
}
fn build(kind: &str, nodes: &[Node], source: &str) -> Option<Documentation> {
    if nodes.is_empty() {
        return None;
    }
    let mut hash = blake3::Hasher::new();
    let mut excerpt = String::new();
    let mut raw_chars = 0;
    let mut kept = 0;
    for (i, n) in nodes.iter().enumerate() {
        let raw = text(*n, source);
        if i > 0 {
            hash.update(b"\n");
            raw_chars += 1;
            if kept < MAX_CHARS {
                excerpt.push('\n');
                kept += 1;
            }
        }
        hash.update(raw.as_bytes());
        raw_chars += raw.chars().count();
        for c in raw.chars().take(MAX_CHARS - kept) {
            excerpt.push(c);
            kept += 1;
        }
    }
    Some(Documentation {
        kind: kind.into(),
        text: excerpt,
        text_truncated: raw_chars > MAX_CHARS,
        raw_chars,
        content_hash: hash.finalize().to_hex().to_string(),
        segments: nodes
            .iter()
            .take(MAX_SEGMENTS)
            .map(|n| DocumentationSpan {
                start_byte: n.start_byte(),
                end_byte: n.end_byte(),
                start_line: n.start_position().row + 1,
                end_line: n.end_position().row + 1,
            })
            .collect(),
        total_segments: nodes.len(),
        segments_truncated: nodes.len() > MAX_SEGMENTS,
    })
}
pub(super) fn attach(root: Node, source: &str, out: &mut IndexedDocument) {
    let extension = out.path.rsplit('.').next().unwrap_or("").to_owned();
    let ext = extension.as_str();
    if !matches!(
        ext,
        "rs" | "py" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "java" | "php" | "phtml"
    ) {
        return;
    }
    let ids: HashMap<_, _> = out
        .symbols
        .iter()
        .enumerate()
        .map(|(i, s)| ((s.start_byte, s.kind.clone()), i))
        .collect();
    let mut stack = vec![(root, 0)];
    while let Some((n, depth)) = stack.pop() {
        if depth >= 256 {
            continue;
        }
        if let Some(i) = ids.get(&(n.start_byte(), n.kind().to_string())) {
            let eligible = if ext == "py" {
                matches!(n.kind(), "function_definition" | "class_definition")
            } else {
                true
            };
            if eligible {
                let (examined, nodes, kind) = if ext == "py" {
                    (true, python(n, source), "python_literal_docstring")
                } else {
                    let (known, nodes) = leading(n, source, ext == "rs");
                    (
                        known,
                        nodes,
                        if ext == "rs" {
                            "rust_outer_doc"
                        } else {
                            "leading_docblock"
                        },
                    )
                };
                out.symbols[*i].documentation_examined = examined && !out.parse_errors;
                out.symbols[*i].documentation = build(kind, &nodes, source);
                php_docblocks::attach(n, &nodes, source, out, *i, examined);
            }
        }
        stack.extend(children(n).into_iter().map(|n| (n, depth + 1)));
    }
}
pub(super) fn quality(snapshot: Option<&CodeSnapshot>) -> serde_json::Value {
    let Some(s) = snapshot else {
        return serde_json::json!({"available":false,"coverage":null,"reason":"snapshot_unavailable"});
    };
    let examined = s
        .symbols
        .iter()
        .filter(|s| s.documentation_examined)
        .count();
    let documented = s
        .symbols
        .iter()
        .filter(|s| s.documentation_examined && s.documentation.is_some())
        .count();
    let facts: Vec<_> = s
        .symbols
        .iter()
        .filter_map(|s| s.documentation.as_ref())
        .collect();
    serde_json::json!({"available":examined>0,"coverage":if examined>0{Some(documented as f64/examined as f64)}else{None},"scope":"supported_syntax_forms_in_fresh_scoped_symbols","examined_symbols":examined,"documented_symbols":documented,"uninspected_symbols":s.symbols.len()-examined,"observed_doc_facts":facts.len(),"truncated_texts":facts.iter().filter(|d|d.text_truncated).count(),"truncated_segment_lists":facts.iter().filter(|d|d.segments_truncated).count(),"compiler_or_rendered_semantics_verified":false,"full_language_coverage_verified":false})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parsed(path: &str, source: &str) -> IndexedDocument {
        let doc = super::super::parse(path, path, source, 1).unwrap();
        assert!(!doc.parse_errors, "{source}");
        doc
    }
    #[test]
    fn python_literal_docstrings_exclude_bytes_fstrings_and_late_expressions() {
        let doc=parsed("/p/a.py","class A:\n    \"\"\"class documentation\"\"\"\n    @decorator\n    def method(self):\n        (\"first \" \"second\")\n    def bytes(self):\n        b'not docs'\n    def dynamic(self):\n        f'not {self}'\n    def late(self):\n        x=1\n        'not first'\n");
        assert_eq!(doc.symbols.len(), 5);
        assert!(doc.symbols.iter().all(|s| s.documentation_examined));
        assert_eq!(
            doc.symbols
                .iter()
                .filter(|s| s.documentation.is_some())
                .count(),
            2
        );
        let method = doc
            .symbols
            .iter()
            .find(|s| s.name == "method")
            .unwrap()
            .documentation
            .as_ref()
            .unwrap();
        assert_eq!(method.total_segments, 2);
        assert!(method.text.contains("first"));
        assert!(method.text.contains("second"));
    }
    #[test]
    fn rust_outer_markers_preserve_source_spans_and_skip_inner_or_unknown_attributes() {
        let source="//! module docs\n/// function λ\n// ordinary note\n#[inline]\nfn documented(){}\nfn bare(){}\n#[ doc = \"attribute docs\"]\nfn unknown(){}";
        let doc = parsed("/p/a.rs", source);
        let f = doc.symbols.iter().find(|s| s.name == "documented").unwrap();
        let d = f.documentation.as_ref().unwrap();
        assert!(f.documentation_examined);
        assert!(d.text.contains("function λ"));
        assert!(!d.text.contains("module docs"));
        assert!(!d.text.contains("ordinary note"));
        let raw = d
            .segments
            .iter()
            .map(|r| &source[r.start_byte..r.end_byte])
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(raw, d.text);
        assert_eq!(content_hash(&raw), d.content_hash);
        assert_eq!(d.segments[0].start_line, 2);
        assert!(doc
            .symbols
            .iter()
            .find(|s| s.name == "bare")
            .unwrap()
            .documentation
            .is_none());
        assert!(
            !doc.symbols
                .iter()
                .find(|s| s.name == "unknown")
                .unwrap()
                .documentation_examined
        );
    }
    #[test]
    fn leading_blocks_follow_export_and_single_arrow_ownership_not_file_or_multi_decl_tags() {
        let doc=parsed("/p/a.ts","/** named */ export function f() {}\n/** arrow */ export const arrow=()=>1;\n/** ambiguous */ const a=()=>1,b=()=>2;\n/** @file library */ function file_header(){}\nexport class C { /** method */ run(){} }\n");
        for name in ["f", "arrow", "run"] {
            assert!(
                doc.symbols
                    .iter()
                    .find(|s| s.name == name)
                    .unwrap()
                    .documentation
                    .is_some(),
                "{name}"
            );
        }
        for name in ["a", "b", "file_header"] {
            let s = doc.symbols.iter().find(|s| s.name == name).unwrap();
            assert!(!s.documentation_examined);
            assert!(s.documentation.is_none());
        }
    }
    #[test]
    fn java_and_php_leading_docblocks_attach_to_classes_and_methods() {
        for (path, source) in [
            (
                "/p/C.java",
                "/** class */ class C { /** method */ public void run() {} }",
            ),
            (
                "/p/C.php",
                "<?php /** class */ class C { /** method */ public function run() {} }",
            ),
        ] {
            let doc = parsed(path, source);
            assert_eq!(doc.symbols.len(), 2);
            assert!(
                doc.symbols
                    .iter()
                    .all(|s| s.documentation_examined && s.documentation.is_some()),
                "{path}: {:?}",
                doc.symbols
            );
        }
    }
    #[test]
    fn unicode_excerpt_and_segment_budgets_have_full_hash_and_explicit_truncation() {
        let source = format!("/** {} */\nfn f(){{}}", "λ".repeat(5000));
        let doc = parsed("/p/a.rs", &source);
        let d = doc.symbols[0].documentation.as_ref().unwrap();
        assert_eq!(d.text.chars().count(), MAX_CHARS);
        assert!(d.text_truncated);
        assert!(d.raw_chars > MAX_CHARS);
        assert_eq!(
            content_hash(&source[d.segments[0].start_byte..d.segments[0].end_byte]),
            d.content_hash
        );
        let source = format!("{}fn f(){{}}", "/** x */\n".repeat(70));
        let doc = parsed("/p/b.rs", &source);
        let d = doc.symbols[0].documentation.as_ref().unwrap();
        assert_eq!(d.total_segments, 70);
        assert_eq!(d.segments.len(), MAX_SEGMENTS);
        assert!(d.segments_truncated);
        assert!(!d.text_truncated);
    }
    #[test]
    fn coverage_counts_only_inspected_forms_and_never_returns_doc_text() {
        let mut snapshot = CodeSnapshot::default();
        snapshot.symbols.extend(
            parsed(
                "/p/a.py",
                "def a():\n    'secret documentation'\ndef b(): pass\n",
            )
            .symbols,
        );
        snapshot
            .symbols
            .extend(parsed("/p/c.lua", "local function c() end").symbols);
        let measured = quality(Some(&snapshot));
        assert_eq!(measured["examined_symbols"], 2);
        assert_eq!(measured["documented_symbols"], 1);
        assert_eq!(measured["uninspected_symbols"], 1);
        assert_eq!(measured["coverage"], 0.5);
        assert!(!measured.to_string().contains("secret documentation"));
        assert_eq!(measured["full_language_coverage_verified"], false);
        assert!(quality(Some(&CodeSnapshot::default()))["coverage"].is_null());
        assert!(quality(None)["coverage"].is_null());
    }
}
