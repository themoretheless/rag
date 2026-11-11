//! Bounded stated PHPDoc parameter shapes; no PHPStan/Psalm/compiler execution.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
mod parser;
const MAX_DOC: usize = 64 * 1024;
const MAX_TAGS: usize = 64;
const MAX_TYPE: usize = 4096;
const MAX_DECLARED: usize = 256;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShapeField {
    pub key: Option<String>,
    pub key_expression: String,
    pub key_kind: String,
    pub required: bool,
    pub type_expression: String,
    pub nested_shape: Option<Box<ShapeContract>>,
    pub source: DocumentationSpan,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShapeContract {
    pub kind: String,
    pub sealed: bool,
    pub extra_types: Option<String>,
    pub fields: Vec<ShapeField>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterContract {
    pub name: Option<String>,
    pub tag: String,
    pub type_expression: Option<String>,
    pub type_hash: Option<String>,
    pub type_truncated: bool,
    pub by_reference: bool,
    pub variadic: bool,
    pub declaration_match: String,
    pub status: String,
    pub diagnostic: Option<String>,
    pub shape: Option<ShapeContract>,
    pub source: DocumentationSpan,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterContracts {
    pub format: u32,
    pub status: String,
    pub complete: bool,
    pub documentation_hash: Option<String>,
    pub parameters: Vec<ParameterContract>,
    pub tags_total: Option<usize>,
    pub tags_truncated: bool,
    pub multiple_tag_names: Vec<String>,
    pub undocumented_parameters: Vec<String>,
    pub declaration_list_complete: bool,
    pub inherited_contract_unverified: bool,
    pub dialect_precedence_applied: bool,
    pub runtime_types_verified: bool,
    pub requiredness_scope: String,
    pub budgets: serde_json::Value,
}
fn php(path: &str) -> bool {
    matches!(path.rsplit('.').next(), Some("php" | "phtml"))
}
fn callable(kind: &str) -> bool {
    matches!(kind, "function_definition" | "method_declaration")
}
pub(super) fn current(doc: &IndexedDocument) -> bool {
    !php(&doc.path)
        || doc.symbols.iter().filter(|s| callable(&s.kind)).all(|s| {
            s.parameter_contracts
                .as_ref()
                .is_some_and(|c| c.format == 1)
        })
}
struct Mapped<'a> {
    text: String,
    map: Vec<usize>,
    raw: &'a str,
    base: usize,
    line: usize,
}
impl Mapped<'_> {
    fn span(&self, start: usize, end: usize) -> DocumentationSpan {
        let a = self.map.get(start).copied().unwrap_or(0);
        let b = if end > start {
            self.map.get(end - 1).copied().map(|v| v + 1).unwrap_or(a)
        } else {
            a
        };
        DocumentationSpan {
            start_byte: self.base + a,
            end_byte: self.base + b,
            start_line: self.line
                + self.raw.as_bytes()[..a]
                    .iter()
                    .filter(|b| **b == b'\n')
                    .count(),
            end_line: self.line
                + self.raw.as_bytes()[..b]
                    .iter()
                    .filter(|b| **b == b'\n')
                    .count(),
        }
    }
}
fn mapped<'a>(node: Node, source: &'a str) -> Mapped<'a> {
    let raw = text(node, source);
    let mut result = Mapped {
        text: String::new(),
        map: Vec::new(),
        raw,
        base: node.start_byte(),
        line: node.start_position().row + 1,
    };
    let mut offset = 0;
    for line in raw.split_inclusive('\n') {
        let mut start = 0;
        let mut end = line.len();
        if offset == 0 && line.starts_with("/**") {
            start = 3;
        }
        let trimmed = line[..end].trim_end();
        if trimmed.ends_with("*/") {
            end = trimmed.len() - 2;
        }
        let bytes = line.as_bytes();
        while start < end && matches!(bytes[start], b' ' | b'\t' | b'\r') {
            start += 1;
        }
        if start < end && bytes[start] == b'*' {
            start += 1;
            if start < end && bytes[start] == b' ' {
                start += 1;
            }
        }
        start = start.min(end);
        result.text.push_str(&line[start..end]);
        result.map.extend((offset + start)..(offset + end));
        offset += line.len();
    }
    result
}
fn declared(node: Node, source: &str) -> (BTreeSet<String>, bool) {
    let Some(params) = node.child_by_field_name("parameters") else {
        return (BTreeSet::new(), false);
    };
    let mut cursor = params.walk();
    let mut names = BTreeSet::new();
    let mut complete = true;
    for (i, param) in params
        .named_children(&mut cursor)
        .filter(|n| n.kind() != "comment")
        .take(MAX_DECLARED + 1)
        .enumerate()
    {
        if i == MAX_DECLARED {
            complete = false;
            break;
        }
        let Some(name) = param.child_by_field_name("name") else {
            complete = false;
            continue;
        };
        let name = text(name, source)
            .trim_start_matches(['&', ' '])
            .strip_prefix('$');
        if let Some(name) = name.filter(|n| n.len() <= 256) {
            names.insert(name.to_string());
        } else {
            complete = false;
        }
    }
    (names, complete)
}
pub(super) fn attach(
    node: Node,
    nodes: &[Node],
    source: &str,
    out: &mut IndexedDocument,
    index: usize,
    examined: bool,
) {
    if !php(&out.path) || !callable(node.kind()) {
        return;
    }
    let mut report = ParameterContracts {
        format: 1,
        status: "captured".into(),
        complete: true,
        documentation_hash: out.symbols[index]
            .documentation
            .as_ref()
            .map(|d| d.content_hash.clone()),
        parameters: Vec::new(),
        tags_total: Some(0),
        tags_truncated: false,
        multiple_tag_names: Vec::new(),
        undocumented_parameters: Vec::new(),
        declaration_list_complete: false,
        inherited_contract_unverified: false,
        dialect_precedence_applied: false,
        runtime_types_verified: false,
        requiredness_scope: "stated_shape_keys_when_value_is_supplied_not_parameter_presence"
            .into(),
        budgets: serde_json::json!({"max_documentation_bytes":MAX_DOC,"max_tags":MAX_TAGS,"max_type_bytes":MAX_TYPE,"max_declared_parameters":MAX_DECLARED,"max_shape_depth":16,"max_fields_per_shape":128,"max_total_shape_fields_per_docblock":512,"AST_parsing_and_raw_documentation_hashing_included":false}),
    };
    if out.parse_errors || !examined {
        report.status = if out.parse_errors {
            "source_parse_error"
        } else {
            "uninspected_documentation_style"
        }
        .into();
        report.complete = false;
        report.tags_total = None;
        out.symbols[index].parameter_contracts = Some(report);
        return;
    }
    let (declared, list_complete) = declared(node, source);
    report.declaration_list_complete = list_complete;
    if nodes.is_empty() {
        report.status = "no_docblock".into();
        report.undocumented_parameters = declared.into_iter().collect();
        out.symbols[index].parameter_contracts = Some(report);
        return;
    }
    if nodes.len() != 1 || text(nodes[0], source).len() > MAX_DOC {
        report.status = "documentation_byte_or_segment_limit".into();
        report.complete = false;
        report.tags_total = None;
        out.symbols[index].parameter_contracts = Some(report);
        return;
    }
    let mapped = mapped(nodes[0], source);
    report.inherited_contract_unverified = mapped.text.to_ascii_lowercase().contains("@inheritdoc");
    let mut tags = Vec::new();
    let mut position = 0;
    for line in mapped.text.split_inclusive('\n') {
        let leading = line.len() - line.trim_start().len();
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('@') {
            let length = rest
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '-')
                .unwrap_or(rest.len());
            let tag = &rest[..length];
            tags.push((position + leading, position + leading + 1 + length, tag));
        }
        position += line.len();
    }
    let accepted = |tag: &str| matches!(tag, "param" | "psalm-param" | "phpstan-param");
    let total = tags.iter().filter(|(_, _, t)| accepted(t)).count();
    report.tags_total = Some(total);
    report.tags_truncated = total > MAX_TAGS;
    report.complete =
        !report.tags_truncated && list_complete && !report.inherited_contract_unverified;
    let mut field_budget = 512usize;
    let mut counts = BTreeMap::new();
    let mut documented = BTreeSet::new();
    for (i, (start, type_start, tag)) in tags
        .iter()
        .enumerate()
        .filter(|(_, (_, _, t))| accepted(t))
        .take(MAX_TAGS)
    {
        let end = tags.get(i + 1).map_or(mapped.text.len(), |v| v.0);
        let raw = &mapped.text[*type_start..end];
        let mut parsed = parser::parameter(raw, *type_start, &mapped, &mut field_budget);
        parsed.tag = (*tag).into();
        parsed.source = mapped.span(*start, end);
        if let Some(name) = &parsed.name {
            parsed.declaration_match = if declared.contains(name) {
                documented.insert(name.clone());
                "matched"
            } else if list_complete {
                "not_in_captured_parameters"
            } else {
                "unverified_limited_parameter_list"
            }
            .into();
            *counts.entry(name.clone()).or_insert(0usize) += 1;
        }
        if parsed.name.is_some() && parsed.declaration_match != "matched" {
            report.complete = false;
        }
        if matches!(parsed.status.as_str(), "unparsed" | "limited") {
            report.complete = false;
        }
        report.parameters.push(parsed);
    }
    report.multiple_tag_names = counts
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(n, _)| n)
        .collect();
    if !report.multiple_tag_names.is_empty() {
        report.complete = false;
    }
    report.undocumented_parameters = declared.difference(&documented).cloned().collect();
    if total == 0 {
        report.status = "no_parameter_tags".into();
    } else if !report.complete {
        report.status = "partial_or_ambiguous".into();
    }
    out.symbols[index].parameter_contracts = Some(report);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn contracts(doc: &str, params: &str) -> (String, ParameterContracts) {
        let source = format!("<?php\n{doc}\nfunction target({params}) {{}}\n");
        let parsed = parse("fixture", "fixture.php", &source, 1).unwrap();
        assert!(!parsed.parse_errors);
        let contract = parsed
            .symbols
            .iter()
            .find(|s| s.name == "target")
            .unwrap()
            .parameter_contracts
            .clone()
            .unwrap();
        (source, contract)
    }
    #[test]
    fn php_contract_nested_quoted_generic_and_source() {
        let (source, c) = contracts("/**\n * @param array{foo:int, opt?:array{'a,b':list<string>, '*':callable(int, string):bool}, ...<string,mixed>} $data don't parse { description\n */", "$data");
        assert!(c.complete, "{c:?}");
        let p = &c.parameters[0];
        assert_eq!(p.declaration_match, "matched");
        let s = p.shape.as_ref().unwrap();
        assert!(!s.sealed);
        assert!(s.fields[0].required);
        assert!(!s.fields[1].required);
        let nested = s.fields[1].nested_shape.as_ref().unwrap();
        assert_eq!(nested.fields[0].key.as_deref(), Some("a,b"));
        assert_eq!(nested.fields[1].key.as_deref(), Some("*"));
        let span = &nested.fields[1].source;
        assert!(source[span.start_byte..span.end_byte].contains("'*':callable"));
        assert_eq!(span.start_line, 3);
    }
    #[test]
    fn php_contract_binding_and_dialect_evidence() {
        let (_, c) = contracts("/**\n * @param array{x:int} $missing\n * @param array{x:int} $data\n * @phpstan-param array{x?:int} $data\n */", "$data, $undocumented");
        assert!(!c.complete);
        assert_eq!(
            c.parameters[0].declaration_match,
            "not_in_captured_parameters"
        );
        assert_eq!(c.multiple_tag_names, vec!["data"]);
        assert_eq!(c.undocumented_parameters, vec!["undocumented"]);
        assert!(!c.dialect_precedence_applied);
    }
    #[test]
    fn php_contract_invalid_does_not_publish_partial_shape() {
        for typ in [
            "array{foo:int, 'foo':string}",
            "array{a:int,,b:string}",
            "array{a:int",
        ] {
            let (_, c) = contracts(&format!("/** @param {typ} $data */"), "$data");
            assert!(!c.complete, "{typ}: {c:?}");
            assert!(c.parameters[0].shape.is_none());
        }
        let (_, c) = contracts("/** @param array{a:int}|null $data */", "$data");
        assert!(c.parameters[0].shape.is_none());
        assert_eq!(c.parameters[0].status, "type_only");
    }
    #[test]
    fn php_contract_full_doc_independent_of_excerpt() {
        let (_, c) = contracts(
            &format!(
                "/**\n * {}\n * @param array{{α?:string}} $data\n */",
                "intro ".repeat(900)
            ),
            "$data",
        );
        assert!(c.complete);
        assert_eq!(
            c.parameters[0].shape.as_ref().unwrap().fields[0]
                .key
                .as_deref(),
            Some("α")
        );
        let (_, large) = contracts(&format!("/** {} */", "a".repeat(MAX_DOC)), "$data");
        assert!(!large.complete);
        assert_eq!(large.tags_total, None);
    }
    #[test]
    fn php_contract_tag_and_type_budgets() {
        let (_, c) = contracts(
            &format!("/**\n{}*/", " * @param array{x:int} $data\n".repeat(65)),
            "$data",
        );
        assert!(c.tags_truncated);
        assert_eq!(c.tags_total, Some(65));
        assert_eq!(c.parameters.len(), 64);
        let (_, c) = contracts(
            &format!("/** @param array{{x:'{}'}} $data */", "a".repeat(MAX_TYPE)),
            "$data",
        );
        assert!(c.parameters[0].type_truncated);
        assert!(c.parameters[0].type_expression.is_none());
    }
    #[test]
    fn php_contract_tuples_rest_and_constants() {
        let (_, c) = contracts(
            "/** @param list{string, int, ...<mixed>} &$data */",
            "&$data",
        );
        let p = &c.parameters[0];
        assert!(p.by_reference);
        let s = p.shape.as_ref().unwrap();
        assert_eq!(s.kind, "list");
        assert_eq!(s.fields[1].key.as_deref(), Some("1"));
        assert!(!s.sealed);
        let (_, c) = contracts("/** @param object{Foo::KEY: string} $data */", "$data");
        let f = &c.parameters[0].shape.as_ref().unwrap().fields[0];
        assert!(f.key.is_none());
        assert_eq!(f.key_kind, "constant_expression_unresolved");
    }
    #[test]
    fn php_contract_adjacent_only_and_cache_guard() {
        let source="<?php /** @param array{x:int} $data */ function previous($data) {} function bare($data) {}";
        let mut parsed = parse("fixture", "fixture.php", source, 1).unwrap();
        let bare = parsed.symbols.iter().find(|s| s.name == "bare").unwrap();
        assert_eq!(
            bare.parameter_contracts.as_ref().unwrap().status,
            "no_docblock"
        );
        assert!(current(&parsed));
        parsed
            .symbols
            .iter_mut()
            .find(|s| s.name == "bare")
            .unwrap()
            .parameter_contracts = None;
        assert!(!current(&parsed));
        let (_, c) = contracts("/**/", "$data");
        assert!(c.parameters.is_empty());
    }
    #[test]
    fn php_contract_shape_limits_are_explicit() {
        let fields = (0..129)
            .map(|n| format!("k{n}:int"))
            .collect::<Vec<_>>()
            .join(",");
        let (_, c) = contracts(&format!("/** @param array{{{fields}}} $data */"), "$data");
        assert_eq!(c.parameters[0].status, "limited");
        assert!(c.parameters[0].shape.is_none());
        let mut typ = "int".to_string();
        for _ in 0..19 {
            typ = format!("array{{x:{typ}}}");
        }
        let (_, c) = contracts(&format!("/** @param {typ} $data */"), "$data");
        assert_eq!(c.parameters[0].status, "limited");
        assert!(c.parameters[0].shape.is_none());
    }
    #[test]
    fn php_contract_methods_promotion_and_parse_errors() {
        let source="<?php class Example { /** @param array{x:int} $data */ public function __construct(public array $data) {} /** @param array{x?:int} ...$args */ public function run(...$args) {} }";
        let parsed = parse("fixture", "fixture.php", source, 1).unwrap();
        assert!(!parsed.parse_errors);
        let callables = parsed
            .symbols
            .iter()
            .filter_map(|s| s.parameter_contracts.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(callables.len(), 2);
        assert!(callables
            .iter()
            .all(|c| c.complete && c.parameters[0].declaration_match == "matched"));
        assert!(callables.iter().any(|c| c.parameters[0].variadic));
        let broken = parse(
            "fixture",
            "fixture.php",
            &format!("{source} function broken( {{"),
            1,
        )
        .unwrap();
        assert!(broken.parse_errors);
        assert!(broken
            .symbols
            .iter()
            .filter_map(|s| s.parameter_contracts.as_ref())
            .all(|c| !c.complete && c.tags_total.is_none()));
    }
}
