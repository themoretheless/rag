use super::*;
struct Snapshot {
    symbols: Vec<Symbol>,
    edges: Vec<CallEdge>,
    docs: HashMap<String, IndexedDocument>,
}
fn snapshot(files: &[(&str, &str, &str)]) -> Snapshot {
    let docs: HashMap<_, _> = files
        .iter()
        .enumerate()
        .map(|(i, (path, project, source))| {
            let id = format!("d{i}");
            let mut doc = parse(&id, path, source, 1).unwrap();
            doc.project = Some((*project).into());
            assert!(!doc.parse_errors, "{path}: {source}");
            (id, doc)
        })
        .collect();
    let symbols = docs
        .values()
        .flat_map(|d| d.symbols.clone())
        .collect::<Vec<_>>();
    let edges = resolve::references(&symbols, &docs);
    Snapshot {
        symbols,
        edges,
        docs,
    }
}
fn targets(s: &Snapshot, caller: &str, expression: &str) -> Vec<String> {
    let owners: BTreeSet<_> = s
        .symbols
        .iter()
        .filter(|s| s.qualified_name == caller)
        .map(|s| s.id.as_str())
        .collect();
    let found: Vec<_> = s
        .edges
        .iter()
        .filter(|e| {
            e.reference
                .caller
                .as_deref()
                .is_some_and(|id| owners.contains(id))
                && e.reference.expression == expression
        })
        .collect();
    assert_eq!(found.len(), 1, "{caller} {expression}: {:?}", s.edges);
    found[0]
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
fn perl_package_blocks_restore_lexical_package_and_explicit_calls() {
    let s=snapshot(&[("/p/a.pm","p","package A; sub work {1} sub run {work();} {package B; sub work {2} sub run {work();}} sub again {work();} package C {sub work {3} sub run {work();}} sub last {A::work();}")]);
    assert_eq!(targets(&s, "A::run", "work"), ["A::work"]);
    assert_eq!(targets(&s, "B::run", "work"), ["B::work"]);
    assert_eq!(targets(&s, "A::again", "work"), ["A::work"]);
    assert_eq!(targets(&s, "C::run", "work"), ["C::work"]);
    assert_eq!(targets(&s, "A::last", "A::work"), ["A::work"]);
}
#[test]
fn perl_forward_declarations_lexical_subs_and_duplicate_definitions() {
    let s = snapshot(&[(
        "/p/a.pl",
        "p",
        "sub work; sub work {1} sub run {my sub work {2} work();} sub main {work();}",
    )]);
    assert_eq!(targets(&s, "main::main", "work"), ["main::work"]);
    assert_eq!(targets(&s, "main::run", "work").len(), 1);
    let s = snapshot(&[
        ("/p/a.pm", "p", "package P;sub work {1}"),
        ("/p/b.pm", "p", "package P;sub work {2}sub run{work();}"),
    ]);
    assert_eq!(targets(&s, "P::run", "work").len(), 2);
}
#[test]
fn perl_parent_package_methods_import_files_and_project_isolation() {
    let s = snapshot(&[
        ("/p/base.pm", "p", "package Base;sub work {1}"),
        (
            "/p/child.pm",
            "p",
            "package Child;use parent 'Base';sub run {Child->work();}",
        ),
        ("/p/main.pl", "p", "use Child ();sub main {Child->run();}"),
        ("/foreign/base.pm", "other", "package Base;sub work {2}"),
    ]);
    assert_eq!(targets(&s, "Child::run", "Child->work"), ["Base::work"]);
    assert_eq!(targets(&s, "main::main", "Child->run"), ["Child::run"]);
    let imports = file_imports::edges(&s.docs);
    assert_eq!(imports.len(), 2);
    assert!(imports
        .iter()
        .all(|e| e.candidates.len() == 1 && !e.runtime_import_proven));
    let child = s.docs.values().find(|d| d.path == "/p/child.pm").unwrap();
    let bases = edges(&s.docs, child);
    assert_eq!(bases.len(), 1);
    assert_eq!(bases[0].candidates.len(), 1);
    assert_ne!(bases[0].hierarchy_status, "complete_static");
}
#[test]
fn perl_variables_distinguish_scalar_array_hash_and_lexical_shadowing() {
    let s = snapshot(&[(
        "/p/a.pl",
        "p",
        "my $x=1;my @x=(1);my %x=(a=>1);sub run {my $x=2; $x; $x[0]; $x{a};} sub outside {$x;}",
    )]);
    let run = s
        .symbols
        .iter()
        .find(|s| s.qualified_name == "main::run")
        .unwrap();
    let uses: Vec<_> = s
        .edges
        .iter()
        .filter(|e| e.reference.caller.as_deref() == Some(&run.id))
        .collect();
    assert_eq!(uses.len(), 3, "{:?}", s.edges);
    assert_eq!(
        uses.iter()
            .map(|e| e.reference.name.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["$x", "@x", "%x"])
    );
    assert!(uses.iter().all(|e| e.candidates.len() == 1), "{:?}", uses);
    let inner = uses
        .iter()
        .find(|e| e.reference.name == "$x")
        .unwrap()
        .candidates[0]
        .clone();
    let outer = targets(&s, "main::outside", "$x");
    assert_eq!(outer, ["main::$x"]);
    let outer_symbol = s
        .symbols
        .iter()
        .filter(|s| s.qualified_name == "main::$x")
        .min_by_key(|s| s.start_byte)
        .unwrap();
    assert_ne!(inner, outer_symbol.id);
}
#[test]
fn perl_dynamic_receivers_imports_eval_and_symbol_table_mutation_fail_closed() {
    let s = snapshot(&[(
        "/p/a.pm",
        "p",
        "package P;use Other;sub work {1}sub run {work();P->work();$object->work();}",
    )]);
    assert!(s
        .edges
        .iter()
        .filter(|e| e.reference.is_call)
        .all(|e| e.candidates.is_empty()));
    let s = snapshot(&[(
        "/p/a.pm",
        "p",
        "package P;sub work {1}sub run {eval 'work()'; work();}",
    )]);
    assert!(s.edges.iter().all(|e| e.candidates.is_empty()));
}
#[test]
fn perl_signature_parameters_and_anonymous_sub_bodies_are_inventoried() {
    let s = snapshot(&[(
        "/p/a.pl",
        "p",
        "use feature 'signatures';sub run ($x) {$x; my $f=sub ($y) {$y;};}",
    )]);
    assert!(s
        .symbols
        .iter()
        .any(|s| s.kind == "anonymous_subroutine_expression"));
    assert!(
        s.edges
            .iter()
            .filter(|e| e.reference.name == "$x" || e.reference.name == "$y")
            .all(|e| e.candidates.len() == 1),
        "{:?}",
        s.edges
    );
}
#[test]
fn perl_nested_receivers_have_distinct_anchors_and_exclude_argument_bodies() {
    let s = snapshot(&[(
        "/p/a.pm",
        "p",
        "package P;sub factory {1}sub work {2}sub run {factory('PRIVATE_BODY')->work();P->work();}",
    )]);
    assert!(!serde_json::to_string(&s.edges)
        .unwrap()
        .contains("PRIVATE_BODY"));
    assert!(
        !serde_json::to_string(&s.docs.values().next().unwrap().perl)
            .unwrap()
            .contains("PRIVATE_BODY")
    );
    assert_eq!(targets(&s, "P::run", "factory"), ["P::factory"]);
    assert_eq!(targets(&s, "P::run", "P->work"), ["P::work"]);
    assert!(targets(&s, "P::run", "<receiver>->work").is_empty());
}
#[test]
fn perl_initializer_reads_outer_binding_and_reference_access_keeps_scalar_sigil() {
    let s = snapshot(&[(
        "/p/a.pl",
        "p",
        "my $x=1;sub run {my $x=$x; $x; my $ref={key=>1};$ref->{key};}",
    )]);
    let run = s
        .symbols
        .iter()
        .find(|s| s.qualified_name == "main::run")
        .unwrap();
    let uses: Vec<_> = s
        .edges
        .iter()
        .filter(|e| e.reference.caller.as_deref() == Some(&run.id) && e.reference.name == "$x")
        .collect();
    assert_eq!(uses.len(), 2);
    assert!(
        uses.iter().all(|e| e.candidates.len() == 1),
        "{:?}",
        s.edges
    );
    assert_ne!(uses[0].candidates, uses[1].candidates);
    let refs: Vec<_> = s
        .edges
        .iter()
        .filter(|e| e.reference.name == "$ref")
        .collect();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].candidates.len(), 1);
    assert!(!s.edges.iter().any(|e| e.reference.name == "%ref"));
}
#[test]
fn perl_function_values_and_modern_class_methods_remain_distinct() {
    let s=snapshot(&[("/p/a.pl","p","sub work {1}sub main {my $callback=\\&work; $callback->(); work();} use feature 'class';class C {method run {1}}")]);
    assert!(s
        .symbols
        .iter()
        .any(|s| s.kind == "class_statement" && s.name == "C"));
    assert!(s
        .symbols
        .iter()
        .any(|s| s.kind == "method_declaration_statement" && s.name == "run"));
    assert!(
        s.edges
            .iter()
            .any(|e| !e.reference.is_call && e.reference.name == "work" && e.candidates.len() == 1),
        "{:?}",
        s.edges
    );
    assert!(s
        .edges
        .iter()
        .filter(|e| e.reference.name == "<coderef>")
        .all(|e| e.candidates.is_empty()));
}
#[test]
fn perl_store_body_cache_refresh_and_frozen_legacy_relations() {
    use crate::models::Document;
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("perl.duckdb")).unwrap();
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
        "/p/base.pm",
        "# café\npackage Base;sub work {'PRIVATE_BODY';1;}\n",
    );
    let mut child = doc(
        "child",
        "/p/child.pm",
        "package Child;use parent 'Base';sub run {Child->work();}",
    );
    store.documents().upsert_document(&base).unwrap();
    store.documents().upsert_document(&child).unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .index_code()
            .unwrap()
            .indexed,
        2
    );
    let q = CodeQuery {
        symbol: Some("Base::work".into()),
        ..Default::default()
    };
    let before = crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap()
        .query("callers", &q);
    assert_eq!(before["items"].as_array().unwrap().len(), 1, "{before}");
    let body = crate::code_index::CodeIndex::new(&store)
        .code_body(&q)
        .unwrap();
    assert!(body.to_string().contains("PRIVATE_BODY"));
    assert!(!before.to_string().contains("PRIVATE_BODY"));
    let forced = crate::code_index::CodeIndex::new(&store)
        .index_code_with_options(&resolution_cache::IndexOptions {
            force_full_resolution: true,
        })
        .unwrap();
    assert!(!forced.revision_created);
    assert_eq!(
        forced.resolution.unwrap().equivalent_to_existing,
        Some(true)
    );
    child.content = "package Child;sub run {1}".into();
    store.documents().upsert_document(&child).unwrap();
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
    assert_eq!(diff["perl_relations_available"], true);
    assert!(
        diff["relations_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["relation"] == "perl_parent_package_candidate"),
        "{diff}"
    );
    assert!(diff["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["import_kind"] == "perl_parent_package_candidate"));
    child.content = "package Later;".into();
    store.documents().upsert_document(&child).unwrap();
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
    graph.as_object_mut().unwrap().remove("perl_syntax_format");
    conn.execute(
        "UPDATE code_index_revisions SET graph_json=? WHERE id=1",
        [graph.to_string()],
    )
    .unwrap();
    drop(conn);
    let legacy = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&query)
        .unwrap();
    assert_eq!(legacy["perl_relations_available"], false);
    assert!(!legacy["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["relation"] == "perl_parent_package_candidate"
            || e["import_kind"] == "perl_parent_package_candidate"));
}
