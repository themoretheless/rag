//! Source occurrences and symbol-bound references. Unresolved names are never promoted to links.
use super::*;

fn compound(kind: &str) -> bool {
    matches!(
        kind,
        "attribute"
            | "selector_expression"
            | "qualified_type"
            | "qualified_identifier"
            | "member_expression"
            | "field_expression"
            | "field_access"
            | "scoped_identifier"
            | "scoped_type_identifier"
            | "nested_type_identifier"
    )
}
/// Keep qualified member reads intact; an object's bare property name must not
/// accidentally bind to an unrelated same-name function in the enclosing scope.
pub(super) fn occurrence<'a>(
    node: Node<'a>,
    source: &str,
    doc: &IndexedDocument,
) -> Option<(Node<'a>, String)> {
    if doc.shell.is_some() {
        return None;
    }
    if doc.c_family.is_some() && super::c_family::non_usage(node) {
        return None;
    }
    if doc.java.is_some() && super::java::non_usage(node) {
        return None;
    }
    if doc.java.is_some() && node.kind() == "method_reference" {
        return super::java::reference_occurrence(node, source);
    }
    if doc.path.ends_with(".go") && super::go::non_usage(node) {
        return None;
    }
    let mut ancestor = node.parent();
    while let Some(parent) = ancestor {
        if matches!(
            parent.kind(),
            "use_declaration" | "import_statement" | "import_from_statement"
        ) {
            return if matches!(node.kind(), "identifier" | "type_identifier")
                && resolve::import_local_at(doc, parent, node, source)
            {
                Some((node, text(node, source).into()))
            } else {
                None
            };
        }
        ancestor = parent.parent();
    }
    if let Some(parent) = node.parent() {
        if compound(parent.kind()) {
            for field in ["attribute", "property", "field", "name"] {
                if parent
                    .child_by_field_name(field)
                    .is_some_and(|n| n.id() == node.id())
                {
                    return None;
                }
            }
        }
        // These identifiers introduce a binding. Annotations/initializers still
        // traverse normally and can refer to an existing symbol.
        if matches!(
            parent.kind(),
            "parameter"
                | "typed_parameter"
                | "required_parameter"
                | "optional_parameter"
                | "default_parameter"
                | "typed_default_parameter"
                | "keyword_argument"
                | "type_parameter"
                | "rest_pattern"
                | "list_splat_pattern"
                | "dictionary_splat_pattern"
                | "variable_declarator"
                | "let_declaration"
        ) {
            for field in ["name", "pattern"] {
                if parent
                    .child_by_field_name(field)
                    .is_some_and(|n| n.id() == node.id())
                {
                    return None;
                }
            }
            if parent.kind() == "typed_parameter"
                && parent.named_child(0).is_some_and(|n| n.id() == node.id())
            {
                return None;
            }
        }
        if matches!(
            parent.kind(),
            "rest_pattern" | "list_splat_pattern" | "dictionary_splat_pattern"
        ) {
            return None;
        }
        if parent.kind() == "assignment_pattern"
            && parent
                .child_by_field_name("left")
                .is_some_and(|n| n.id() == node.id())
        {
            return None;
        }
        if matches!(parent.kind(), "parameters" | "formal_parameters")
            && node.kind() == "identifier"
        {
            return None;
        }
        if matches!(parent.kind(), "assignment" | "assignment_expression")
            && parent
                .child_by_field_name("left")
                .is_some_and(|n| n.id() == node.id())
            && node.kind() == "identifier"
        {
            return None;
        }
    }
    if compound(node.kind()) {
        // Computed properties (obj[key]) have runtime names, not the symbol `key`.
        if node.child_by_field_name("property").is_some_and(|n| {
            !matches!(
                n.kind(),
                "property_identifier" | "identifier" | "type_identifier"
            )
        }) {
            return None;
        }
        return last_identifier(node).map(|name| (name, text(node, source).into()));
    }
    if matches!(
        node.kind(),
        "identifier" | "type_identifier" | "shorthand_property_identifier"
    ) {
        Some((node, text(node, source).into()))
    } else {
        None
    }
}

