use super::*;
fn graph(files: &[(&str, &str, &str)]) -> (CodeSnapshot, HashMap<String, IndexedDocument>) {
    let mut docs = HashMap::new();
    let mut s = CodeSnapshot::default();
    for (i, (project, path, body)) in files.iter().enumerate() {
        let mut d = parse(&i.to_string(), path, body, 1).unwrap();
        assert!(!d.parse_errors, "{path}");
        d.project = Some((*project).into());
        s.document_scopes.insert(
            i.to_string(),
            DocumentScope {
                path: (*path).into(),
                project: Some((*project).into()),
            },
        );
        s.symbols.extend(d.symbols.clone());
        s.references.extend(d.references.clone());
        docs.insert(i.to_string(), d);
    }
    s.bound_references = crate::code_index::resolve::references(&s.symbols, &docs);
    s.calls = s
        .bound_references
        .iter()
        .filter(|e| e.reference.is_call)
        .cloned()
        .collect();
    (s, docs)
}
fn targets(s: &CodeSnapshot, expression: &str) -> Vec<String> {
    let edge = s
        .bound_references
        .iter()
        .find(|e| e.reference.expression == expression)
        .unwrap_or_else(|| panic!("missing {expression}: {:?}", s.bound_references));
    edge.candidates
        .iter()
        .map(|id| {
            s.symbols
                .iter()
                .find(|s| &s.id == id)
                .unwrap()
                .qualified_name
                .clone()
        })
        .collect()
}
#[test]
fn php_names_namespace_functions_aliases_case_and_prefixes() {
    let (s, _) = graph(&[
        (
            "p",
            "/p/lib.php",
            r"<?php namespace Lib; function hello(){} namespace Lib\Sub; function work(){}",
        ),
        (
            "p",
            "/p/app.php",
            r"<?php namespace App; use function Lib\hello as greet; use Lib as L; function local(){} function run(){GREET(); local(); L\Sub\work(); \Lib\hello(); namespace\local();}",
        ),
    ]);
    assert_eq!(targets(&s, "GREET"), vec!["Lib\\hello"]);
    assert_eq!(targets(&s, "local"), vec!["App\\local"]);
    assert_eq!(targets(&s, "L\\Sub\\work"), vec!["Lib\\Sub\\work"]);
    assert_eq!(targets(&s, "\\Lib\\hello"), vec!["Lib\\hello"]);
    assert_eq!(targets(&s, "namespace\\local"), vec!["App\\local"]);
    assert!(s
        .symbols
        .iter()
        .any(|s| s.kind == "namespace_definition" && s.qualified_name == "Lib"));
}
#[test]
fn php_names_group_import_domains_and_reopened_scopes_do_not_leak() {
    let (s, docs) = graph(&[
        (
            "p",
            "/p/lib.php",
            r"<?php namespace Lib; function ping(){} class Service {public static function save(){}}",
        ),
        (
            "p",
            "/p/app.php",
            r"<?php namespace App {use Lib\{Service as S, function ping as p, const FLAG}; function one(){p(); S::save();}} namespace Other {function two(){p(); S::save();}}",
        ),
    ]);
    assert_eq!(targets(&s, "p"), vec!["Lib\\ping"]);
    assert_eq!(targets(&s, "S::save"), vec!["Lib\\Service::save"]);
    for e in s.calls.iter().filter(|e| {
        e.reference
            .caller
            .as_ref()
            .is_some_and(|id| s.symbols.iter().any(|s| &s.id == id && s.name == "two"))
    }) {
        assert!(e.candidates.is_empty(), "{e:?}");
    }
    let f = docs["1"].php_names.as_ref().unwrap();
    let imports = &f.scopes[1].imports;
    assert_eq!(imports.len(), 3);
    assert!(imports
        .iter()
        .any(|i| i.kind == "const" && i.target == "Lib\\FLAG"));
}
#[test]
fn php_names_fallback_is_function_only_and_project_isolated() {
    let (s, _) = graph(&[
        (
            "p",
            "/p/global.php",
            r"<?php function fallback(){} class GlobalOnly{public static function save(){}}",
        ),
        (
            "p",
            "/p/ns.php",
            r"<?php namespace N; function fallback(){} function run(){fallback(); GlobalOnly::save(); missing();}",
        ),
        (
            "q",
            "/q/other.php",
            r"<?php namespace N; function missing(){}",
        ),
    ]);
    assert_eq!(targets(&s, "fallback"), vec!["N\\fallback"]);
    assert!(targets(&s, "GlobalOnly::save").is_empty());
    assert!(targets(&s, "missing").is_empty());
    let (s, _) = graph(&[
        ("p", "/p/global.php", r"<?php function fallback(){}"),
        ("p", "/p/ns.php", r"<?php namespace N; fallback();"),
    ]);
    assert_eq!(targets(&s, "fallback"), vec!["fallback"]);
    assert!(s.calls[0].resolution_basis.contains("FALLBACK"));
    assert_eq!(s.calls[0].reference.document_id, "1");
}
#[test]
fn php_names_declared_static_this_visibility_and_late_dispatch_guards() {
    let (s, _) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php namespace N; class Service {public static function save(){} private static function secret(){} public function instance(){} public function run(){self::save(); $this->instance(); self::secret(); static::save(); parent::save(); $other->instance();} public static function bad(){$this->instance();}} function outside(){Service::save(); Service::secret(); Service::instance();}",
    )]);
    assert_eq!(targets(&s, "self::save"), vec!["N\\Service::save"]);
    assert_eq!(targets(&s, "$this->instance"), vec!["N\\Service::instance"]);
    assert_eq!(targets(&s, "self::secret"), vec!["N\\Service::secret"]);
    assert_eq!(targets(&s, "Service::save"), vec!["N\\Service::save"]);
    for expr in [
        "static::save",
        "parent::save",
        "<dynamic>->instance",
        "Service::secret",
        "Service::instance",
    ] {
        assert!(targets(&s, expr).is_empty(), "{expr}");
    }
    assert!(s
        .calls
        .iter()
        .find(|e| e
            .reference
            .caller
            .as_ref()
            .is_some_and(|id| s.symbols.iter().any(|s| &s.id == id && s.name == "bad")))
        .unwrap()
        .candidates
        .is_empty());
}
#[test]
fn php_names_first_class_values_types_and_anonymous_owner_barrier() {
    let (s, _) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php namespace N; function work(){} class Service{public static function save(){}} function run(){ $f=work(...); $g=Service::save(...); $x=new Service; $c=function(){work();}; $d=fn()=>work(); $f(); }",
    )]);
    assert_eq!(
        s.bound_references
            .iter()
            .filter(|e| e.reference.expression == "work" && !e.reference.is_call)
            .count(),
        1
    );
    assert_eq!(targets(&s, "Service"), vec!["N\\Service"]);
    assert!(s
        .calls
        .iter()
        .filter(|e| e.reference.expression == "work")
        .all(|e| e.reference.caller.is_none()));
    assert!(targets(&s, "<dynamic_php_function>").is_empty());
    assert!(!s
        .calls
        .iter()
        .any(|e| e.reference.expression == "Service::save"));
}
#[test]
fn php_names_conditional_duplicate_imports_traits_and_parse_errors_are_not_guessed() {
    let (s, docs) = graph(&[
        (
            "p",
            "/p/a.php",
            r"<?php namespace N; if ($flag) {function conditional(){}} function run(){conditional();} trait T{public static function save(){} public function run(){self::save();}}",
        ),
        (
            "p",
            "/p/b.php",
            r"<?php namespace B; use function N\conditional as x; use function N\other as x; x();",
        ),
    ]);
    assert!(targets(&s, "conditional").is_empty());
    assert!(targets(&s, "self::save").is_empty());
    assert!(targets(&s, "x").is_empty());
    assert!(!docs["1"].php_names.as_ref().unwrap().scopes[1].complete);
    let mut broken = parse(
        "broken",
        "broken.php",
        "<?php function target(){} function run(){target();} function broken( {",
        1,
    )
    .unwrap();
    assert!(broken.parse_errors);
    broken.project = Some("p".into());
    let mut docs = docs;
    docs.insert("broken".into(), broken);
    let e = document(&docs, &docs["broken"]);
    assert!(e.iter().all(|e| e.candidates.is_empty()));
}

