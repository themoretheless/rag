//! Explicit captured build selection. Raw AST caches are never destructively
//! configured: both current snapshots and frozen history apply the same view.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Input {
    pub(super) target_kind: String,
    pub(super) target_name: Option<String>,
    features: BTreeSet<String>,
    default_features: bool,
    flags: BTreeSet<String>,
    values: BTreeMap<String, BTreeSet<String>>,
    target_triple: Option<String>,
}
impl Input {
    pub(super) fn enable_tests(&mut self) {
        self.flags.insert("test".into());
    }
    pub(super) fn command_options(&self) -> Vec<String> {
        let mut argv = Vec::new();
        if !self.default_features {
            argv.push("--no-default-features".into());
        }
        if !self.features.is_empty() {
            argv.extend([
                "--features".into(),
                self.features.iter().cloned().collect::<Vec<_>>().join(","),
            ]);
        }
        if let Some(target) = &self.target_triple {
            argv.extend(["--target".into(), target.clone()]);
        }
        argv
    }
}

/// A test-query overlay, never persisted to raw extraction caches. Unasserted
/// features/platform cfg remain unknown; only test/not(test) is decided here.
pub(super) fn prepare_test_view(docs: &mut HashMap<String, IndexedDocument>) {
    fn test_value(p: &Predicate) -> Option<bool> {
        match p {
            Predicate::Flag(flag) if flag == "test" => Some(true),
            Predicate::Not(p) => test_value(p).map(|v| !v),
            Predicate::All(ps) => {
                let values: Vec<_> = ps.iter().map(test_value).collect();
                if values.contains(&Some(false)) {
                    Some(false)
                } else if values.contains(&None) {
                    None
                } else {
                    Some(true)
                }
            }
            Predicate::Any(ps) => {
                let values: Vec<_> = ps.iter().map(test_value).collect();
                if values.contains(&Some(true)) {
                    Some(true)
                } else if values.contains(&None) {
                    None
                } else {
                    Some(false)
                }
            }
            _ => None,
        }
    }
    for doc in docs.values_mut() {
        rust_cargo::enable_test_flag(doc);
        if !doc.path.ends_with(".rs") {
            continue;
        }
        let conditions = doc.rust_cfg.conditions.clone();
        let decision = |at: usize| {
            let mut unknown = false;
            for condition in conditions.iter().filter(|c| c.start <= at && at < c.end) {
                match condition.predicate.as_ref().and_then(test_value) {
                    Some(false) => return Some(false),
                    None => unknown = true,
                    _ => (),
                }
            }
            if unknown {
                None
            } else {
                Some(true)
            }
        };
        doc.symbols
            .retain(|s| decision(s.start_byte) != Some(false));
        doc.references
            .retain(|r| decision(r.start_byte) != Some(false));
        doc.test_definitions
            .retain(|at| decision(*at) != Some(false));
        doc.bases.retain(|b| decision(b.start) != Some(false));
        rust_cargo::filter_modules(doc, decision);
        rust_cargo::configure_reexports(doc, decision);
        // This is only the test/not(test) overlay. Keep feature/platform import
        // facts intact until apply evaluates the captured build configuration.
        resolve::configure_rust_bindings(doc, |at| Some(decision(at) != Some(false)));
        rust_semantics::configure(doc, &|at| Some(decision(at) != Some(false)));
    }
}
pub(super) fn parse_input(item: &toml_edit::Item) -> std::result::Result<Input, String> {
    let t = item.as_table_like().ok_or("configuration_not_table")?;
    let allowed = [
        "version",
        "target_kind",
        "target_name",
        "features",
        "default_features",
        "cfg_flags",
        "cfg_values",
        "target_triple",
    ];
    if t.iter().any(|(k, _)| !allowed.contains(&k)) {
        return Err("unknown_configuration_key".into());
    }
    if t.get("version").and_then(toml_edit::Item::as_integer) != Some(1) {
        return Err("unsupported_configuration_version".into());
    }
    let target_kind = t
        .get("target_kind")
        .and_then(toml_edit::Item::as_str)
        .ok_or("target_kind_required")?
        .to_string();
    if !["lib", "bin", "test", "bench", "example"].contains(&target_kind.as_str()) {
        return Err("invalid_target_kind".into());
    }
    let list = |item: Option<&toml_edit::Item>,
                allow_empty: bool|
     -> std::result::Result<BTreeSet<String>, String> {
        let Some(item) = item else {
            return Ok(BTreeSet::new());
        };
        let a = item.as_array().ok_or("expected_string_array")?;
        a.iter()
            .map(|v| {
                v.as_str()
                    .filter(|s| s.len() <= 256 && (allow_empty || !s.is_empty()))
                    .map(str::to_string)
                    .ok_or("invalid_array_value".into())
            })
            .collect()
    };
    let string = |key: &str| -> std::result::Result<Option<String>, String> {
        t.get(key)
            .map(|v| {
                v.as_str()
                    .map(str::to_string)
                    .ok_or("expected_string".into())
            })
            .transpose()
    };
    let default_features = t
        .get("default_features")
        .map(|v| v.as_bool().ok_or("expected_boolean"))
        .transpose()?
        .unwrap_or(true);
    let mut values = BTreeMap::new();
    if let Some(v) = t.get("cfg_values") {
        for (key, value) in v.as_table_like().ok_or("cfg_values_not_table")?.iter() {
            values.insert(key.into(), list(Some(value), true)?);
        }
    }
    if values.contains_key("feature") {
        return Err("feature_cfg_must_use_features_field".into());
    }
    let input = Input {
        target_kind,
        target_name: string("target_name")?,
        features: list(t.get("features"), false)?,
        default_features,
        flags: list(t.get("cfg_flags"), false)?,
        values,
        target_triple: string("target_triple")?,
    };
    if input.features.len() > 128
        || input.flags.len() > 128
        || input.values.len() > 32
        || input.values.values().any(|v| v.len() > 128)
        || input.target_name.as_ref().is_some_and(|v| v.len() > 256)
        || input.target_triple.as_ref().is_some_and(|v| v.len() > 256)
    {
        return Err("configuration_size_limit".into());
    }
    Ok(input)
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Facts {
    conditions: Vec<Condition>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Condition {
    start: usize,
    end: usize,
    kind: String,
    predicate: Option<Predicate>,
    #[serde(default)]
    start_line: usize,
    #[serde(default)]
    end_line: usize,
}
fn projectable_condition(kind: &str) -> bool {
    matches!(
        kind,
        "function_item"
            | "function_signature_item"
            | "mod_item"
            | "impl_item"
            | "struct_item"
            | "enum_item"
            | "trait_item"
            | "type_item"
            | "use_declaration"
            | "public_use_declaration"
            | "source_file"
            | "argument_expression"
            | "block"
            | "expression_statement"
            | "field_declaration"
            | "field_initializer"
            | "shorthand_field_initializer"
            | "extern_crate_declaration"
    )
}
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Predicate {
    OpaqueAttribute,
    Flag(String),
    Value(String, String),
    All(Vec<Predicate>),
    Any(Vec<Predicate>),
    Not(Box<Predicate>),
}
#[derive(Clone, Debug, PartialEq)]
enum Token {
    Name(String),
    String(String),
    Open,
    Close,
    Comma,
    Equals,
}
fn predicate(raw: &str) -> Option<Predicate> {
    let mut tokens = vec![];
    let mut i = 0;
    let bytes = raw.as_bytes();
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let punctuation = match bytes[i] {
            b'(' => Some(Token::Open),
            b')' => Some(Token::Close),
            b',' => Some(Token::Comma),
            b'=' => Some(Token::Equals),
            _ => None,
        };
        if let Some(token) = punctuation {
            tokens.push(token);
            i += 1;
            continue;
        }
        if bytes[i] == b'"' {
            let start = i;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            tokens.push(Token::String(
                serde_json::from_str(raw.get(start..i)?).ok()?,
            ));
            continue;
        }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
            i += 1;
        }
        if i == start {
            return None;
        }
        tokens.push(Token::Name(raw[start..i].into()));
        if tokens.len() > 512 {
            return None;
        }
    }
    fn parse(tokens: &[Token], at: &mut usize, depth: usize) -> Option<Predicate> {
        if depth > 32 {
            return None;
        }
        let Token::Name(name) = tokens.get(*at)?.clone() else {
            return None;
        };
        *at += 1;
        if tokens.get(*at) == Some(&Token::Equals) {
            *at += 1;
            let Token::String(value) = tokens.get(*at)?.clone() else {
                return None;
            };
            *at += 1;
            return Some(Predicate::Value(name, value));
        }
        if tokens.get(*at) != Some(&Token::Open) {
            return Some(Predicate::Flag(name));
        }
        *at += 1;
        let mut args = vec![];
        while tokens.get(*at) != Some(&Token::Close) {
            args.push(parse(tokens, at, depth + 1)?);
            if tokens.get(*at) == Some(&Token::Comma) {
                *at += 1;
            } else if tokens.get(*at) != Some(&Token::Close) {
                return None;
            }
        }
        *at += 1;
        match name.as_str() {
            "all" => Some(Predicate::All(args)),
            "any" => Some(Predicate::Any(args)),
            "not" if args.len() == 1 => Some(Predicate::Not(Box::new(args.pop()?))),
            _ => None,
        }
    }
    let mut at = 0;
    let result = parse(&tokens, &mut at, 0)?;
    (at == tokens.len()).then_some(result)
}
fn meta_arguments(raw: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut stack = Vec::new();
    let mut quoted = false;
    let mut escape = false;
    for (i, c) in raw.char_indices() {
        if quoted {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                quoted = false;
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '(' | '[' | '{' => stack.push(c),
            ')' | ']' | '}' => {
                let open = stack.pop()?;
                if !matches!((open, c), ('(', ')') | ('[', ']') | ('{', '}')) {
                    return None;
                }
            }
            ',' if stack.is_empty() => {
                parts.push(raw[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    if quoted || !stack.is_empty() {
        return None;
    }
    let last = raw[start..].trim();
    if !last.is_empty() {
        parts.push(last);
    }
    Some(parts)
}
fn attribute_predicate(raw: &str, depth: usize) -> Option<Predicate> {
    if depth > 16 || raw.len() > 8192 {
        return None;
    }
    let raw = raw.trim();
    let head = raw.split(['(', '=']).next()?.trim();
    if head == "cfg" || head == "cfg_attr" {
        let body = raw
            .strip_prefix(head)?
            .trim()
            .strip_prefix('(')?
            .strip_suffix(')')?;
        if head == "cfg" {
            return predicate(body);
        }
        let parts = meta_arguments(body)?;
        if parts.len() < 2 || parts.len() > 33 {
            return None;
        }
        let condition = predicate(parts[0])?;
        let requirements = parts[1..]
            .iter()
            .map(|p| attribute_predicate(p, depth + 1))
            .collect::<Option<Vec<_>>>()?;
        return Some(Predicate::Any(vec![
            Predicate::Not(Box::new(condition)),
            Predicate::All(requirements),
        ]));
    }
    Some(
        if matches!(
            head,
            "allow"
                | "warn"
                | "deny"
                | "forbid"
                | "expect"
                | "inline"
                | "cold"
                | "must_use"
                | "deprecated"
                | "doc"
                | "rustfmt::skip"
                | "automatically_derived"
        ) {
            Predicate::All(vec![])
        } else {
            Predicate::OpaqueAttribute
        },
    )
}
pub(super) fn collect(root: Node, source: &str, doc: &mut IndexedDocument) {
    fn condition_kind(node: Node) -> String {
        if node.kind() == "expression_statement"
            && node.parent().is_some_and(|p| {
                p.kind() == "source_file"
                    || (p.kind() == "declaration_list"
                        && p.parent().is_some_and(|p| p.kind() == "mod_item"))
            })
            && node
                .named_child(0)
                .is_some_and(|n| n.kind() == "macro_invocation")
        {
            return "macro_invocation".into();
        }
        // Callee typing does not infer from argument values or argument arity.
        // Each argument's references, bindings, fields and mutations are filtered
        // separately, so an excluded argument cannot establish a receiver type.
        if node.parent().is_some_and(|p| p.kind() == "arguments") {
            "argument_expression".into()
        } else {
            node.kind().into()
        }
    }
    let mut stack = vec![(root, 0)];
    while let Some((node, depth)) = stack.pop() {
        if depth > 128 {
            continue;
        }
        let mut cursor = node.walk();
        let mut pending = vec![];
        for child in node.named_children(&mut cursor) {
            if matches!(child.kind(), "attribute_item" | "inner_attribute_item") {
                let raw = text(child, source)
                    .trim()
                    .trim_start_matches("#![")
                    .trim_start_matches("#[")
                    .trim_end_matches(']')
                    .trim();
                let head = raw.split(['(', '=']).next().unwrap_or("").trim();
                if matches!(head, "cfg" | "cfg_attr") {
                    let entry = (child.start_byte(), attribute_predicate(raw, 0));
                    if child.kind() == "inner_attribute_item"
                        || matches!(
                            node.kind(),
                            "field_initializer" | "shorthand_field_initializer"
                        )
                    {
                        doc.rust_cfg.conditions.push(Condition {
                            start_line: node.start_position().row + 1,
                            end_line: node.end_position().row + 1,
                            start: node.start_byte(),
                            end: node.end_byte(),
                            kind: condition_kind(node),
                            predicate: entry.1,
                        });
                    } else {
                        pending.push(entry);
                    }
                }
                continue;
            }
            if child.kind().ends_with("comment") {
                continue;
            }
            for (start, predicate) in pending.drain(..) {
                doc.rust_cfg.conditions.push(Condition {
                    start_line: source[..start].bytes().filter(|b| *b == b'\n').count() + 1,
                    end_line: child.end_position().row + 1,
                    start,
                    end: child.end_byte(),
                    kind: if child.kind() == "use_declaration" && {
                        let mut cursor = child.walk();
                        let visible = child
                            .named_children(&mut cursor)
                            .any(|n| n.kind() == "visibility_modifier");
                        visible
                    } {
                        "public_use_declaration".into()
                    } else {
                        condition_kind(child)
                    },
                    predicate,
                });
            }
            stack.push((child, depth + 1));
        }
    }
}
fn evaluate(p: &Predicate, input: &Input, features: &BTreeSet<String>) -> Option<bool> {
    match p {
        Predicate::OpaqueAttribute => None,
        Predicate::Flag(name) => Some(input.flags.contains(name)),
        Predicate::Value(key, value) if key == "feature" => Some(features.contains(value)),
        Predicate::Value(key, value) => input.values.get(key).map(|values| values.contains(value)),
        Predicate::Not(p) => evaluate(p, input, features).map(|v| !v),
        Predicate::All(ps) => {
            let values: Vec<_> = ps.iter().map(|p| evaluate(p, input, features)).collect();
            if values.contains(&Some(false)) {
                Some(false)
            } else if values.contains(&None) {
                None
            } else {
                Some(true)
            }
        }
        Predicate::Any(ps) => {
            let values: Vec<_> = ps.iter().map(|p| evaluate(p, input, features)).collect();
            if values.contains(&Some(true)) {
                Some(true)
            } else if values.contains(&None) {
                None
            } else {
                Some(false)
            }
        }
    }
}
fn selection(
    man: &IndexedDocument,
    input: &Input,
) -> std::result::Result<(BTreeSet<String>, BTreeSet<String>, bool), String> {
    let definitions = rust_cargo::feature_definitions(man);
    let optional = rust_cargo::optional_names(man);
    let mut queue: Vec<_> = input.features.iter().cloned().collect();
    if input.default_features && definitions.contains_key("default") {
        queue.push("default".into());
    }
    let mut features = BTreeSet::new();
    let mut dependencies = BTreeSet::new();
    let mut forwarded = false;
    while let Some(feature) = queue.pop() {
        if features.len() + dependencies.len() > 4096 {
            return Err("feature_expansion_limit".into());
        }
        if let Some(dep) = feature.strip_prefix("dep:") {
            if !optional.contains(dep) {
                return Err("unknown_optional_dependency".into());
            }
            dependencies.insert(dep.into());
            continue;
        }
        if feature.contains('/') {
            forwarded = true;
            continue;
        }
        if !features.insert(feature.clone()) {
            continue;
        }
        if let Some(values) = definitions.get(&feature) {
            queue.extend(values.iter().cloned());
        } else if optional.contains(&feature) {
            dependencies.insert(feature);
        } else {
            return Err("unknown_feature".into());
        }
    }
    Ok((features, dependencies, forwarded))
}
pub(super) struct Applied {
    pub(super) contexts: Vec<serde_json::Value>,
    pub(super) diagnostics: Vec<serde_json::Value>,
}
fn block(doc: &mut IndexedDocument, reason: &str) {
    doc.binding_limitations.push(reason.into());
    doc.policies.clear();
    doc.sql = sql::SqlDocument::default();
    doc.routes.clear();
    doc.endpoints.clear();
    doc.bases.clear();
}
pub(super) fn apply(docs: &mut HashMap<String, IndexedDocument>) -> Applied {
    let mut result = Applied {
        contexts: vec![],
        diagnostics: vec![],
    };
    let boundaries: Vec<_> = docs
        .iter()
        .filter(|(_, d)| rust_cargo::manifest(&d.path))
        .collect();
    let mut owners = HashMap::new();
    let mut blocked = Vec::new();
    for (id, doc) in docs.iter().filter(|(_, d)| d.path.ends_with(".rs")) {
        let matching: Vec<_> = boundaries
            .iter()
            .filter(|(_, m)| {
                m.project == doc.project
                    && std::path::Path::new(&doc.path).starts_with(
                        std::path::Path::new(&m.path)
                            .parent()
                            .unwrap_or(std::path::Path::new("")),
                    )
            })
            .collect();
        let longest = matching.iter().map(|(_, d)| d.path.len()).max();
        let nearest: Vec<_> = matching
            .into_iter()
            .filter(|(_, d)| Some(d.path.len()) == longest)
            .collect();
        let owner = if nearest.len() == 1 {
            Some(nearest[0].0.clone())
        } else {
            None
        };
        if nearest.len() > 1 {
            blocked.push((id.clone(), "RUST_BUILD_MANIFEST_AMBIGUOUS"));
        } else if nearest
            .first()
            .is_some_and(|(_, d)| !rust_cargo::boundary_verified(d))
        {
            blocked.push((id.clone(), "RUST_BUILD_MANIFEST_UNVERIFIED"));
        }
        owners.insert(id.clone(), owner);
    }
    for (id, reason) in blocked {
        block(docs.get_mut(&id).unwrap(), reason);
        result
            .diagnostics
            .push(serde_json::json!({"document_id":id,"code":reason}));
    }
    if !docs.values().any(|d| rust_cargo::build_input(d).is_some()) {
        return result;
    }
    let original = docs.clone();
    let manifests: Vec<_> = original
        .iter()
        .filter(|(_, d)| rust_cargo::build_input(d).is_some())
        .map(|(id, _)| id.clone())
        .collect();
    for manifest_id in manifests {
        let manifest = &original[&manifest_id];
        let sources: Vec<_> = original
            .iter()
            .filter(|(id, _)| owners.get(*id).and_then(Option::as_ref) == Some(&manifest_id))
            .map(|(id, _)| id.clone())
            .collect();
        let ambiguous = original
            .values()
            .filter(|d| d.project == manifest.project && d.path == manifest.path)
            .count()
            > 1;
        let prepared = rust_cargo::build_input(manifest)
            .unwrap()
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|input| {
                if ambiguous {
                    return Err("manifest_ambiguous".into());
                }
                selection(manifest, input).map(|selected| (input, selected))
            });
        let (input, (features, enabled_deps, forwarded)) = match prepared {
            Ok(value) => value,
            Err(code) => {
                result.diagnostics.push(serde_json::json!({"document_id":manifest_id,"path":manifest.path,"code":"RUST_BUILD_CONFIGURATION_INVALID","reason":code}));
                for id in &sources {
                    block(
                        docs.get_mut(id).unwrap(),
                        "RUST_BUILD_CONFIGURATION_INVALID",
                    );
                }
                result.contexts.push(serde_json::json!({"manifest_path":manifest.path,"project":manifest.project,"available":false,"reason":code}));
                continue;
            }
        };
        let configuration_id = content_hash(
            &serde_json::to_string(&(
                &manifest.project,
                &manifest.path,
                &manifest.hash,
                input,
                &features,
            ))
            .unwrap(),
        );
        let mut view = original.clone();
        let mut selected_manifest = manifest.clone();
        if let Err(reason) = rust_cargo::restrict(
            &mut selected_manifest,
            input,
            &features,
            &enabled_deps,
            &original,
        ) {
            for id in &sources {
                block(docs.get_mut(id).unwrap(), "RUST_BUILD_TARGET_UNAVAILABLE");
            }
            result.contexts.push(serde_json::json!({"configuration_id":configuration_id,"manifest_path":manifest.path,"project":manifest.project,"available":false,"reason":reason}));
            continue;
        }
        view.insert(manifest_id.clone(), selected_manifest.clone());
        let mut unknown = 0;
        let mut disabled = 0;
        for id in &sources {
            let doc = view.get_mut(id).unwrap();
            let conditions = doc.rust_cfg.conditions.clone();
            let decision = |at: usize| -> Option<bool> {
                let mut unknown = false;
                for condition in conditions.iter().filter(|c| c.start <= at && at < c.end) {
                    match condition
                        .predicate
                        .as_ref()
                        .and_then(|p| evaluate(p, input, &features))
                    {
                        Some(false) => return Some(false),
                        None => unknown = true,
                        _ => (),
                    }
                }
                if unknown {
                    None
                } else {
                    Some(true)
                }
            };
            unknown += conditions
                .iter()
                .filter(|c| {
                    c.predicate
                        .as_ref()
                        .and_then(|p| evaluate(p, input, &features))
                        .is_none()
                })
                .count();
            disabled += conditions
                .iter()
                .filter(|c| {
                    c.predicate
                        .as_ref()
                        .and_then(|p| evaluate(p, input, &features))
                        == Some(false)
                })
                .count();
            let dropped_owners: Vec<_> = conditions
                .iter()
                .filter(|c| {
                    projectable_condition(&c.kind)
                        && c.predicate
                            .as_ref()
                            .and_then(|p| evaluate(p, input, &features))
                            == Some(false)
                })
                .collect();
            if conditions.iter().any(|c| {
                !projectable_condition(&c.kind)
                    && !(matches!(c.kind.as_str(), "macro_invocation" | "macro_definition")
                        && decision(c.start) == Some(false))
                    && !dropped_owners
                        .iter()
                        .any(|parent| parent.start <= c.start && c.end <= parent.end)
            }) {
                doc.binding_limitations
                    .push("RUST_CFG_NESTED_SEMANTICS_UNMODELED".into());
            }
            doc.symbols.retain(|s| decision(s.start_byte) == Some(true));
            doc.references
                .retain(|r| decision(r.start_byte) == Some(true));
            doc.bases.retain(|b| decision(b.start) == Some(true));
            doc.routes.retain(|r| decision(r.start) == Some(true));
            doc.endpoints
                .retain(|r| decision(r.start_byte) == Some(true));
            rust_cargo::filter_modules(doc, decision);
            rust_cargo::configure_reexports(doc, decision);
            resolve::configure_rust_bindings(doc, decision);
            rust_semantics::configure(doc, &decision);
        }
        let paths = rust_cargo::configured_paths(&selected_manifest, &view);
        let mut excluded = 0;
        for id in &sources {
            let mut doc = view.remove(id).unwrap();
            if !paths.contains(&doc.path) {
                excluded += 1;
                doc.symbols.clear();
                doc.references.clear();
                doc.bases.clear();
                doc.rust_semantics = rust_semantics::Facts::default();
                doc.policies.clear();
                doc.sql = sql::SqlDocument::default();
                doc.routes.clear();
                doc.endpoints.clear();
                doc.test_definitions.clear();
            }
            let remap: HashMap<_, _> = doc
                .symbols
                .iter()
                .map(|s| (s.id.clone(), format!("{}@{}", s.id, configuration_id)))
                .collect();
            for s in &mut doc.symbols {
                s.id = remap[&s.id].clone();
                s.build_context = Some(configuration_id.clone());
            }
            for r in &mut doc.references {
                r.caller = r.caller.as_ref().and_then(|id| remap.get(id)).cloned();
                r.build_context = Some(configuration_id.clone());
            }
            rust_semantics::remap(&mut doc, &remap);
            policy::remap_build_context(&mut doc, &remap);
            if doc
                .binding_limitations
                .iter()
                .any(|s| s == "RUST_CFG_NESTED_SEMANTICS_UNMODELED")
            {
                doc.policies.clear();
            }
            let conditions = doc.rust_cfg.conditions.clone();
            // Policy decisions capture enclosing return/control-flow syntax,
            // rather than independently projectable argument facts. Do not
            // publish decisions from an unprojected conditional argument body.
            if conditions.iter().any(|c| {
                matches!(
                    c.kind.as_str(),
                    "argument_expression"
                        | "block"
                        | "expression_statement"
                        | "field_initializer"
                        | "shorthand_field_initializer"
                ) && c
                    .predicate
                    .as_ref()
                    .and_then(|p| evaluate(p, input, &features))
                    != Some(true)
            }) {
                doc.policies.clear();
            }
            sql::configure_rust_context(&mut doc, &remap, |line| {
                conditions
                    .iter()
                    .filter(|c| c.start_line <= line && line <= c.end_line)
                    .all(|c| {
                        c.predicate
                            .as_ref()
                            .and_then(|p| evaluate(p, input, &features))
                            == Some(true)
                    })
            });
            docs.insert(id.clone(), doc);
        }
        docs.insert(manifest_id.clone(), selected_manifest);
        result.contexts.push(serde_json::json!({"configuration_id":configuration_id,"manifest_path":manifest.path,"manifest_revision":manifest.revision,"manifest_hash":manifest.hash,"project":manifest.project,
            "available":true,"target_kind":input.target_kind,"target_name":input.target_name,"target_triple":input.target_triple,"features":features.iter().take(64).collect::<Vec<_>>(),"features_total":features.len(),"features_truncated":features.len()>64,"default_features":input.default_features,
            "cfg_flags":input.flags.iter().take(64).collect::<Vec<_>>(),"cfg_flags_total":input.flags.len(),"cfg_flags_truncated":input.flags.len()>64,"cfg_values":input.values.iter().take(16).map(|(k,v)|(k,v.iter().take(8).collect::<Vec<_>>())).collect::<BTreeMap<_,_>>(),"cfg_values_truncated":input.values.len()>16||input.values.values().any(|v|v.len()>8),"disabled_conditions":disabled,"unknown_conditions":unknown,"excluded_source_files":excluded,
            "scope":"one_explicit_compilation_target_per_package","dependency_feature_forwarding_verified":!forwarded,"multi_target_instantiation":false,"compiler_verified":false}));
    }
    result
        .contexts
        .sort_by_key(|v| v["manifest_path"].as_str().unwrap_or("").to_string());
    result
}

#[cfg(test)]
mod tests;
