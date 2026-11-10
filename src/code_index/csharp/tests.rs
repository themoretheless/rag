use super::*;
struct TestSnapshot {
    symbols: Vec<Symbol>,
    calls: Vec<CallEdge>,
    documents: HashMap<String, IndexedDocument>,
}
fn snapshot(files: &[(&str, &str, &str)]) -> TestSnapshot {
    let mut documents = HashMap::new();
    for (i, (path, project, source)) in files.iter().enumerate() {
        let id = format!("d{i}");
        let mut doc = parse(&id, path, source, 1).unwrap();
        doc.project = Some((*project).into());
        assert!(
            !doc.parse_errors,
            "{}: {:?}",
            path,
            doc.symbols.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        documents.insert(id, doc);
    }
    let mut symbols: Vec<_> = documents.values().flat_map(|d| d.symbols.clone()).collect();
    symbols.sort_by(|a, b| a.id.cmp(&b.id));
    let references = resolve::references(&symbols, &documents);
    let calls = references
        .iter()
        .filter(|e| e.reference.is_call)
        .cloned()
        .collect();
    TestSnapshot {
        symbols,
        calls,
        documents,
    }
}
fn candidates(s: &TestSnapshot, caller: &str, expression: &str) -> Vec<String> {
    let owner = s
        .symbols
        .iter()
        .find(|s| s.qualified_name == caller)
        .unwrap();
    let edges: Vec<_> = s
        .calls
        .iter()
        .filter(|e| {
            e.reference.caller.as_deref() == Some(&owner.id) && e.reference.expression == expression
        })
        .collect();
    assert_eq!(edges.len(), 1, "{caller} {expression}: {:?}", s.calls);
    edges[0]
        .candidates
        .iter()
        .map(|id| {
            s.symbols
                .iter()
                .find(|s| s.id == *id)
                .unwrap()
                .qualified_name
                .clone()
        })
        .collect()
}
#[test]
fn csharp_declarations_cover_reference_profile_and_exact_unicode_spans() {
    let source="// café\nnamespace N { interface I { void Run(); } struct S {} record R(int X); record struct RS(int X); enum E { A } class C { int field=42; public int P { get { return 1; } } C() {} ~C() {} public static C operator +(C a,C b)=>a; void Run() { void Local() {} Local(); } } }";
    let s = snapshot(&[("/p/all.cs", "p", source)]);
    for expected in [
        "N",
        "N.I",
        "N.I.Run",
        "N.S",
        "N.R",
        "N.RS",
        "N.E",
        "N.C",
        "N.C.field",
        "N.C.P",
        "N.C.C",
        "N.C.~C",
        "N.C.operator +",
        "N.C.Local",
    ] {
        assert!(
            s.symbols.iter().any(|s| s.qualified_name == expected),
            "missing {expected}"
        );
    }
    let field = s
        .symbols
        .iter()
        .find(|s| s.qualified_name == "N.C.field")
        .unwrap();
    assert!(!analysis::callable(field));
    assert_eq!(field.signature, "field");
    let local = s
        .symbols
        .iter()
        .find(|s| s.qualified_name == "N.C.Local")
        .unwrap();
    assert_eq!(&source[local.start_byte..local.end_byte], "void Local() {}");
    assert_eq!(candidates(&s, "N.C.Run", "Local"), ["N.C.Local"]);
}
#[test]
fn csharp_namespace_usings_aliases_and_arity_preserve_ambiguity() {
    let s=snapshot(&[("/p/a.cs","p","namespace A; public class C { public static void Run(){} public static void Run(int x){} public static void Run(string x){} }"),("/p/b.cs","p","using Alias = A.C; namespace B; class MainClass { static void Main(){ Alias.Run(); Alias.Run(1); } }")]);
    let mut calls: Vec<_> = s
        .calls
        .iter()
        .filter(|e| e.reference.expression == "Alias.Run")
        .collect();
    calls.sort_by_key(|e| e.reference.start_byte);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].candidates.len(), 1);
    assert_eq!(calls[1].candidates.len(), 2);
    assert_eq!(calls[1].resolution, "ambiguous");
}
#[test]
fn csharp_type_receivers_inherited_members_and_project_scopes() {
    let s=snapshot(&[("/p/base.cs","p","namespace Lib; class Base { public void Work(){} public static void Api(){} }"),("/p/client.cs","p","using Lib; namespace App; class Child : Base { void Run(Base b) { this.Work(); base.Work(); b.Work(); Base.Api(); } }"),("/foreign/base.cs","other","namespace Lib; class Base { public void Work(){} }")]);
    for expr in ["this.Work", "base.Work", "b.Work"] {
        assert_eq!(candidates(&s, "App.Child.Run", expr), ["Lib.Base.Work"]);
    }
    assert_eq!(
        candidates(&s, "App.Child.Run", "Base.Api"),
        ["Lib.Base.Api"]
    );
    let edges = edges(
        &s.documents,
        s.documents
            .values()
            .find(|d| d.path == "/p/client.cs")
            .unwrap(),
    );
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].candidates.len(), 1);
    assert_ne!(edges[0].hierarchy_status, "complete_static");
}
#[test]
fn csharp_unknown_shadowed_mutated_optional_generic_and_conditional_calls_fail_closed() {
    let s=snapshot(&[("/p/a.cs","p","class C { public static void Api() {} public void Work(){} void Optional(int x=1){} void Generic<T>(){} void Run(C c, System.Action Api) { c = other; c.Work(); Api(); Optional(); Generic<int>(); System.Action f=()=>c.Work(); } }")]);
    let run = s
        .symbols
        .iter()
        .find(|s| s.qualified_name == "C.Run")
        .unwrap();
    assert!(
        s.calls
            .iter()
            .filter(|e| e.reference.caller.as_deref() == Some(&run.id))
            .all(|e| e.candidates.is_empty()),
        "{:?}",
        s.calls
    );
    let c=snapshot(&[("/p/config.cs","p","class C { static void Api() {} static void Main() { Api(); } }\n#if DEBUG\nclass D {}\n#endif")]);
    assert!(c.calls.iter().all(|e| e.candidates.is_empty()));
}
#[test]
fn csharp_property_field_and_local_namespace_shadows_do_not_become_static_calls() {
    let s=snapshot(&[("/p/a.cs","p","class Target { public static void Api(){} } class C { System.Action Target; void Run(){ Target.Api(); } void Local(int Target){Target.Api();} }")]);
    assert!(s.calls.iter().all(|e| e.candidates.is_empty()));
}
#[test]
fn csharp_unknown_receivers_do_not_copy_argument_bodies_and_nested_calls_remain() {
    let s=snapshot(&[("/p/a.cs","p","class C { C Factory(string text)=>this; void Work(){} void Run(){ Factory(\"PRIVATE_ARGUMENT\").Work(); } }")]);
    assert!(!serde_json::to_string(&s.calls)
        .unwrap()
        .contains("PRIVATE_ARGUMENT"));
    let facts = &s.documents.values().next().unwrap().csharp;
    assert!(!serde_json::to_string(facts)
        .unwrap()
        .contains("PRIVATE_ARGUMENT"));
    assert_eq!(candidates(&s, "C.Run", "Factory"), ["C.Factory"]);
    assert!(candidates(&s, "C.Run", "<receiver>.Work").is_empty());
}
#[test]
fn csharp_namespace_relative_imports_precede_outer_types_and_absolute_aliases() {
    let s=snapshot(&[("/p/a.cs","p","class C { public static void Ping(){} } namespace N { namespace Sub { class C { public static void Ping(){} } } namespace App { using N.Sub; using Alias = global::C; class Entry { static void Main(){ C.Ping(); Alias.Ping(); } } } }")]);
    assert_eq!(
        candidates(&s, "N.App.Entry.Main", "C.Ping"),
        ["N.Sub.C.Ping"]
    );
    assert_eq!(candidates(&s, "N.App.Entry.Main", "Alias.Ping"), ["C.Ping"]);
    let s=snapshot(&[("/p/a.cs","p","namespace N { namespace Sub { class C { public static void Ping(){} } } namespace App { using Sub; class Entry { static void Main(){ C.Ping(); } } } }")]);
    assert_eq!(
        candidates(&s, "N.App.Entry.Main", "C.Ping"),
        ["N.Sub.C.Ping"]
    );
}
#[test]
fn csharp_duplicate_types_and_using_collisions_remain_unresolved_or_ambiguous() {
    let s=snapshot(&[("/p/a.cs","p","namespace A { class C { public static void Ping(){} } } namespace B { class C { public static void Ping(){} } }"),("/p/main.cs","p","using A; using B; class Entry { static void Main(){C.Ping();} }")]);
    assert!(candidates(&s, "Entry.Main", "C.Ping").is_empty());
    let s = snapshot(&[
        ("/p/a.cs", "p", "partial class C { static void Ping(){} }"),
        (
            "/p/b.cs",
            "p",
            "partial class C { static void Main(){Ping();} }",
        ),
    ]);
    assert!(candidates(&s, "C.Main", "Ping").is_empty());
}
#[test]
fn csharp_global_using_and_explicit_constructor_links_are_captured() {
    let s = snapshot(&[
        ("/p/usings.cs", "p", "global using Lib;"),
        (
            "/p/lib.cs",
            "p",
            "namespace Lib; class C { public C(){} public void Work(){} }",
        ),
        (
            "/p/main.cs",
            "p",
            "class Entry { static void Main(){ C c = new C(); c.Work(); } }",
        ),
    ]);
    assert_eq!(candidates(&s, "Entry.Main", "C"), ["Lib.C.C"]);
    assert_eq!(candidates(&s, "Entry.Main", "c.Work"), ["Lib.C.Work"]);
}
#[test]
fn csharp_store_freshness_history_body_and_full_resolution_equivalence() {
    use crate::models::Document;
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("csharp.duckdb")).unwrap();
    let doc = |id: &str, path: &str, source: &str| Document {
        id: id.into(),
        uri: format!("file://{id}"),
        source_file: Some(path.into()),
        wing: Some("p".into()),
        title: path.into(),
        content: source.into(),
        ..Default::default()
    };
    let base = doc(
        "base",
        "/p/base.cs",
        "namespace Lib; class Base { public void Work(){ string q=\"SELECT * FROM users\"; } }",
    );
    let mut client = doc(
        "client",
        "/p/client.cs",
        "using Lib; class Child : Base { void Run(){Work();} }",
    );
    store.documents().upsert_document(&base).unwrap();
    store.documents().upsert_document(&client).unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .index_code()
            .unwrap()
            .indexed,
        2
    );
    let q = CodeQuery {
        symbol: Some("Lib.Base.Work".into()),
        ..Default::default()
    };
    let before = crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap()
        .query("callers", &q);
    assert_eq!(before["items"].as_array().unwrap().len(), 1, "{before}");
    assert!(crate::code_index::CodeIndex::new(&store)
        .code_body(&q)
        .unwrap()
        .to_string()
        .contains("SELECT * FROM users"));
    assert!(!before.to_string().contains("SELECT * FROM users"));
    let full = crate::code_index::CodeIndex::new(&store)
        .index_code_with_options(&resolution_cache::IndexOptions {
            force_full_resolution: true,
        })
        .unwrap();
    assert!(!full.revision_created);
    assert_eq!(full.resolution.unwrap().equivalent_to_existing, Some(true));
    client.content = "class Child { void Run(){} }".into();
    store.documents().upsert_document(&client).unwrap();
    assert!(crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap()
        .query("callers", &q)["items"]
        .as_array()
        .unwrap()
        .is_empty());
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let query = history::DiffQuery {
        before: 1,
        after: Some(2),
        project: Some("p".into()),
        limit: None,
    };
    let diff = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&query)
        .unwrap();
    assert_eq!(diff["csharp_relations_available"], true);
    assert!(
        diff["relations_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["relation"] == "csharp_base_type_candidate"),
        "{diff}"
    );
    client.content = "class Later {}".into();
    store.documents().upsert_document(&client).unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_index_diff(&query)
            .unwrap(),
        diff
    );
    let conn = store.lock().unwrap();
    let raw: String = conn
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
        .remove("csharp_syntax_format");
    conn.execute(
        "UPDATE code_index_revisions SET graph_json=? WHERE id=1",
        [graph.to_string()],
    )
    .unwrap();
    drop(conn);
    let legacy = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&query)
        .unwrap();
    assert_eq!(legacy["csharp_relations_available"], false);
    assert!(!legacy["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["relation"] == "csharp_base_type_candidate"));
}
#[test]
fn csharp_generic_and_unsupported_binding_shapes_cannot_fall_through_to_global_types() {
    let s=snapshot(&[("/p/a.cs","p","class C { public static void Api(){} } class Generic<C> { void Run(){C.Api();} } class Primary(int C) {void Run(){C.Api();}} class Entry { void Generic<C>(){C.Api();} void Catch(){try {} catch(System.Exception C){C.Api();}} void Pattern(object x){if(x is int C){C.Api();}} void Tuple(){var (C,x)=(1,2);C.Api();} }")]);
    assert!(
        s.calls.iter().all(|e| e.candidates.is_empty()),
        "{:?}",
        s.calls
    );
    let s=snapshot(&[("/p/a.cs","p","using C=System.Collections.Generic.List<int>; class C { public static void Api(){} } class Entry { static void Main(){C.Api();} }")]);
    assert!(s.calls.iter().all(|e| e.candidates.is_empty()));
}
#[test]
fn csharp_nested_local_function_preserves_captured_value_shadow() {
    let s=snapshot(&[("/p/a.cs","p","class C { public static void Api(){} } class Entry { void Outer(int C){void Inner(){C.Api();} Inner();} }")]);
    assert!(candidates(&s, "Entry.Inner", "C.Api").is_empty());
}
#[test]
fn csharp_inherited_fields_and_unknown_bases_prevent_type_receiver_guesses() {
    let s=snapshot(&[("/p/a.cs","p","class Target { public static void Api(){} } class Base { protected int Target; } class Child : Base { void Run(){Target.Api();} } class Unknown : External { void Run(){Target.Api();} }")]);
    assert!(
        s.calls.iter().all(|e| e.candidates.is_empty()),
        "{:?}",
        s.calls
    );
}