#[test]
fn php_names_flat_import_lists_separate_class_function_domains_and_type_occurrences() {
    let (s, _) = graph(&[
        (
            "p",
            "/p/lib.php",
            r"<?php namespace Lib; function first(){} function second(){} class Service {public static function save(){}}",
        ),
        (
            "p",
            "/p/app.php",
            r"<?php namespace App; use function Lib\first as Same, Lib\second as second; use Lib\Service as Same; function run(Same $p): Same {Same(); second(); Same::save(); $x=Same::class; return new Same;}",
        ),
    ]);
    assert_eq!(targets(&s, "Same"), vec!["Lib\\Service"]);
    assert!(s.calls.iter().any(|e| e.reference.expression == "Same"
        && e.candidates.iter().any(|id| s
            .symbols
            .iter()
            .any(|s| &s.id == id && s.qualified_name == "Lib\\first"))));
    assert_eq!(targets(&s, "second"), vec!["Lib\\second"]);
    assert_eq!(targets(&s, "Same::save"), vec!["Lib\\Service::save"]);
    assert!(
        s.bound_references
            .iter()
            .filter(|e| e.reference.expression == "Same" && !e.reference.is_call)
            .count()
            >= 4
    );
}
#[test]
fn php_names_duplicate_targets_stay_ambiguous_and_other_languages_are_not_php_functions() {
    let (s, _) = graph(&[
        (
            "p",
            "/p/a.php",
            r"<?php namespace N; function same(){} function run(){same(); foreign();}",
        ),
        ("p", "/p/b.php", r"<?php namespace N; function SAME(){}"),
        ("p", "/p/c.py", "def foreign(): pass"),
    ]);
    assert_eq!(targets(&s, "same").len(), 2);
    assert_eq!(
        s.calls
            .iter()
            .find(|e| e.reference.expression == "same")
            .unwrap()
            .resolution,
        "ambiguous"
    );
    assert!(targets(&s, "foreign").is_empty());
}
#[test]
fn php_names_cache_capability_repair_and_frozen_alias_changes() {
    use crate::models::Document;
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("names.duckdb")).unwrap();
    let doc = |id: &str, body: &str| Document {
        id: id.into(),
        uri: format!("file:///p/{id}.php"),
        source_file: Some(format!("/p/{id}.php")),
        content: body.into(),
        wing: Some("p".into()),
        ..Default::default()
    };
    store
        .upsert_document(&doc(
            "lib",
            r"<?php namespace Lib; function first(){} function other(){}",
        ))
        .unwrap();
    let source = r"<?php namespace App; use function Lib\first as go; function run(){go();}";
    store.upsert_document(&doc("app", source)).unwrap();
    let before = store.index_code().unwrap().index_revision.unwrap();
    let payload = crate::code_index::test_get_code_doc_payload(&store, "app");
    let mut payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
    payload.as_object_mut().unwrap().remove("php_names");
    crate::code_index::test_set_code_doc_payload(&store, "app", &payload.to_string());
    assert!(store
        .code_snapshot()
        .unwrap()
        .stale_documents
        .contains(&"app".into()));
    assert_eq!(store.index_code().unwrap().indexed, 1);
    store
        .upsert_document(&doc("app", &source.replace("Lib\\first", "Lib\\other")))
        .unwrap();
    let after = store.index_code().unwrap().index_revision.unwrap();
    let q = history::DiffQuery {
        before,
        after: Some(after),
        project: Some("p".into()),
        limit: None,
    };
    let diff = store.code_index_diff(&q).unwrap();
    assert_eq!(diff["php_name_binding_comparison"]["available"], true);
    assert!(diff["edges_added_total"].as_u64().unwrap() > 0);
    assert!(diff["edges_removed_total"].as_u64().unwrap() > 0);
    let graph = crate::code_index::test_get_rev_graph_json(&store, before);
    let mut graph: serde_json::Value = serde_json::from_str(&graph).unwrap();
    graph.as_object_mut().unwrap().remove("php_names_format");
    crate::code_index::test_set_rev_graph_json(&store, before, &graph.to_string());
    assert_eq!(
        store.code_index_diff(&q).unwrap()["php_name_binding_comparison"]["available"],
        false
    );
}

#[test]
fn php_names_capture_limits_fail_closed_with_explicit_capability_state() {
    let source = format!(
        "<?php namespace N; /*{}*/ function f(){{}} f();",
        "x".repeat(MAX_SOURCE)
    );
    let doc = parse("large", "large.php", &source, 1).unwrap();
    assert!(!doc.parse_errors);
    assert!(current(&doc));
    assert!(!doc.php_names.as_ref().unwrap().complete);
    assert!(doc.references.is_empty());
    assert!(doc
        .symbols
        .iter()
        .all(|s| s.qualified_name.starts_with("<php-name-unavailable@")));
    assert!(doc
        .binding_limitations
        .iter()
        .any(|s| s == "PHP_NAME_CAPTURE_SOURCE_LIMIT"));
    let source = format!(
        "<?php {}",
        (0..129)
            .map(|i| format!("namespace N{i}; "))
            .collect::<String>()
    );
    let doc = parse("scopes", "scopes.php", &source, 1).unwrap();
    assert!(!doc.php_names.as_ref().unwrap().complete);
    assert_eq!(doc.php_names.as_ref().unwrap().scopes.len(), 128);
}

#[test]
fn php_names_inherited_methods_imported_chain_parent_override_and_visibility() {
    let (s, docs) = graph(&[
        (
            "p",
            "/p/base.php",
            r"<?php namespace Lib; class Base {public static function save(){} protected function work(){} private function hidden(){} public function replace(){}} class Mid extends Base {}",
        ),
        (
            "p",
            "/p/app.php",
            r"<?php namespace App; use Lib\Mid as M; class Child extends M {public function replace(){} public function run(){self::save();$this->work();parent::replace();$this->replace();$this->hidden();} public static function noInstance(){parent::work();}} function outside(){Child::save();Child::work();} ",
        ),
        (
            "q",
            "/q/base.php",
            r"<?php namespace Lib; class Base {public static function save(){}}",
        ),
    ]);
    assert_eq!(targets(&s, "self::save"), vec!["Lib\\Base::save"]);
    assert_eq!(targets(&s, "$this->work"), vec!["Lib\\Base::work"]);
    assert_eq!(targets(&s, "parent::replace"), vec!["Lib\\Base::replace"]);
    assert_eq!(targets(&s, "$this->replace"), vec!["App\\Child::replace"]);
    assert_eq!(targets(&s, "Child::save"), vec!["Lib\\Base::save"]);
    for name in ["$this->hidden", "parent::work", "Child::work"] {
        assert!(targets(&s, name).is_empty(), "{name}")
    }
    let inheritance = super::super::inheritance::edges(&s.symbols, &docs);
    assert_eq!(inheritance.len(), 2);
    assert!(inheritance
        .iter()
        .all(|e| e.candidates.len() == 1 && e.hierarchy_status != "complete_static"));
    let overrides = super::super::inheritance::overrides(&s.symbols, &docs);
    assert_eq!(overrides.len(), 1);
    assert!(!overrides[0].semantic_override_proven);
    assert_eq!(
        s.symbols
            .iter()
            .find(|s| s.id == overrides[0].candidates[0])
            .unwrap()
            .qualified_name,
        "Lib\\Base::replace"
    );
}

#[test]
fn php_names_inheritance_guards_cycles_missing_bases_traits_final_and_abstract() {
    for (declarations, expected) in [
        ("class Base extends Missing {public static function save(){}} class Child extends Base {}", "PHP_BASE_TARGET_NOT_CAPTURED"),
        ("class Base extends Child {public static function save(){}} class Child extends Base {}", "PHP_INHERITANCE_CYCLE"),
        ("final class Base {public static function save(){}} class Child extends Base {}", "PHP_INVALID_BASE_KIND_OR_FINAL"),
        ("class Base {public static function save(){}} trait T {} class Child extends Base {use Missing;}", "PHP_TRAIT_TARGET_NOT_CAPTURED"),
        ("abstract class Base {abstract public static function save();} class Child extends Base {}", "PHP_ABSTRACT_METHOD_DISPATCH_UNVERIFIED"),
        ("class Base {private static function save(){}} class Child extends Base {}", "PHP_PRIVATE_BASE_METHOD_NOT_INHERITED"),
        ("interface Base {public static function save();} class Child extends Base {}", "PHP_NONCONCRETE_OWNER_UNVERIFIED"),
        ("class Base {public static function save(){}} class Base {public static function save(){}} class Child extends Base {}", "PHP_BASE_IDENTITY_AMBIGUOUS"),
    ] {
        let source = format!("<?php {declarations} function run(){{Child::save();}}");
        let (s, _) = graph(&[("p","/p/a.php",&source)]);
        let edge = s.calls.iter().find(|e|e.reference.expression=="Child::save").unwrap();
        assert!(edge.candidates.is_empty(),"{declarations}: {edge:?}");
        assert_eq!(edge.resolution_basis,expected,"{declarations}");
    }
}