pub(super) fn query(
    snapshot: &CodeSnapshot,
    q: &CodeQuery,
    selected: &[&Symbol],
    limit: usize,
) -> serde_json::Value {
    let mode = q.reference_mode.as_deref().unwrap_or("bound");
    if !matches!(mode, "bound" | "lexical") {
        return serde_json::json!({"error":"reference_mode must be bound or lexical","items":[],"total":0});
    }
    let ids: HashSet<_> = selected.iter().map(|s| s.id.as_str()).collect();
    let names: HashSet<_> = selected.iter().map(|s| s.name.as_str()).collect();
    let mut items = vec![];
    if mode == "bound" {
        for binding in &snapshot.bound_references {
            if binding
                .candidates
                .iter()
                .any(|id| ids.contains(id.as_str()))
            {
                // Preserve the historical flat occurrence fields, adding evidence
                // rather than changing every occurrence to a nested edge shape.
                let mut item = serde_json::to_value(&binding.reference).unwrap();
                item["candidates"] = serde_json::json!(binding.candidates);
                item["resolution"] = serde_json::json!(binding.resolution);
                item["resolution_basis"] = serde_json::json!(binding.resolution_basis);
                item["provenance"] = serde_json::json!(binding.provenance);
                items.push(item);
            }
        }
    } else {
        for r in &snapshot.references {
            if names.contains(r.name.as_str()) {
                let mut item = serde_json::to_value(r).unwrap();
                item["resolution"] = serde_json::json!("lexical_name_match");
                item["provenance"] = serde_json::json!("lexical");
                items.push(item);
            }
        }
    }
    items.sort_by(|a, b| {
        (
            a["document_id"].as_str(),
            a["start_byte"].as_u64(),
            a["expression"].as_str(),
        )
            .cmp(&(
                b["document_id"].as_str(),
                b["start_byte"].as_u64(),
                b["expression"].as_str(),
            ))
    });
    let overrides: Vec<_> = snapshot
        .overrides
        .iter()
        .filter(|e| mode == "bound" && e.candidates.iter().any(|id| ids.contains(id.as_str())))
        .collect();
    serde_json::json!({"method_relations":{"items":overrides.iter().take(limit).collect::<Vec<_>>(),"total":overrides.len(),"truncated":overrides.len()>limit,"kind":"override_candidate","semantic_override_proven":false},"items":items.iter().take(limit).collect::<Vec<_>>(),"total":items.len(),"truncated":items.len()>limit,
        "matched_symbols":selected.len(),"reference_mode":mode,"references_are_lexical":mode=="lexical",
        "resolution":"conservative_static_bindings","runtime_dispatch_proven":false,
        "stale_documents":snapshot.stale_documents,"unindexed_documents":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors,
        "unresolved_occurrences":snapshot.bound_references.iter().filter(|r|r.candidates.is_empty()).count(),
        "absence_proven":false})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(files: &[(&str, &str, &str)]) -> CodeSnapshot {
        let mut docs = HashMap::new();
        let mut out = CodeSnapshot::default();
        for (i, (path, project, source)) in files.iter().enumerate() {
            let mut d = parse(&i.to_string(), path, source, 1).unwrap();
            assert!(!d.parse_errors, "{path}");
            d.project = Some((*project).into());
            out.symbols.extend(d.symbols.clone());
            out.references.extend(d.references.clone());
            docs.insert(i.to_string(), d);
        }
        out.bound_references = resolve::references(&out.symbols, &docs);
        out.calls = out
            .bound_references
            .iter()
            .filter(|r| r.reference.is_call)
            .cloned()
            .collect();
        out.entrypoints = analysis::entrypoints(&out.symbols, &docs);
        out
    }
    fn query(s: &CodeSnapshot, path: &str, name: &str, mode: Option<&str>) -> Value {
        s.query(
            "references",
            &CodeQuery {
                path: Some(path.into()),
                symbol: Some(name.into()),
                reference_mode: mode.map(str::to_owned),
                ..Default::default()
            },
        )
    }
    use serde_json::Value;
    #[test]
    fn python_alias_callbacks_shadowing_and_member_reads_bind_to_definition() {
        let s=snapshot(&[
            ("/p/api.py","p","def work():\n    pass\nclass Service:\n    def run(self):\n        pass\n"),
            ("/p/main.py","p","from api import work as execute, Service\ncallback = execute\ndef caller(execute):\n    execute()\n    return execute\ndef use(s: Service):\n    cb = s.run\n    execute()\n    return cb\n"),
            ("/p/other.py","p","def work():\n    pass\ndef local():\n    return work\n")]);
        let r = query(&s, "/p/api.py", "work", None);
        assert!(
            r["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["expression"] == "execute" && !r["is_call"].as_bool().unwrap()),
            "{r}"
        );
        assert!(
            r["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["expression"] == "execute" && r["is_call"] == true),
            "{r}"
        );
        assert!(
            !r["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["path"] == "/p/other.py"),
            "{r}"
        );
        assert!(
            !r["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["line"] == 4 || r["line"] == 5),
            "{r}"
        );
        let r = query(&s, "/p/api.py", "Service::run", None);
        assert_eq!(r["total"], 1, "{r}");
        assert_eq!(r["items"][0]["expression"], "s.run");
        assert_eq!(r["items"][0]["resolution_basis"], "receiver_type");
        let lexical = query(&s, "/p/api.py", "work", Some("lexical"));
        assert!(lexical["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["path"] == "/p/other.py"));
        assert!(!lexical["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["expression"] == "execute"));
    }
    #[test]
    fn rust_imported_function_values_qualified_methods_and_type_uses_are_bound() {
        let s=snapshot(&[
            ("/p/src/api.rs","p","pub fn work() {} pub struct Service; impl Service { pub fn run(&self) {} }"),
            ("/p/src/main.rs","p","use crate::api::{work as execute, Service}; fn main() { let callback = execute; let other = crate::api::work; } fn use_it(s: Service) { let cb = Service::run; s.run(); }"),
        ]);
        let r = query(&s, "/p/src/api.rs", "work", None);
        for expression in ["execute", "crate::api::work"] {
            assert!(
                r["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["expression"] == expression),
                "{r}"
            );
        }
        let r = query(&s, "/p/src/api.rs", "Service::run", None);
        assert!(
            r["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["expression"] == "Service::run"),
            "{r}"
        );
        let r = query(&s, "/p/src/api.rs", "Service", None);
        assert!(
            r["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["expression"] == "Service"),
            "{r}"
        );
    }
    #[test]
    fn js_namespace_default_alias_and_inherited_method_values_keep_full_expression() {
        let s=snapshot(&[
            ("/p/api.ts","p","export default function work() {} export class Base { run() {} } export class Child extends Base {}"),
            ("/p/main.ts","p","import execute, {Child} from './api'; import * as api from './api'; const cb = execute; const cb2 = api.default; function use(c: Child) { const cb = c.run; return cb; } function run() {} const x = unknown.run;"),
        ]);
        let r = query(&s, "/p/api.ts", "work", None);
        for expression in ["execute", "api.default"] {
            assert!(
                r["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r["expression"] == expression),
                "{r}"
            );
        }
        let r = query(&s, "/p/api.ts", "Base::run", None);
        assert_eq!(r["total"], 1, "{r}");
        assert_eq!(r["items"][0]["expression"], "c.run");
        assert_eq!(query(&s, "/p/main.ts", "run", None)["total"], 0);
    }
    #[test]
    fn bound_references_do_not_cross_projects_and_keep_ambiguous_candidates_explicit() {
        let s = snapshot(&[
            (
                "/p/a.py",
                "one",
                "from target import work as execute\ncb = execute\n",
            ),
            ("/p/target.py", "two", "def work():\n    pass\n"),
        ]);
        assert_eq!(query(&s, "/p/target.py", "work", None)["total"], 0);
        let s = snapshot(&[(
            "/p/a.py",
            "p",
            "def work():\n    pass\ndef work():\n    pass\ncb = work\n",
        )]);
        let r = query(&s, "/p/a.py", "work", None);
        assert_eq!(r["items"][0]["resolution"], "ambiguous", "{r}");
        assert_eq!(r["items"][0]["candidates"].as_array().unwrap().len(), 2);
        assert_eq!(r["absence_proven"], false);
        assert!(query(&s, "/p/a.py", "work", Some("typo"))["error"].is_string());
    }
    #[test]
    fn callbacks_preserve_transitive_callee_reachability() {
        let s = snapshot(&[
            (
                "/p/api.py",
                "p",
                "def callback():\n    helper()\ndef helper():\n    pass\ndef unused():\n    pass\n",
            ),
            (
                "/p/main.py",
                "p",
                "from api import callback as execute\nregistered = execute\n",
            ),
        ]);
        let r = s.query("dead_code", &CodeQuery::default());
        let names: Vec<_> = r["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["symbol"]["name"].as_str())
            .collect();
        assert_eq!(names, vec!["unused"], "{r}");
    }
    #[test]
    fn import_source_tokens_keyword_names_and_default_parameters_do_not_bind_as_usages() {
        let s=snapshot(&[
            ("/p/api.py","p","def work():\n    pass\n"),
            ("/p/main.py","p","from api import work as execute\ndef work():\n    pass\ndef shadow(execute=None):\n    return execute\ndef kwargs():\n    other(work=1)\n"),
        ]);
        assert_eq!(query(&s, "/p/main.py", "work", None)["total"], 0);
        let r = query(&s, "/p/api.py", "work", None);
        assert_eq!(r["total"], 1, "{r}");
        assert_eq!(r["items"][0]["expression"], "execute");
        let s=snapshot(&[("/p/a.ts","p","function work() {} function f(work=()=>{}) { work(); return work; } function g(...work) { return work; }")]);
        assert_eq!(query(&s, "/p/a.ts", "work", None)["total"], 0);
    }
}
