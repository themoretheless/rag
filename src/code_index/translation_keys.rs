//! Literal translation-key syntax, without claiming translator or template-engine binding.
use super::*;
use std::sync::LazyLock;
const MAX_SOURCE: usize = 2 * 1024 * 1024;
const MAX_FACTS: usize = 4096;
const MAX_KEY_CHARS: usize = 120;
const MAX_KEY_BYTES: usize = 480;
static PLACEHOLDERS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"[%@]\{([A-Za-z0-9_.\-]+)\}|\{\{([A-Za-z0-9_.\-]+)\}\}")
        .expect("constant placeholder pattern")
});
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Spec {
    pub key: String,
    #[serde(default)]
    pub key_encoding: String,
    pub usage_kind: String,
    pub syntax: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub line: usize,
    pub column: usize,
    pub owner: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Facts {
    pub format: u32,
    pub complete: bool,
    pub limit_reason: Option<String>,
    pub excluded_dynamic_or_unsupported_calls: usize,
    pub excluded_oversized_keys: usize,
    pub items: Vec<Spec>,
}
#[derive(Debug, Serialize)]
pub(super) struct Occurrence {
    pub key: String,
    pub key_encoding: String,
    pub usage_kind: String,
    pub syntax: String,
    pub document_id: String,
    pub path: String,
    pub project: Option<String>,
    pub revision: i64,
    pub content_hash: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub line: usize,
    pub column: usize,
    pub owner: Option<String>,
}
#[derive(Debug, Default, Serialize)]
pub(super) struct Snapshot {
    pub items: Vec<Occurrence>,
    pub coverage: Vec<serde_json::Value>,
}
pub(super) fn supported(path: &str) -> bool {
    matches!(
        path.rsplit('.').next(),
        Some("php" | "phtml" | "html" | "htm")
    )
}
pub(super) fn current(doc: &IndexedDocument) -> bool {
    !supported(&doc.path) || doc.translation_keys.as_ref().is_some_and(|f| f.format == 1)
}
pub(super) fn validate_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.len() > MAX_KEY_BYTES
        || key.chars().count() > MAX_KEY_CHARS
        || key.chars().any(char::is_control)
    {
        return Err(AppError::config("translation key must be nonempty, at most 120 Unicode characters (480 UTF-8 bytes) and contain no control characters"));
    }
    Ok(())
}
fn append(facts: &mut Facts, spec: Spec) {
    if facts.items.len() >= MAX_FACTS {
        facts.complete = false;
        facts.limit_reason = Some("occurrence_limit".into());
    } else {
        facts.items.push(spec);
    }
}
pub(super) fn begin(source: &str, out: &mut IndexedDocument) {
    if !supported(&out.path) {
        return;
    }
    let mut facts = Facts {
        format: 1,
        complete: true,
        limit_reason: None,
        excluded_dynamic_or_unsupported_calls: 0,
        excluded_oversized_keys: 0,
        items: Vec::new(),
    };
    if source.len() > MAX_SOURCE {
        facts.complete = false;
        facts.limit_reason = Some("source_byte_limit".into());
        out.translation_keys = Some(facts);
        return;
    }
    let mut previous = 0;
    let mut line = 1;
    let mut line_start = 0;
    for captures in PLACEHOLDERS.captures_iter(source) {
        let whole = captures.get(0).unwrap();
        let key = captures
            .get(1)
            .or_else(|| captures.get(2))
            .unwrap()
            .as_str();
        if key.len() > MAX_KEY_BYTES || key.chars().count() > MAX_KEY_CHARS {
            facts.excluded_oversized_keys += 1;
            continue;
        }
        for (offset, byte) in source.as_bytes()[previous..whole.start()]
            .iter()
            .enumerate()
        {
            if *byte == b'\n' {
                line += 1;
                line_start = previous + offset + 1;
            }
        }
        previous = whole.start();
        let syntax = if whole.as_str().starts_with("%{") {
            "%{}"
        } else if whole.as_str().starts_with("@{") {
            "@{}"
        } else {
            "{{}}"
        };
        append(
            &mut facts,
            Spec {
                key: key.into(),
                key_encoding: "literal_placeholder".into(),
                usage_kind: "template_placeholder".into(),
                syntax: syntax.into(),
                start_byte: whole.start(),
                end_byte: whole.end(),
                line,
                column: whole.start() - line_start + 1,
                owner: None,
            },
        );
        if !facts.complete {
            break;
        }
    }
    out.translation_keys = Some(facts);
}
fn decode_php_key(raw: &str, quote: u8) -> Option<String> {
    let input = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < input.len() {
        let c = input[i];
        i += 1;
        if quote == b'"'
            && c == b'$'
            && input
                .get(i)
                .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_' || *c == b'{' || *c >= 128)
        {
            return None;
        }
        if c != b'\\' {
            out.push(c);
            continue;
        }
        let next = *input.get(i)?;
        i += 1;
        if quote == b'\'' {
            if !matches!(next, b'\\' | b'\'') {
                out.push(b'\\');
            }
            out.push(next);
            continue;
        }
        match next {
            b'\\' | b'"' | b'$' => out.push(next),
            b'n' => out.push(b'\n'),
            b'r' => out.push(b'\r'),
            b't' => out.push(b'\t'),
            b'v' => out.push(11),
            b'e' => out.push(27),
            b'f' => out.push(12),
            b'0'..=b'7' => {
                let mut value = u16::from(next - b'0');
                let mut count = 1;
                while count < 3 && input.get(i).is_some_and(|c| matches!(c, b'0'..=b'7')) {
                    value = value * 8 + u16::from(input[i] - b'0');
                    i += 1;
                    count += 1;
                }
                out.push(value as u8);
            }
            b'x' => {
                let mut value = 0u8;
                let mut count = 0;
                while count < 2 && input.get(i).is_some_and(u8::is_ascii_hexdigit) {
                    value = value * 16 + (input[i] as char).to_digit(16)? as u8;
                    i += 1;
                    count += 1;
                }
                if count == 0 {
                    out.extend_from_slice(b"\\x");
                } else {
                    out.push(value);
                }
            }
            b'u' if input.get(i) == Some(&b'{') => {
                i += 1;
                let start = i;
                while input.get(i).is_some_and(u8::is_ascii_hexdigit) {
                    i += 1;
                }
                if i == start || input.get(i) != Some(&b'}') {
                    return None;
                }
                let value = u32::from_str_radix(&raw[start..i], 16).ok()?;
                i += 1;
                let mut encoded = [0u8; 4];
                out.extend_from_slice(char::from_u32(value)?.encode_utf8(&mut encoded).as_bytes());
            }
            other => {
                out.push(b'\\');
                out.push(other);
            }
        }
        if out.len() > MAX_KEY_BYTES {
            return String::from_utf8(out).ok();
        }
    }
    String::from_utf8(out).ok()
}
pub(super) fn collect(node: Node, source: &str, out: &mut IndexedDocument, owner: Option<&str>) {
    if !matches!(out.path.rsplit('.').next(), Some("php" | "phtml"))
        || node.kind() != "function_call_expression"
    {
        return;
    }
    let Some(facts) = out.translation_keys.as_mut().filter(|f| f.complete) else {
        return;
    };
    let Some(function) = node
        .child_by_field_name("function")
        .filter(|n| matches!(n.kind(), "name" | "qualified_name"))
    else {
        return;
    };
    let callee = text(function, source);
    let base = callee
        .rsplit('\\')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(
        base.as_str(),
        "translate" | "trans" | "t" | "__" | "tr" | "gettext" | "_t" | "lang"
    ) {
        return;
    }
    let mut literal = None;
    if let Some(args) = node.child_by_field_name("arguments") {
        let mut cursor = args.walk();
        let first = args
            .named_children(&mut cursor)
            .find(|n| n.kind() != "comment");
        if let Some(argument) = first {
            if argument.kind() == "argument" && argument.child_by_field_name("name").is_none() {
                let mut cursor = argument.walk();
                let children: Vec<_> = argument
                    .named_children(&mut cursor)
                    .filter(|n| n.kind() != "comment")
                    .collect();
                if children.len() == 1 && matches!(children[0].kind(), "string" | "encapsed_string")
                {
                    literal = Some(children[0]);
                }
            }
        }
    }
    let Some(literal) = literal else {
        facts.excluded_dynamic_or_unsupported_calls += 1;
        return;
    };
    let raw = text(literal, source);
    let bytes = raw.as_bytes();
    if bytes.len() < 2 || !matches!(bytes[0], b'\'' | b'"') || bytes.last() != Some(&bytes[0]) {
        facts.excluded_dynamic_or_unsupported_calls += 1;
        return;
    }
    let Some(key) = decode_php_key(&raw[1..raw.len() - 1], bytes[0]) else {
        facts.excluded_dynamic_or_unsupported_calls += 1;
        return;
    };
    if key.chars().any(char::is_control) || key.is_empty() {
        facts.excluded_dynamic_or_unsupported_calls += 1;
        return;
    }
    if key.len() > MAX_KEY_BYTES || key.chars().count() > MAX_KEY_CHARS {
        facts.excluded_oversized_keys += 1;
        return;
    }
    append(
        facts,
        Spec {
            key,
            key_encoding: if bytes[0] == b'\'' {
                "php_single_quoted"
            } else {
                "php_double_quoted"
            }
            .into(),
            usage_kind: "php_translation_call".into(),
            syntax: callee.trim_start_matches('\\').to_ascii_lowercase(),
            start_byte: literal.start_byte() + 1,
            end_byte: literal.end_byte() - 1,
            line: literal.start_position().row + 1,
            column: literal.start_position().column + 2,
            owner: owner.map(str::to_owned),
        },
    );
}
pub(super) fn snapshot(docs: &HashMap<String, IndexedDocument>) -> Snapshot {
    let mut result = Snapshot::default();
    for (id, doc) in docs {
        if !supported(&doc.path) {
            continue;
        }
        let Some(facts) = &doc.translation_keys else {
            result.coverage.push(serde_json::json!({"document_id":id,"project":doc.project,"path":doc.path,"complete":false,"status":"capture_unavailable"}));
            continue;
        };
        let complete = facts.complete && !doc.parse_errors && facts.format == 1;
        result.coverage.push(serde_json::json!({"document_id":id,"project":doc.project,"path":doc.path,"revision":doc.revision,"content_hash":doc.hash,"complete":complete,"status":if doc.parse_errors{"parse_error"}else if complete{"captured_syntax_subset"}else{"limited_or_unsupported_capture"},"limit_reason":facts.limit_reason,"excluded_dynamic_or_unsupported_calls":facts.excluded_dynamic_or_unsupported_calls,"excluded_oversized_keys":facts.excluded_oversized_keys,"occurrences_retained":if doc.parse_errors{0}else{facts.items.len()},"budgets":{"max_source_bytes":MAX_SOURCE,"max_occurrences":MAX_FACTS,"max_key_bytes":MAX_KEY_BYTES,"max_key_characters":MAX_KEY_CHARS,"AST_parsing_included":false},"runtime_binding_proven":false}));
        if doc.parse_errors || facts.format != 1 {
            continue;
        }
        result.items.extend(facts.items.iter().map(|s| Occurrence {
            key: s.key.clone(),
            key_encoding: s.key_encoding.clone(),
            usage_kind: s.usage_kind.clone(),
            syntax: s.syntax.clone(),
            document_id: id.clone(),
            path: doc.path.clone(),
            project: doc.project.clone(),
            revision: doc.revision,
            content_hash: doc.hash.clone(),
            start_byte: s.start_byte,
            end_byte: s.end_byte,
            line: s.line,
            column: s.column,
            owner: s.owner.clone(),
        }));
    }
    result.items.sort_by(|a, b| {
        (&a.path, &a.document_id, a.start_byte, &a.key, &a.usage_kind).cmp(&(
            &b.path,
            &b.document_id,
            b.start_byte,
            &b.key,
            &b.usage_kind,
        ))
    });
    result
        .coverage
        .sort_by(|a, b| a["document_id"].as_str().cmp(&b["document_id"].as_str()));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn placeholder_occurrences_preserve_repeats_unicode_offsets_and_exact_key_case() {
        let src = "Привет %{reg.months} %{reg.months}\n@{Other-Key} {{reg.months}} {{not a key}}";
        let doc = parse("a", "/p/view.html", src, 7).unwrap();
        let facts = doc.translation_keys.unwrap();
        assert!(facts.complete);
        assert_eq!(facts.items.len(), 4);
        for fact in &facts.items {
            assert!(src.is_char_boundary(fact.start_byte));
            assert!(src[fact.start_byte..fact.end_byte].contains(&fact.key));
        }
        assert_eq!(facts.items[0].column, "Привет ".len() + 1);
        assert_eq!(facts.items[2].line, 2);
        assert_eq!(facts.items[2].key, "Other-Key");
    }
    #[test]
    fn php_literal_calls_keep_owners_and_exclude_dynamic_interpolated_and_composed_keys() {
        let src = r#"<?php
function render() {
 Translate('reg.months'); \App\Tr("reg.months"); gettext('Mixed Key');
 Translate($key); Translate("$key"); Translate('reg.' . 'months');
 Translate(key: 'named'); Translate('escaped\\key'); other('not_translation');
 $obj->Translate('not_function'); Translate(...$args);
}
"#;
        let doc = parse("php", "/p/view.php", src, 3).unwrap();
        assert!(!doc.parse_errors);
        let facts = doc.translation_keys.unwrap();
        assert_eq!(facts.items.len(), 4, "{facts:?}");
        assert!(facts.items.iter().all(|s| s.owner.is_some()));
        assert_eq!(facts.excluded_dynamic_or_unsupported_calls, 5);
        for fact in facts.items.iter().filter(|f| f.key != "escaped\\key") {
            assert_eq!(&src[fact.start_byte..fact.end_byte], fact.key);
        }
        assert!(facts
            .items
            .iter()
            .any(|f| f.key == "escaped\\key" && f.key_encoding == "php_single_quoted"));
    }
    #[test]
    fn php_quoted_escapes_unicode_and_literal_dollars_are_decoded_not_guessed() {
        let src = r#"<?php t('a\'b'); t('a\\b'); t('$literal'); t("\$literal"); t("reg\x2Emonths"); t("\u{41}"); t("\101"); t("\400"); t("\xFF"); t("$variable"); t("$5");"#;
        let doc = parse("a", "/p/escapes.php", src, 1).unwrap();
        assert!(!doc.parse_errors);
        let f = doc.translation_keys.unwrap();
        assert_eq!(
            f.items.iter().map(|i| i.key.as_str()).collect::<Vec<_>>(),
            vec![
                "a'b",
                r"a\b",
                "$literal",
                "$literal",
                "reg.months",
                "A",
                "A",
                "$5"
            ]
        );
        assert_eq!(f.excluded_dynamic_or_unsupported_calls, 3);
    }
    #[test]
    fn reference_unicode_key_character_limit_does_not_become_a_shorter_byte_limit() {
        for unit in ["Я", "界", "🦀"] {
            let key = unit.repeat(120);
            assert!(validate_key(&key).is_ok());
            assert!(validate_key(&unit.repeat(121)).is_err());
            let source = format!("<?php Translate('{key}');");
            let doc = parse("a", "/p/unicode.php", &source, 1).unwrap();
            let facts = doc.translation_keys.unwrap();
            assert_eq!(facts.items.len(), 1);
            assert_eq!(facts.items[0].key, key);
        }
    }
    #[test]
    fn capture_limits_are_explicit_without_false_completeness() {
        let doc = parse("a", "/p/view.html", &"%{key}".repeat(MAX_FACTS + 2), 1).unwrap();
        let f = doc.translation_keys.unwrap();
        assert_eq!(f.items.len(), MAX_FACTS);
        assert!(!f.complete);
        assert_eq!(f.limit_reason.as_deref(), Some("occurrence_limit"));
        let doc = parse("b", "/p/view.html", &"a".repeat(MAX_SOURCE + 1), 1).unwrap();
        let f = doc.translation_keys.unwrap();
        assert!(f.items.is_empty());
        assert_eq!(f.limit_reason.as_deref(), Some("source_byte_limit"));
        assert!(parse("c", "/p/code.js", "const x='{{key}}';", 1)
            .unwrap()
            .translation_keys
            .is_none());
    }
}

#[cfg(test)]
mod query_tests {
    use super::*;
    use crate::models::Document;
    fn put(store: &Store, id: &str, path: &str, project: &str, content: &str) {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("doc://{id}"),
                source_file: Some(path.into()),
                wing: Some(project.into()),
                content: content.into(),
                ..Default::default()
            })
            .unwrap();
    }
    #[test]
    fn key_query_links_template_and_php_candidates_with_project_freshness_and_count_guards() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("keys.duckdb")).unwrap();
        put(
            &store,
            "php",
            "/p/render.php",
            "p",
            "<?php function render(){ Translate('reg.months'); Translate($key); }",
        );
        put(
            &store,
            "view",
            "/p/view.html",
            "p",
            "Привет %{reg.months} %{reg.months} @{reg.months} {{other.key}}",
        );
        put(&store, "foreign", "/q/view.html", "q", "%{reg.months}");
        store.index_code().unwrap();
        let q = ReferenceQuery {
            key: Some("reg.months".into()),
            project: Some("p".into()),
            ..Default::default()
        };
        let r = store.code_references(&q).unwrap();
        assert_eq!(r["total"], 4, "{r}");
        assert_eq!(r["counts_by_kind"]["uses_key"], 4);
        assert!(r["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["from"]["kind"] == "symbol" && i["from"]["name"] == "render"));
        let limited = store
            .code_references(&ReferenceQuery {
                selection: CodeQuery {
                    limit: Some(1),
                    ..Default::default()
                },
                ..ReferenceQuery {
                    key: Some("reg.months".into()),
                    project: Some("p".into()),
                    ..Default::default()
                }
            })
            .unwrap();
        assert_eq!(limited["total"], 4);
        assert_eq!(limited["items"].as_array().unwrap().len(), 1);
        assert_eq!(limited["truncated"], true);
        let ambiguous = store
            .code_references(&ReferenceQuery {
                key: Some("reg.months".into()),
                selection: CodeQuery {
                    path: Some("/p/view.html".into()),
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        assert_eq!(ambiguous["needs_disambiguation"], true);
        put(
            &store,
            "php",
            "/p/render.php",
            "p",
            "<?php function render(){ Translate('other.key'); }",
        );
        let stale = store.code_references(&q).unwrap();
        assert_eq!(stale["total"], 3);
        assert_eq!(stale["stale_documents"].as_array().unwrap().len(), 1);
        store.index_code().unwrap();
        let fresh = store.code_references(&q).unwrap();
        assert_eq!(fresh["total"], 3);
        assert!(fresh["stale_documents"].as_array().unwrap().is_empty());
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::models::Document;
    #[test]
    fn missing_per_document_capability_is_stale_even_with_current_global_format() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("cache.duckdb")).unwrap();
        store
            .upsert_document(&Document {
                id: "a".into(),
                uri: "doc://a".into(),
                source_file: Some("/p/view.html".into()),
                content: "%{key}".into(),
                wing: Some("p".into()),
                ..Default::default()
            })
            .unwrap();
        store.index_code().unwrap();
        let payload: String = store
            .lock()
            .unwrap()
            .query_row(
                "SELECT payload FROM code_index_documents WHERE document_id='a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut json: serde_json::Value = serde_json::from_str(&payload).unwrap();
        json.as_object_mut().unwrap().remove("translation_keys");
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE code_index_documents SET payload=? WHERE document_id='a'",
                [json.to_string()],
            )
            .unwrap();
        let query = ReferenceQuery {
            key: Some("key".into()),
            project: Some("p".into()),
            ..Default::default()
        };
        let stale = store.code_references(&query).unwrap();
        assert_eq!(stale["total"], 0);
        assert_eq!(stale["stale_documents"].as_array().unwrap().len(), 1);
        store.index_code().unwrap();
        assert_eq!(store.code_references(&query).unwrap()["total"], 1);
    }
}