#[test]
fn php_names_traits_shadow_bases_but_local_declarations_take_precedence() {
    let (s, _) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php class Base {public static function save(){}} trait T {public static function save(){}} class Mid extends Base {use T;} class Child extends Mid {} class Local extends Mid {public static function save(){}} function run(){Child::save();Local::save();}",
    )]);
    assert_eq!(targets(&s, "Child::save"), vec!["T::save"]);
    assert_eq!(targets(&s, "Local::save"), vec!["Local::save"]);
}

#[test]
fn php_names_inheritance_interface_relations_and_depth_budget() {
    let (s, docs) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php namespace N; interface Root {} interface I extends Root {} class Base {} class Child extends Base implements I {}",
    )]);
    let edges = super::super::inheritance::edges(&s.symbols, &docs);
    assert_eq!(edges.len(), 3);
    assert!(edges.iter().all(|e| e.candidates.len() == 1));
    assert_eq!(
        edges.iter().filter(|e| e.relation == "implements").count(),
        1
    );
    let mut source = String::from("<?php class C0 {public static function save(){}}");
    for i in 1..=65 {
        source.push_str(&format!("class C{i} extends C{} {{}}", i - 1));
    }
    source.push_str("function run(){C65::save();}");
    let (s, _) = graph(&[("p", "/p/a.php", &source)]);
    assert!(targets(&s, "C65::save").is_empty());
    assert_eq!(s.calls[0].resolution_basis, "PHP_INHERITANCE_DEPTH_LIMIT");
}

#[test]
fn php_names_inheritance_cache_upgrade_and_frozen_base_changes() {
    use crate::models::Document;
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("inheritance.duckdb")).unwrap();
    let doc = |id: &str, body: &str| Document {
        id: id.into(),
        uri: format!("file:///p/{id}.php"),
        source_file: Some(format!("/p/{id}.php")),
        content: body.into(),
        wing: Some("p".into()),
        ..Default::default()
    };
    store.upsert_document(&doc("base", "<?php namespace Lib; class A {public static function save(){}} class B {public static function save(){}}" )).unwrap();
    let body = "<?php namespace App; use Lib\\A as Base; class Child extends Base {public function run(){self::save();}}";
    store.upsert_document(&doc("app", body)).unwrap();
    let before = store.index_code().unwrap().index_revision.unwrap();
    let payload = crate::code_index::test_get_code_doc_payload(&store, "app");
    let mut legacy: serde_json::Value = serde_json::from_str(&payload).unwrap();
    legacy["php_names"]["format"] = serde_json::json!(1);
    for definition in legacy["php_names"]["definitions"].as_array_mut().unwrap() {
        definition.as_object_mut().unwrap().remove("bases");
    }
    crate::code_index::test_set_code_doc_payload(&store, "app", &legacy.to_string());
    assert!(store
        .code_snapshot()
        .unwrap()
        .stale_documents
        .contains(&"app".into()));
    assert_eq!(store.index_code().unwrap().indexed, 1);
    store
        .upsert_document(&doc("app", &body.replace("Lib\\A", "Lib\\B")))
        .unwrap();
    let after = store.index_code().unwrap().index_revision.unwrap();
    let q = history::DiffQuery {
        before,
        after: Some(after),
        project: Some("p".into()),
        limit: None,
    };
    let diff = store.code_index_diff(&q).unwrap();
    assert_eq!(diff["php_inheritance_comparison"]["available"], true);
    assert_eq!(
        diff["php_inheritance_comparison"]["source_semantic_change_proven"],
        false
    );
    assert!(diff["edges_added_total"].as_u64().unwrap() > 0);
    assert!(diff["edges_removed_total"].as_u64().unwrap() > 0);
    assert!(diff["relations_added_total"].as_u64().unwrap() > 0);
    let graph = crate::code_index::test_get_rev_graph_json(&store, before);
    let mut legacy: serde_json::Value = serde_json::from_str(&graph).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("php_inheritance_format");
    crate::code_index::test_set_rev_graph_json(&store, before, &legacy.to_string());
    assert_eq!(
        store.code_index_diff(&q).unwrap()["php_inheritance_comparison"]["available"],
        false
    );
    let legacy_diff = store.code_index_diff(&q).unwrap();
    for key in ["relations_added", "relations_removed"] {
        assert!(legacy_diff[key]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["kind"] != "inheritance" && r["kind"] != "method_override_candidate"));
    }
}

#[test]
fn php_names_inheritance_import_comments_duplicates_and_local_visibility_barriers() {
    let (s, docs) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php class Base {public static function save(){}} class Child extends /* comment */ Base {} class Blocked extends Base {private static function save(){}} class Duplicate {public static function save(){}} class Duplicate {} function run(){Child::save();Blocked::save();Duplicate::save();}",
    )]);
    assert_eq!(targets(&s, "Child::save"), vec!["Base::save"]);
    assert!(targets(&s, "Blocked::save").is_empty());
    assert!(targets(&s, "Duplicate::save").is_empty());
    let edges = super::super::inheritance::edges(&s.symbols, &docs);
    assert_eq!(edges.len(), 2);
    assert!(edges.iter().all(|e| e.base_expression == "Base"));
}

#[test]
fn php_names_inheritance_conditional_bases_and_final_override_relations_fail_closed() {
    let (s, docs) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php if ($flag) {class Conditional {public static function save(){}}} class Child extends Conditional {} class Base {final public function stop(){} public function __construct(){}} class Override extends Base {public function stop(){} public function __construct(){}} function run(){Child::save();}",
    )]);
    assert!(targets(&s, "Child::save").is_empty());
    assert_eq!(
        s.calls[0].resolution_basis,
        "PHP_CONDITIONAL_DECLARATION_UNVERIFIED"
    );
    assert!(super::super::inheritance::overrides(&s.symbols, &docs).is_empty());
}

#[test]
fn php_names_trait_composition_namespace_nested_precedence_and_private_host() {
    let (s, _) = graph(&[
        (
            "p",
            "/p/t.php",
            r"<?php namespace Lib; trait Inner {public static function save(){} protected function work(){} private function secret(){}} trait Outer {use Inner;}",
        ),
        (
            "p",
            "/p/a.php",
            r"<?php namespace App; use Lib\Outer as T; class Base {public static function save(){}} class Host extends Base {use T; public function run(){self::save();$this->work();$this->secret();}} class Child extends Host {public function child(){$this->secret();}} class Local extends Host {public static function save(){}} function outside(){Host::save();Host::work();Host::secret();Child::save();Local::save();}",
        ),
        (
            "q",
            "/q/t.php",
            r"<?php namespace Lib; trait Outer {public static function save(){}}",
        ),
    ]);
    assert_eq!(targets(&s, "self::save"), vec!["Lib\\Inner::save"]);
    assert_eq!(targets(&s, "$this->work"), vec!["Lib\\Inner::work"]);
    assert_eq!(targets(&s, "$this->secret"), vec!["Lib\\Inner::secret"]);
    assert_eq!(targets(&s, "Host::save"), vec!["Lib\\Inner::save"]);
    assert_eq!(targets(&s, "Child::save"), vec!["Lib\\Inner::save"]);
    assert_eq!(targets(&s, "Local::save"), vec!["App\\Local::save"]);
    for expr in ["Host::work", "Host::secret"] {
        assert!(targets(&s, expr).is_empty())
    }
    let child = s
        .symbols
        .iter()
        .find(|s| s.qualified_name == "App\\Child::child")
        .unwrap();
    assert!(s
        .calls
        .iter()
        .find(|e| e.reference.caller.as_deref() == Some(&child.id))
        .unwrap()
        .candidates
        .is_empty());
}

#[test]
fn php_names_trait_insteadof_aliases_keep_excluded_and_original_methods() {
    let (s, docs) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php trait A {public static function save(){}} trait B {public static function save(){}} class Host {use A,B {A::save insteadof B; B::save as other; save as selected;}} function run(){Host::save();Host::other();Host::selected();}",
    )]);
    assert_eq!(targets(&s, "Host::save"), vec!["A::save"]);
    assert_eq!(targets(&s, "Host::other"), vec!["B::save"]);
    assert_eq!(targets(&s, "Host::selected"), vec!["A::save"]);
    let host = docs["0"]
        .php_names
        .as_ref()
        .unwrap()
        .definitions
        .iter()
        .find(|d| d.fqn == "Host")
        .unwrap();
    assert_eq!(host.trait_uses[0].names, vec!["A", "B"]);
    assert_eq!(host.trait_uses[0].rules.len(), 3);
    assert!(host.trait_uses[0].complete);
}

#[test]
fn php_names_trait_visibility_alias_does_not_rename_or_change_original() {
    let (s, _) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php trait T {public static function save(){} public static function hide(){}} class Host {use T {save as private local; hide as protected;} public function run(){self::local();self::hide();}} function run(){Host::save();Host::local();Host::hide();}",
    )]);
    assert_eq!(targets(&s, "Host::save"), vec!["T::save"]);
    assert_eq!(targets(&s, "self::local"), vec!["T::save"]);
    assert_eq!(targets(&s, "self::hide"), vec!["T::hide"]);
    assert!(targets(&s, "Host::local").is_empty());
    assert!(targets(&s, "Host::hide").is_empty());
}

#[test]
fn php_names_trait_conflicts_unknown_cycles_and_nontraits_are_not_guessed() {
    for (declarations,expected) in [
        ("trait A {public static function save(){}} trait B {public static function save(){}} class Host {use A,B;}","PHP_TRAIT_METHOD_CONFLICT"),
        ("class Host {use Unknown;}","PHP_TRAIT_TARGET_NOT_CAPTURED"),
        ("trait A {use B;} trait B {use A;} class Host {use A;}","PHP_TRAIT_COMPOSITION_CYCLE"),
        ("class A {public static function save(){}} class Host {use A;}","PHP_TRAIT_TARGET_KIND_INVALID"),
        ("trait A {public static function save(){}} trait A {} class Host {use A;}","PHP_TRAIT_IDENTITY_AMBIGUOUS"),
        ("if ($flag) {trait A {public static function save(){}}} class Host {use A;}","PHP_CONDITIONAL_TRAIT_UNVERIFIED"),
        ("trait A {abstract public static function save();} class Host {use A;}","PHP_ABSTRACT_METHOD_DISPATCH_UNVERIFIED"),
    ] {
        let body=format!("<?php {declarations} function run(){{Host::save();}}");
        let (s,_)=graph(&[("p","/p/a.php",&body)]);
        assert!(targets(&s,"Host::save").is_empty(),"{body}");
        assert_eq!(s.calls[0].resolution_basis,expected,"{body}");
    }
}

#[test]
fn php_names_trait_multiple_exclusions_unqualified_alias_and_first_class_values() {
    let (s, _) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php trait A {public static function save(){}} trait B {public static function save(){}} trait C {public static function save(){}} class Host {use A,B,C {A::save insteadof B; A::save insteadof C; save as chosen;}} function run(){Host::save();Host::chosen();$f=Host::chosen(...);}",
    )]);
    assert_eq!(targets(&s, "Host::save"), vec!["A::save"]);
    assert_eq!(targets(&s, "Host::chosen"), vec!["A::save"]);
    assert_eq!(s.calls.len(), 2);
    assert!(s.bound_references.iter().any(|e| !e.reference.is_call
        && e.reference.expression == "Host::chosen"
        && !e.candidates.is_empty()));
}

#[test]
fn php_names_trait_composition_depth_work_and_capture_limits_fail_closed() {
    let mut deep = String::from("<?php trait T0 {}");
    for i in 1..=33 {
        deep.push_str(&format!("trait T{i} {{use T{};}}", i - 1));
    }
    deep.push_str("class Host {use T33;} function run(){Host::missing();}");
    let (s, _) = graph(&[("p", "/p/a.php", &deep)]);
    assert_eq!(
        s.calls[0].resolution_basis,
        "PHP_TRAIT_COMPOSITION_DEPTH_LIMIT"
    );
    let mut wide = String::from("<?php trait A0 {} trait B0 {}");
    for i in 1..=13 {
        wide.push_str(&format!(
            "trait A{i} {{use A{},B{};}} trait B{i} {{use A{},B{};}}",
            i - 1,
            i - 1,
            i - 1,
            i - 1
        ));
    }
    wide.push_str("class Host {use A13;} function run(){Host::missing();}");
    let (s, _) = graph(&[("p", "/p/a.php", &wide)]);
    assert_eq!(
        s.calls[0].resolution_basis,
        "PHP_TRAIT_COMPOSITION_WORK_LIMIT"
    );
    let capped=format!("<?php trait T {{public static function save(){{}}}} class Host {{{}}} function run(){{Host::save();}}", "use T;".repeat(65));
    let (s, docs) = graph(&[("p", "/p/a.php", &capped)]);
    assert!(targets(&s, "Host::save").is_empty());
    assert_eq!(s.calls[0].resolution_basis, "PHP_TRAIT_CAPTURE_INCOMPLETE");
    let host = docs["0"]
        .php_names
        .as_ref()
        .unwrap()
        .definitions
        .iter()
        .find(|d| d.fqn == "Host")
        .unwrap();
    assert_eq!(host.trait_uses.len(), 64);
}

#[test]
fn php_names_trait_invalid_adaptations_and_multiple_use_conflicts_stay_explicit() {
    for (rules, expected) in [
        (
            "use A,B {A::save insteadof A;}",
            "PHP_TRAIT_RULE_SELF_EXCLUSION",
        ),
        (
            "use A,B {Missing::save insteadof B;}",
            "PHP_TRAIT_TARGET_NOT_CAPTURED",
        ),
        (
            "use A,B {C::save insteadof B;}",
            "PHP_TRAIT_RULE_SOURCE_NOT_USED",
        ),
        (
            "use A,B {save as private;}",
            "PHP_TRAIT_ADAPTATION_AMBIGUOUS",
        ),
        ("use A; use B;", "PHP_TRAIT_METHOD_CONFLICT"),
        ("use A,A;", "PHP_TRAIT_USE_DUPLICATE"),
    ] {
        let body=format!("<?php trait A {{public static function save(){{}}}} trait B {{public static function save(){{}}}} trait C {{public static function save(){{}}}} class Host {{{rules}}} function run(){{Host::save();}}");
        let (s, _) = graph(&[("p", "/p/a.php", &body)]);
        assert!(targets(&s, "Host::save").is_empty());
        assert_eq!(s.calls[0].resolution_basis, expected, "{body}");
    }
}

#[test]
fn php_names_trait_capture_upgrade_and_frozen_selection_changes() {
    use crate::models::Document;
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("traits.duckdb")).unwrap();
    let doc = |body: &str| Document {
        id: "app".into(),
        uri: "file:///p/a.php".into(),
        source_file: Some("/p/a.php".into()),
        content: body.into(),
        wing: Some("p".into()),
        ..Default::default()
    };
    let body="<?php trait A {public static function save(){}} trait B {public static function save(){}} class Host {use A,B {A::save insteadof B;}} function run(){Host::save();}";
    store.upsert_document(&doc(body)).unwrap();
    let before = store.index_code().unwrap().index_revision.unwrap();
    let payload = crate::code_index::test_get_code_doc_payload(&store, "app");
    let mut legacy: serde_json::Value = serde_json::from_str(&payload).unwrap();
    legacy["php_names"]["format"] = serde_json::json!(2);
    for d in legacy["php_names"]["definitions"].as_array_mut().unwrap() {
        d.as_object_mut().unwrap().remove("trait_uses");
    }
    crate::code_index::test_set_code_doc_payload(&store, "app", &legacy.to_string());
    assert!(store
        .code_snapshot()
        .unwrap()
        .stale_documents
        .contains(&"app".into()));
    assert_eq!(store.index_code().unwrap().indexed, 1);
    store
        .upsert_document(&doc(
            &body.replace("A::save insteadof B", "B::save insteadof A")
        ))
        .unwrap();
    let after = store.index_code().unwrap().index_revision.unwrap();
    let q = history::DiffQuery {
        before,
        after: Some(after),
        project: Some("p".into()),
        limit: None,
    };
    let delta = store.code_index_diff(&q).unwrap();
    assert_eq!(delta["php_traits_comparison"]["available"], true);
    assert!(delta["edges_added_total"].as_u64().unwrap() > 0);
    assert!(delta["edges_removed_total"].as_u64().unwrap() > 0);
    // Separate use-relation capability guard: removing a used trait is comparable only with both inventories.
    store
        .upsert_document(&doc(
            &body.replace("use A,B {A::save insteadof B;}", "use A;")
        ))
        .unwrap();
    let latest = store.index_code().unwrap().index_revision.unwrap();
    let q = history::DiffQuery {
        before,
        after: Some(latest),
        project: Some("p".into()),
        limit: None,
    };
    assert!(store.code_index_diff(&q).unwrap()["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["relation"] == "php_trait_use_candidate"));
    let graph = crate::code_index::test_get_rev_graph_json(&store, before);
    let mut legacy: serde_json::Value = serde_json::from_str(&graph).unwrap();
    legacy.as_object_mut().unwrap().remove("php_traits_format");
    crate::code_index::test_set_rev_graph_json(&store, before, &legacy.to_string());
    let legacy_delta = store.code_index_diff(&q).unwrap();
    assert_eq!(legacy_delta["php_traits_comparison"]["available"], false);
    assert!(legacy_delta["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["relation"] != "php_trait_use_candidate"));
}

#[test]
fn php_names_trait_unsupported_final_adaptations_never_become_aliases() {
    let mut d=parse("unsupported","/p/a.php","<?php trait T {public static function save(){}} class Host {use T {save as final;}} function run(){Host::save();Host::final();}",1).unwrap();
    d.project = Some("p".into());
    if !d.parse_errors {
        let host = d
            .php_names
            .as_ref()
            .unwrap()
            .definitions
            .iter()
            .find(|d| d.fqn == "Host")
            .unwrap();
        assert!(!host.trait_uses[0].complete);
    }
    let mut docs = HashMap::new();
    docs.insert("unsupported".into(), d);
    let calls = super::document(&docs, &docs["unsupported"]);
    assert!(!calls.is_empty());
    assert!(calls.iter().all(|e| e.candidates.is_empty()));
    let d=parse("multi","/p/multi.php","<?php trait A {public static function save(){}} trait B {} trait C {} class Host {use A,B,C {A::save insteadof B,C;}} function run(){Host::save();}",1).unwrap();
    assert!(
        d.parse_errors,
        "locked grammar's multi-name exclusion limitation must be requalified if it changes"
    );
}

#[test]
fn php_names_trait_body_contexts_keep_self_this_and_parent_consumers_separate() {
    let (s, _) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php trait T {public function run(){self::save();$this->work();parent::base();static::save();}} class PA {public static function base(){}} class PB {public static function base(){}} class A extends PA {use T;private static function save(){} private function work(){}} class B extends PB {use T;private static function save(){} private function work(){}}",
    )]);
    for (expression, expected) in [
        ("self::save", [("A", "A::save"), ("B", "B::save")]),
        ("$this->work", [("A", "A::work"), ("B", "B::work")]),
        ("parent::base", [("A", "PA::base"), ("B", "PB::base")]),
    ] {
        let edge = s
            .calls
            .iter()
            .find(|e| e.reference.expression == expression)
            .unwrap();
        assert_eq!(edge.resolution, "ambiguous");
        let evidence = edge.context_evidence.as_ref().unwrap();
        assert!(evidence.enumeration_complete);
        assert!(!evidence.runtime_contexts_complete && !evidence.context_correlation_proven);
        assert_eq!(evidence.contexts.len(), 2);
        for (owner, target) in expected {
            let owner = s
                .symbols
                .iter()
                .find(|s| s.qualified_name == owner)
                .unwrap();
            let target = s
                .symbols
                .iter()
                .find(|s| s.qualified_name == target)
                .unwrap();
            let context = evidence
                .contexts
                .iter()
                .find(|c| c.owner == owner.id)
                .unwrap();
            assert_eq!(context.candidates, vec![target.id.clone()]);
        }
    }
    let late = s
        .calls
        .iter()
        .find(|e| e.reference.expression == "static::save")
        .unwrap();
    assert!(late.candidates.is_empty());
    assert!(late
        .context_evidence
        .as_ref()
        .unwrap()
        .contexts
        .iter()
        .all(|c| c.resolution_basis == "PHP_LATE_STATIC_BINDING_UNVERIFIED"));
}

#[test]
fn php_names_trait_body_alias_retains_consumer_but_override_and_inheritance_do_not_rebind_scope() {
    let (s, _) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php trait T {public function run(){self::save();}} trait Outer {use T;} class Kept {use Outer {Outer::run as alias;} public function run(){} public static function save(){}} class Dropped {use Outer;public function run(){} public static function save(){}} class Inherited extends Kept {public static function save(){}}",
    )]);
    let edge = s
        .calls
        .iter()
        .find(|e| e.reference.expression == "self::save")
        .unwrap();
    assert_eq!(targets(&s, "self::save"), vec!["Kept::save"]);
    assert_eq!(edge.resolution, "resolved_static");
    assert_eq!(edge.context_evidence.as_ref().unwrap().contexts.len(), 1);
}

#[test]
fn php_names_trait_body_single_inherited_host_keeps_lexical_base_and_trait_imports() {
    let (s, _) = graph(&[
        (
            "p",
            "/p/t.php",
            r"<?php namespace Lib; use Helper\Service as Alias; trait T {public function run(){self::save();Alias::send();$other->save();}}",
        ),
        (
            "p",
            "/p/a.php",
            r"<?php namespace App; use Lib\T; class Base {use T; public static function save(){}} class Child extends Base {public static function save(){}} class Alias {public static function send(){}}",
        ),
        (
            "p",
            "/p/h.php",
            r"<?php namespace Helper; class Service {public static function send(){}}",
        ),
        (
            "q",
            "/q/a.php",
            r"<?php namespace App; use Lib\T; class Foreign {use T;public static function save(){}}",
        ),
    ]);
    assert_eq!(targets(&s, "self::save"), vec!["App\\Base::save"]);
    assert_eq!(targets(&s, "Alias::send"), vec!["Helper\\Service::send"]);
    assert!(targets(&s, "<dynamic>->save").is_empty());
    assert_eq!(
        s.calls
            .iter()
            .find(|e| e.reference.expression == "Alias::send")
            .unwrap()
            .context_evidence
            .as_ref()
            .unwrap()
            .contexts
            .len(),
        1
    );
}

#[test]
fn php_names_trait_body_missing_context_results_and_first_class_values_are_explicit() {
    let (s, _) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php trait T {public function run(){self::save();$f=self::save(...);}} class A {use T;public static function save(){}} class B {use T;} trait Unused {public function run(){self::never();}}",
    )]);
    let edge = s
        .calls
        .iter()
        .find(|e| e.reference.expression == "self::save")
        .unwrap();
    assert_eq!(edge.resolution, "ambiguous");
    assert_eq!(edge.candidates.len(), 1);
    let contexts = &edge.context_evidence.as_ref().unwrap().contexts;
    assert_eq!(contexts.len(), 2);
    assert!(contexts.iter().any(|c| c.candidates.is_empty()));
    let value = s
        .bound_references
        .iter()
        .find(|e| e.reference.expression == "self::save" && !e.reference.is_call)
        .unwrap();
    assert_eq!(value.candidates, edge.candidates);
    assert!(value.context_evidence.is_some());
    let unused = s
        .calls
        .iter()
        .find(|e| e.reference.expression == "self::never")
        .unwrap();
    assert_eq!(
        unused.resolution_basis,
        "PHP_TRAIT_BODY_NO_CAPTURED_CONSUMERS"
    );
    assert!(unused
        .context_evidence
        .as_ref()
        .unwrap()
        .contexts
        .is_empty());
}

#[test]
fn php_names_trait_body_consumer_limit_is_not_a_complete_single_binding() {
    let mut body = String::from("<?php trait T {public function run(){self::save();}}");
    for i in 0..130 {
        body.push_str(&format!(
            "class C{i} {{use T;public static function save(){{}}}}"
        ));
    }
    let (s, _) = graph(&[("p", "/p/a.php", &body)]);
    let edge = s
        .calls
        .iter()
        .find(|e| e.reference.expression == "self::save")
        .unwrap();
    let context = edge.context_evidence.as_ref().unwrap();
    assert_eq!(context.contexts.len(), 128);
    assert!(context.truncated && !context.enumeration_complete);
    assert_eq!(edge.resolution, "ambiguous");
}

#[test]
fn php_names_trait_body_context_cache_roundtrip_validates_metadata_and_same_byte_type_values() {
    let (mut s, mut docs) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php class D {public static function save(){}} trait T {public function run(){self::save();D::save();$f=D::save(...);}} class A {use T;public static function save(){}}",
    )]);
    let mut foreign = parse(
        "foreign",
        "/q/foreign.php",
        "<?php class Foreign {public static function save(){}}",
        1,
    )
    .unwrap();
    foreign.project = Some("q".into());
    let foreign_owner = foreign
        .symbols
        .iter()
        .find(|s| s.qualified_name == "Foreign")
        .unwrap()
        .id
        .clone();
    let foreign_method = foreign
        .symbols
        .iter()
        .find(|s| s.qualified_name == "Foreign::save")
        .unwrap()
        .id
        .clone();
    s.symbols.extend(foreign.symbols.clone());
    docs.insert("foreign".into(), foreign);
    let cold = super::super::resolution_cache::resolve(&s.symbols, &docs, None, false).unwrap();
    let warm = super::super::resolution_cache::resolve(&s.symbols, &docs, Some(&cold.cache), false)
        .unwrap();
    assert_eq!(warm.stats.invalid_cache_entries, 0);
    assert_eq!(warm.stats.reference_sites_reused, s.references.len());
    assert_eq!(
        serde_json::to_value(&warm.edges).unwrap(),
        serde_json::to_value(&cold.edges).unwrap()
    );
    for alteration in [
        "missing",
        "runtime",
        "owner",
        "union",
        "foreign-owner",
        "foreign-target",
    ] {
        let mut cache = cold.cache.clone();
        let entries = cache["entries"].as_object_mut().unwrap();
        let entry = entries.get_mut("0").unwrap();
        let edges = entry["edges"].as_array_mut().unwrap();
        let edge = edges
            .iter_mut()
            .find(|e| e["context_evidence"].is_object())
            .unwrap();
        match alteration {
            "missing" => {
                edge.as_object_mut().unwrap().remove("context_evidence");
            }
            "runtime" => {
                edge["context_evidence"]["runtime_contexts_complete"] = serde_json::json!(true)
            }
            "owner" => {
                edge["context_evidence"]["contexts"][0]["owner"] = serde_json::json!("missing")
            }
            "union" => edge["candidates"] = serde_json::json!([]),
            "foreign-owner" => {
                edge["context_evidence"]["contexts"][0]["owner"] = serde_json::json!(foreign_owner)
            }
            "foreign-target" => {
                edge["context_evidence"]["contexts"][0]["candidates"] =
                    serde_json::json!([foreign_method]);
                edge["candidates"] = serde_json::json!([foreign_method]);
            }
            _ => unreachable!(),
        }
        let checksum = content_hash(&serde_json::to_string(&entry["edges"]).unwrap());
        entry["checksum"] = serde_json::json!(checksum);
        let fixed = super::super::resolution_cache::resolve(&s.symbols, &docs, Some(&cache), false)
            .unwrap();
        assert_eq!(fixed.stats.invalid_cache_entries, 1, "{alteration}");
        assert_eq!(
            serde_json::to_value(&fixed.edges).unwrap(),
            serde_json::to_value(&cold.edges).unwrap()
        );
    }
}

#[test]
fn php_names_trait_body_frozen_owner_target_swap_is_visible_with_unchanged_union() {
    use crate::models::Document;
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("contexts.duckdb")).unwrap();
    let doc = |body: &str| Document {
        id: "app".into(),
        uri: "file:///p/a.php".into(),
        source_file: Some("/p/a.php".into()),
        content: body.into(),
        wing: Some("p".into()),
        ..Default::default()
    };
    let body="<?php trait T {public function run(){self::save();}} class P1 {public static function save(){}} class P2 {public static function save(){}} class A extends P1 {use T;} class B extends P2 {use T;}";
    store.upsert_document(&doc(body)).unwrap();
    let before = store.index_code().unwrap().index_revision.unwrap();
    let payload = crate::code_index::test_get_code_doc_payload(&store, "app");
    let mut legacy: serde_json::Value = serde_json::from_str(&payload).unwrap();
    legacy["php_names"]["format"] = serde_json::json!(3);
    crate::code_index::test_set_code_doc_payload(&store, "app", &legacy.to_string());
    assert!(store
        .code_snapshot()
        .unwrap()
        .stale_documents
        .contains(&"app".into()));
    assert_eq!(store.index_code().unwrap().indexed, 1);
    let changed = body
        .replace("A extends P1", "A extends P2")
        .replace("B extends P2", "B extends P1");
    store.upsert_document(&doc(&changed)).unwrap();
    let after = store.index_code().unwrap().index_revision.unwrap();
    let q = history::DiffQuery {
        before,
        after: Some(after),
        project: Some("p".into()),
        limit: None,
    };
    let delta = store.code_index_diff(&q).unwrap();
    assert_eq!(delta["edges_added_total"], 0);
    assert_eq!(delta["edges_removed_total"], 0);
    assert_eq!(delta["php_trait_context_comparison"]["available"], true);
    for key in ["relations_added", "relations_removed"] {
        assert_eq!(
            delta[key]
                .as_array()
                .unwrap()
                .iter()
                .filter(|r| r["kind"] == "php_trait_body_context_candidate")
                .count(),
            2
        );
    }
    let graph = crate::code_index::test_get_rev_graph_json(&store, before);
    let mut legacy: serde_json::Value = serde_json::from_str(&graph).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("php_trait_contexts_format");
    crate::code_index::test_set_rev_graph_json(&store, before, &legacy.to_string());
    let delta = store.code_index_diff(&q).unwrap();
    assert_eq!(delta["php_trait_context_comparison"]["available"], false);
    for key in ["relations_added", "relations_removed"] {
        assert!(delta[key]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["kind"] != "php_trait_body_context_candidate"));
    }
}

fn affected(
    s: &CodeSnapshot,
    target: &str,
) -> (std::collections::BTreeSet<String>, serde_json::Value) {
    let id = &s
        .symbols
        .iter()
        .find(|s| s.qualified_name == target)
        .unwrap()
        .id;
    let q = CodeQuery {
        symbol: Some(id.clone()),
        depth: Some(10),
        limit: Some(500),
        ..Default::default()
    };
    let result = s.query("impact", &q);
    let names = result["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            s.symbols
                .iter()
                .find(|s| s.id == row["symbol_id"].as_str().unwrap())
                .unwrap()
                .qualified_name
                .clone()
        })
        .collect();
    (names, result)
}
#[test]
fn php_names_trait_path_impact_prunes_incompatible_entry_consumers() {
    let (s, _) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php trait T {public function run(){self::save();}} class A {use T;private static function save(){} public function entry(){$this->run();}} class B {use T;private static function save(){} public function entry(){$this->run();}}",
    )]);
    let (names, result) = affected(&s, "A::save");
    assert_eq!(
        names,
        std::collections::BTreeSet::from(["T::run".into(), "A::entry".into()])
    );
    assert!(
        result["trait_context_traversal"]["transitions_pruned"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(
        result["trait_context_traversal"]["metadata_incomplete"],
        false
    );
    assert_eq!(result["traversal_complete"], true);
    let entry = s
        .calls
        .iter()
        .find(|e| {
            s.symbols.iter().any(|s| {
                Some(&s.id) == e.reference.caller.as_ref() && s.qualified_name == "A::entry"
            })
        })
        .unwrap();
    let metadata = entry.context_evidence.as_ref().unwrap();
    assert_eq!(metadata.kind, "php_captured_trait_entry");
    assert_eq!(
        metadata.contexts[0].target_owner.as_deref(),
        Some(metadata.contexts[0].owner.as_str())
    );
}
#[test]
fn php_names_trait_path_impact_visits_all_context_states_with_one_symbol_witness() {
    let (s, _) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php trait T {public function run(){self::save();}} class Root {public static function save(){}} class A extends Root {use T;public function entry(){$this->run();}} class B extends Root {use T;public function entry(){$this->run();}}",
    )]);
    let (names, result) = affected(&s, "Root::save");
    assert_eq!(
        names,
        std::collections::BTreeSet::from(["T::run".into(), "A::entry".into(), "B::entry".into()])
    );
    assert_eq!(result["items"].as_array().unwrap().len(), 3);
    assert!(
        result["trait_context_traversal"]["states_visited"]
            .as_u64()
            .unwrap()
            > 4
    );
}
#[test]
fn php_names_trait_path_nested_body_to_inherited_trait_body_maps_two_lexical_frames() {
    let (s, _) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php trait Inner {public function inner(){self::leaf();}} trait Outer {public function outer(){$this->inner();}} class BaseA {use Inner;public static function leaf(){}} class BaseB {use Inner;public static function leaf(){}} class A extends BaseA {use Outer;public function entry(){$this->outer();}} class B extends BaseB {use Outer;public function entry(){$this->outer();}}",
    )]);
    let (names, result) = affected(&s, "BaseA::leaf");
    assert_eq!(
        names,
        std::collections::BTreeSet::from([
            "Inner::inner".into(),
            "Outer::outer".into(),
            "A::entry".into()
        ])
    );
    assert_eq!(
        result["trait_context_traversal"]["metadata_incomplete"],
        false
    );
    let edge = s
        .calls
        .iter()
        .find(|e| e.reference.expression == "$this->inner")
        .unwrap();
    let row = edge
        .context_evidence
        .as_ref()
        .unwrap()
        .contexts
        .iter()
        .find(|c| {
            s.symbols
                .iter()
                .any(|s| s.id == c.owner && s.qualified_name == "A")
        })
        .unwrap();
    assert_eq!(
        s.symbols
            .iter()
            .find(|s| Some(&s.id) == row.target_owner.as_ref())
            .unwrap()
            .qualified_name,
        "BaseA"
    );
}
#[test]
fn php_names_trait_path_missing_entry_metadata_fails_closed_and_reports_incompleteness() {
    let (mut s, _) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php trait T {public function run(){self::save();}} class A {use T;public static function save(){} public function entry(){$this->run();}}",
    )]);
    for edge in &mut s.calls {
        if edge
            .context_evidence
            .as_ref()
            .is_some_and(|e| e.kind == "php_captured_trait_entry")
        {
            edge.context_evidence = None;
        }
    }
    let (names, result) = affected(&s, "A::save");
    assert_eq!(names, std::collections::BTreeSet::from(["T::run".into()]));
    assert_eq!(
        result["trait_context_traversal"]["metadata_incomplete"],
        true
    );
    assert_eq!(result["traversal_complete"], false);
}

#[test]
fn php_names_trait_path_callable_values_keep_contexts_without_becoming_calls() {
    let (s, _) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php trait T {public function run(){self::save();}} class A {use T;public static function save(){} public function entry(){$f=$this->run(...);}} class B {use T;public static function save(){} public function entry(){$f=$this->run(...);}}",
    )]);
    let (names, _) = affected(&s, "A::save");
    assert_eq!(names, std::collections::BTreeSet::from(["T::run".into()]));
    let target = s
        .symbols
        .iter()
        .find(|s| s.qualified_name == "A::save")
        .unwrap();
    for mode in ["references", "all"] {
        let q = CodeQuery {
            symbol: Some(target.id.clone()),
            impact_relations: Some(mode.into()),
            depth: Some(10),
            ..Default::default()
        };
        let result = s.query("impact", &q);
        let names: std::collections::BTreeSet<_> = result["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                s.symbols
                    .iter()
                    .find(|s| s.id == r["symbol_id"].as_str().unwrap())
                    .unwrap()
                    .qualified_name
                    .as_str()
            })
            .collect();
        assert_eq!(
            names,
            std::collections::BTreeSet::from(["T::run", "A::entry"])
        );
    }
}
#[test]
fn php_names_trait_path_entry_metadata_cache_missing_frame_repairs_without_new_calls() {
    let (s, docs) = graph(&[(
        "p",
        "/p/a.php",
        r"<?php trait T {public function run(){}} class A {use T;public function entry(){$this->run();$f=$this->run(...);}}",
    )]);
    let cold = super::super::resolution_cache::resolve(&s.symbols, &docs, None, false).unwrap();
    let warm = super::super::resolution_cache::resolve(&s.symbols, &docs, Some(&cold.cache), false)
        .unwrap();
    assert_eq!(warm.stats.invalid_cache_entries, 0);
    assert_eq!(warm.stats.reference_sites_reused, s.references.len());
    for alteration in ["frame", "kind", "metadata"] {
        let mut cache = cold.cache.clone();
        let entry = &mut cache["entries"]["0"];
        let edge = entry["edges"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["context_evidence"]["kind"] == "php_captured_trait_entry")
            .unwrap();
        match alteration {
            "frame" => {
                edge["context_evidence"]["contexts"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("target_owner");
            }
            "kind" => {
                edge["context_evidence"]["kind"] = serde_json::json!("php_captured_trait_consumers")
            }
            "metadata" => {
                edge.as_object_mut().unwrap().remove("context_evidence");
            }
            _ => unreachable!(),
        }
        entry["checksum"] = serde_json::json!(content_hash(
            &serde_json::to_string(&entry["edges"]).unwrap()
        ));
        let fixed = super::super::resolution_cache::resolve(&s.symbols, &docs, Some(&cache), false)
            .unwrap();
        assert_eq!(fixed.stats.invalid_cache_entries, 1, "{alteration}");
        assert_eq!(
            serde_json::to_value(&fixed.edges).unwrap(),
            serde_json::to_value(&cold.edges).unwrap()
        );
    }
}
#[test]
fn php_names_trait_path_frozen_entry_frames_have_separate_legacy_capability_guard() {
    use crate::models::Document;
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("path-contexts.duckdb")).unwrap();
    let doc = |body: &str| Document {
        id: "app".into(),
        uri: "file:///p/a.php".into(),
        source_file: Some("/p/a.php".into()),
        content: body.into(),
        wing: Some("p".into()),
        ..Default::default()
    };
    let body="<?php trait T {public function run(){}} class A {use T;public function entry(){$this->run();}}";
    store.upsert_document(&doc(body)).unwrap();
    let before = store.index_code().unwrap().index_revision.unwrap();
    store
        .upsert_document(&doc(&body.replace("use T;", "")))
        .unwrap();
    let after = store.index_code().unwrap().index_revision.unwrap();
    let q = history::DiffQuery {
        before,
        after: Some(after),
        project: Some("p".into()),
        limit: None,
    };
    let delta = store.code_index_diff(&q).unwrap();
    assert_eq!(delta["php_trait_path_comparison"]["available"], true);
    let entry = delta["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["kind"] == "php_trait_entry_context_candidate")
        .unwrap();
    assert!(entry["target_owner"].as_str().is_some());
    let graph = crate::code_index::test_get_rev_graph_json(&store, before);
    let mut legacy: serde_json::Value = serde_json::from_str(&graph).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("php_trait_paths_format");
    crate::code_index::test_set_rev_graph_json(&store, before, &legacy.to_string());
    let legacy = store.code_index_diff(&q).unwrap();
    assert_eq!(legacy["php_trait_path_comparison"]["available"], false);
    assert!(legacy["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["kind"] != "php_trait_entry_context_candidate"));
}

#[test]
fn phpunit_candidates_require_framework_base_and_public_instance_methods() {
    let (_, docs) = graph(&[(
        "p",
        "/p/tests.php",
        r"<?php
namespace Suite;
use PHPUnit\Framework\TestCase as Base;
use PHPUnit\Framework\Attributes\Test as Check;
abstract class AbstractTest extends Base { public function testAbstract(){} }
class Good extends AbstractTest {
 public function testWorks(){} #[Check] public function works(){}
 private function testHidden(){} public static function testStatic(){}
 #[Unrelated] public function unrelated(){} public function helper(){}
}
class Fake {public function testWrong(){} #[Check] public function wrong(){}}
function testLoose(){}
",
    )]);
    let doc = docs.values().next().unwrap();
    let entries = test_entrypoints(&docs, doc);
    let mut names: Vec<_> = entries
        .iter()
        .map(|e| {
            doc.symbols
                .iter()
                .find(|s| Some(&s.id) == e.symbol_id.as_ref())
                .unwrap()
                .qualified_name
                .as_str()
        })
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "Suite\\AbstractTest::testAbstract",
            "Suite\\Good::testWorks",
            "Suite\\Good::works"
        ]
    );
    assert!(entries.iter().all(|e| doc
        .symbols
        .iter()
        .find(|s| Some(&s.id) == e.test_context.as_ref().map(|c| &c.host_symbol_id))
        .unwrap()
        .qualified_name
        == "Suite\\Good"));
    assert!(entries
        .iter()
        .all(|e| e.kind == "phpunit_test_candidate" && e.content_hash == doc.hash));
    assert!(entries
        .iter()
        .any(|e| e.reference_kind.as_deref() == Some("phpunit_test_attribute_candidate")));
}

#[test]
fn phpunit_candidates_reject_cycles_foreign_projects_and_legacy_capture() {
    let (_, mut docs) = graph(&[
        (
            "p",
            "/p/test.php",
            r"<?php class Good extends \PHPUnit\Framework\TestCase {#[\PHPUnit\Framework\Attributes\Test] public function works(){}} class Cycle extends Loop {public function testNo(){}} class Loop extends Cycle {} class Foreign extends Other {public function testNo(){}}",
        ),
        (
            "q",
            "/q/base.php",
            r"<?php class Other extends \PHPUnit\Framework\TestCase {}",
        ),
    ]);
    assert_eq!(test_entrypoints(&docs, &docs["0"]).len(), 1);
    docs.get_mut("0")
        .unwrap()
        .php_names
        .as_mut()
        .unwrap()
        .format = 5;
    assert!(test_entrypoints(&docs, &docs["0"]).is_empty());
}

#[test]
fn phpunit_candidates_frozen_history_guards_legacy_enrichment() {
    use crate::models::Document;
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("tests.duckdb")).unwrap();
    let doc = |body: &str| Document {
        id: "tests".into(),
        uri: "file:///p/tests.php".into(),
        source_file: Some("/p/tests.php".into()),
        content: body.into(),
        wing: Some("p".into()),
        ..Default::default()
    };
    store.upsert_document(&doc("<?php class Tests extends \\PHPUnit\\Framework\\TestCase {public function testWorks(){}}" )).unwrap();
    let before = store.index_code().unwrap().index_revision.unwrap();
    store
        .upsert_document(&doc("<?php class Tests {public function testWorks(){}}"))
        .unwrap();
    let after = store.index_code().unwrap().index_revision.unwrap();
    let q = history::DiffQuery {
        before,
        after: Some(after),
        project: Some("p".into()),
        limit: None,
    };
    let delta = store.code_index_diff(&q).unwrap();
    assert_eq!(delta["phpunit_test_comparison"]["available"], true);
    assert!(delta["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["entrypoint_kind"] == "phpunit_test_candidate"));
    let graph = crate::code_index::test_get_rev_graph_json(&store, before);
    let mut legacy: serde_json::Value = serde_json::from_str(&graph).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("phpunit_tests_format");
    crate::code_index::test_set_rev_graph_json(&store, before, &legacy.to_string());
    let delta = store.code_index_diff(&q).unwrap();
    assert_eq!(delta["phpunit_test_comparison"]["available"], false);
    assert!(delta["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["entrypoint_kind"] != "phpunit_test_candidate"));
}

#[test]
fn phpunit_effective_candidates_preserve_hosts_aliases_precedence_and_body_sources() {
    let (snapshot, docs) = graph(&[
        (
            "p",
            "/p/base.php",
            r"<?php trait Checks {public function testTrait(){}} abstract class Base extends \PHPUnit\Framework\TestCase {public function testInherited(){}}",
        ),
        (
            "p",
            "/p/tests.php",
            r"<?php class A extends Base {use Checks {testTrait as testAlias;}} class B extends Base {use Checks;public function testInherited(){}}",
        ),
    ]);
    let entries = analysis::entrypoints(&snapshot.symbols, &docs);
    let names: HashMap<_, _> = snapshot
        .symbols
        .iter()
        .map(|s| (s.id.as_str(), s.qualified_name.as_str()))
        .collect();
    let mut cases: Vec<_> = entries
        .iter()
        .filter_map(|e| {
            e.test_context.as_ref().map(|c| {
                (
                    names[c.host_symbol_id.as_str()],
                    c.effective_method.as_str(),
                    names[e.symbol_id.as_deref().unwrap()],
                    c.origin.as_str(),
                    names[c.lexical_owner_symbol_id.as_str()],
                )
            })
        })
        .collect();
    cases.sort();
    assert_eq!(
        cases,
        vec![
            ("A", "testAlias", "Checks::testTrait", "trait", "A"),
            (
                "A",
                "testInherited",
                "Base::testInherited",
                "inherited_declared",
                "Base"
            ),
            ("A", "testTrait", "Checks::testTrait", "trait", "A"),
            ("B", "testInherited", "B::testInherited", "declared", "B"),
            ("B", "testTrait", "Checks::testTrait", "trait", "B")
        ]
    );
    let inherited = entries
        .iter()
        .find(|e| {
            e.test_context
                .as_ref()
                .is_some_and(|c| c.origin == "inherited_declared")
        })
        .unwrap();
    assert_eq!(inherited.path, "/p/base.php");
    assert_eq!(
        inherited.test_context.as_ref().unwrap().host_path,
        "/p/tests.php"
    );
    assert_ne!(
        inherited.document_id,
        inherited.test_context.as_ref().unwrap().host_document_id
    );
}

#[test]
fn phpunit_effective_candidates_reject_conflicts_visibility_cycles_and_unrelated_roots() {
    let (snapshot, docs) = graph(&[(
        "p",
        "/p/tests.php",
        r"<?php trait X {public function testX(){}} trait Y {public function testX(){}} trait Cycle {use Cycle;} class Conflict extends \PHPUnit\Framework\TestCase {use X,Y;} class Hidden extends \PHPUnit\Framework\TestCase {use X {testX as private;}} class Recursive extends \PHPUnit\Framework\TestCase {use Cycle;} class Unrelated {use X;} class Alias extends \PHPUnit\Framework\TestCase {use X {testX as private;testX as public testAlias;}}",
    )]);
    let entries = analysis::entrypoints(&snapshot.symbols, &docs);
    let candidates: Vec<_> = entries
        .iter()
        .filter_map(|e| e.test_context.as_ref())
        .collect();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].effective_method, "testAlias");
    assert_eq!(candidates[0].origin, "trait");
}

#[test]
fn phpunit_effective_frozen_history_normalizes_hosts_and_guards_old_invocation_format() {
    use crate::models::Document;
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("effective.duckdb")).unwrap();
    let body = r"<?php trait Tests {public function testWorks(){}} class A extends \PHPUnit\Framework\TestCase {use Tests;} class B extends \PHPUnit\Framework\TestCase {use Tests;}";
    let doc = |body: &str| Document {
        id: "tests".into(),
        uri: "file:///p/tests.php".into(),
        source_file: Some("/p/tests.php".into()),
        content: body.into(),
        wing: Some("p".into()),
        ..Default::default()
    };
    store.upsert_document(&doc(body)).unwrap();
    let before = store.index_code().unwrap().index_revision.unwrap();
    store
        .upsert_document(&doc(&body.replacen(
            "use Tests;",
            "use Tests {testWorks as testAlias;}",
            1,
        )))
        .unwrap();
    let after = store.index_code().unwrap().index_revision.unwrap();
    let q = history::DiffQuery {
        before,
        after: Some(after),
        project: Some("p".into()),
        limit: None,
    };
    let delta = store.code_index_diff(&q).unwrap();
    let added: Vec<_> = delta["relations_added"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["entrypoint_kind"] == "phpunit_test_candidate")
        .collect();
    assert_eq!(added.len(), 1, "{delta}");
    assert_eq!(added[0]["test_context"]["effective_method"], "testAlias");
    assert!(added[0]["test_context"]["host"]
        .as_str()
        .unwrap()
        .contains("A"));
    assert!(delta["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["entrypoint_kind"] != "phpunit_test_candidate"));
    let graph = crate::code_index::test_get_rev_graph_json(&store, before);
    let mut legacy: serde_json::Value = serde_json::from_str(&graph).unwrap();
    legacy["phpunit_tests_format"] = serde_json::json!(1);
    crate::code_index::test_set_rev_graph_json(&store, before, &legacy.to_string());
    let delta = store.code_index_diff(&q).unwrap();
    assert_eq!(delta["phpunit_test_comparison"]["available"], false);
    assert!(delta["relations_added"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["entrypoint_kind"] != "phpunit_test_candidate"));
}

#[test]
fn phpunit_effective_project_snapshot_excludes_foreign_and_stale_hosts() {
    use crate::models::Document;
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("scoped.duckdb")).unwrap();
    let put = |id: &str, project: &str, path: &str, body: &str| {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("file://{path}"),
                source_file: Some(path.into()),
                content: body.into(),
                wing: Some(project.into()),
                ..Default::default()
            })
            .unwrap()
    };
    put(
        "base",
        "p",
        "/p/base.php",
        r"<?php trait Checks {public function testWorks(){}} abstract class Base extends \PHPUnit\Framework\TestCase {public function testBase(){}} ",
    );
    put(
        "host",
        "p",
        "/p/host.php",
        "<?php class Host extends Base {use Checks;}",
    );
    put(
        "foreign",
        "q",
        "/q/foreign.php",
        r"<?php class Foreign extends \PHPUnit\Framework\TestCase {public function testForeign(){}}",
    );
    store.index_code().unwrap();
    let scoped = store.code_snapshot_in_project(Some("p")).unwrap();
    let query: analysis::EntryPointsQuery =
        serde_json::from_value(serde_json::json!({"project":"p","path":"/p/host.php","limit":20}))
            .unwrap();
    assert_eq!(query.project.as_deref(), Some("p"));
    let result = scoped.query("entrypoints", &query.query);
    assert_eq!(result["items"].as_array().unwrap().len(), 2, "{result}");
    put(
        "host",
        "p",
        "/p/host.php",
        "<?php class Host extends Base {}",
    );
    let stale = store.code_snapshot_in_project(Some("p")).unwrap();
    assert!(stale.query("entrypoints", &CodeQuery::default())["items"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        store
            .code_snapshot_in_project(Some("q"))
            .unwrap()
            .query("entrypoints", &CodeQuery::default())["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
