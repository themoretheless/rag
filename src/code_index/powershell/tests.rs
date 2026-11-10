
use super::*;
struct Snapshot {
    symbols: Vec<Symbol>,
    refs: Vec<CallEdge>,
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
    let refs = resolve::references(&symbols, &docs);
    Snapshot {
        symbols,
        refs,
        docs,
    }
}
fn targets(s: &Snapshot, caller: &str, expression: &str) -> Vec<String> {
    let ids: BTreeSet<_> = s
        .symbols
        .iter()
        .filter(|s| s.qualified_name == caller)
        .map(|s| s.id.as_str())
        .collect();
    let edges: Vec<_> = s
        .refs
        .iter()
        .filter(|e| {
            e.reference
                .caller
                .as_deref()
                .is_some_and(|id| ids.contains(id))
                && e.reference.expression == expression
        })
        .collect();
    assert_eq!(edges.len(), 1, "{caller} {expression}: {:?}", s.refs);
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
fn powershell_functions_are_case_insensitive_and_nested_scope_is_bounded() {
    let s=snapshot(&[("/p/a.ps1","p","function Get-Work { 1 }\nfunction Invoke-Main { get-work; function Get-Inner { 2 }; GET-INNER }\nfunction Outside { Get-Inner }")]);
    assert_eq!(targets(&s, "Invoke-Main", "get-work"), ["Get-Work"]);
    assert_eq!(targets(&s, "Invoke-Main", "get-inner"), ["Get-Inner"]);
    assert!(targets(&s, "Outside", "get-inner").is_empty());
}
#[test]
fn powershell_dot_source_literal_root_links_functions_and_project_guards() {
    let s = snapshot(&[
        ("/p/lib.ps1", "p", "function Get-Work {1}"),
        (
            "/p/main.ps1",
            "p",
            ". \"$PSScriptRoot/lib.ps1\"\nfunction Invoke-Main {Get-Work}",
        ),
        ("/foreign/lib.ps1", "other", "function Get-Work {2}"),
    ]);
    assert_eq!(targets(&s, "Invoke-Main", "get-work"), ["Get-Work"]);
    let edges = file_imports::edges(&s.docs);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].candidates.len(), 1);
    assert!(!edges[0].runtime_import_proven);
    let s = snapshot(&[
        ("/p/lib.ps1", "p", "function Get-Work {1}"),
        (
            "/p/main.ps1",
            "p",
            ". ./lib.ps1\nfunction Invoke-Main {Get-Work}",
        ),
    ]);
    assert!(targets(&s, "Invoke-Main", "get-work").is_empty());
}
#[test]
fn powershell_class_static_instance_constructor_and_base_candidates() {
    let s=snapshot(&[("/p/a.ps1","p","class Base { [int] Work(){return 1} static [int] Api(){return 2} }\nclass Child : Base { Child(){} [int] Run(){return $this.Work()} static [int] Call(){return [Base]::Api()} }\nfunction Invoke-Main {[Child]::new()}")]);
    assert_eq!(targets(&s, "child::Run", "child.work"), ["base::Work"]);
    assert_eq!(targets(&s, "child::Call", "base::api"), ["base::Api"]);
    assert_eq!(targets(&s, "Invoke-Main", "child::new"), ["child::Child"]);
    let bases = edges(&s.docs, s.docs.values().next().unwrap());
    assert_eq!(bases.len(), 1);
    assert_eq!(bases[0].candidates.len(), 1);
    assert_ne!(bases[0].hierarchy_status, "complete_static");
}
#[test]
fn powershell_using_module_types_do_not_invent_function_exports() {
    let s = snapshot(&[
        (
            "/p/types.psm1",
            "p",
            "class Worker { static [int] Run(){return 1} }\nfunction Get-Work {1}",
        ),
        (
            "/p/main.ps1",
            "p",
            "using module './types.psm1'\nfunction Invoke-Main {[Worker]::Run();Get-Work}",
        ),
    ]);
    assert_eq!(targets(&s, "Invoke-Main", "worker::run"), ["worker::Run"]);
    assert!(targets(&s, "Invoke-Main", "get-work").is_empty());
}
#[test]
fn powershell_alias_dynamic_loader_scoped_and_conditional_definitions_fail_closed() {
    for source in [
        "function Get-Work {1}\nSet-Alias Get-Work Other\nfunction Invoke-Main {Get-Work}",
        "function Get-Work {1}\nImport-Module Whatever\nfunction Invoke-Main {Get-Work}",
        "function script:Get-Work {1}\nfunction Invoke-Main {Get-Work}",
        "if($true){function Get-Work {1}}\nfunction Invoke-Main {Get-Work}",
    ] {
        let s = snapshot(&[("/p/a.ps1", "p", source)]);
        assert!(
            targets(&s, "Invoke-Main", "get-work").is_empty(),
            "{source}"
        );
    }
}
#[test]
fn powershell_overloads_preserve_ambiguity_and_unknown_receivers_stay_unbound() {
    let s=snapshot(&[("/p/a.ps1","p","class Worker { static [int] Run([int]$n){return 1} static [int] Run([string]$n){return 2} [int] Work(){return 3} }\nfunction Invoke-Main {[Worker]::Run(1);$unknown.Work();[Worker].Work()}")]);
    assert_eq!(targets(&s, "Invoke-Main", "worker::run").len(), 2);
    assert!(s
        .refs
        .iter()
        .filter(|e| e.reference.name == "work")
        .all(|e| e.candidates.is_empty()));
}
#[test]
fn powershell_deferred_blocks_keep_occurrences_without_assuming_execution_scope() {
    let s = snapshot(&[(
        "/p/a.ps1",
        "p",
        "function Get-Work {1}\nfunction Invoke-Main { & { Get-Work }; $f={Get-Work}; & $f }",
    )]);
    let uses: Vec<_> = s
        .refs
        .iter()
        .filter(|e| e.reference.name == "get-work")
        .collect();
    assert_eq!(uses.len(), 2);
    assert!(uses.iter().all(|e| e.candidates.is_empty()));
}
#[test]
fn powershell_enum_properties_and_body_spans_are_source_bound() {
    let source="# café\nenum Color { Red; Blue }\nclass Worker { [string] $Value='PRIVATE_INITIALIZER'; [int] Work(){ return 1 } }";
    let s = snapshot(&[("/p/a.ps1", "p", source)]);
    assert!(s.symbols.iter().any(|s| s.kind == "enum_statement"));
    let property = s
        .symbols
        .iter()
        .find(|s| s.kind == "class_property_definition")
        .unwrap();
    assert!(!property.signature.contains("PRIVATE_INITIALIZER"));
    let method = s
        .symbols
        .iter()
        .find(|s| s.kind == "class_method_definition")
        .unwrap();
    assert_eq!(
        &source[method.start_byte..method.end_byte],
        "[int] Work(){ return 1 }"
    );
}
#[test]
fn powershell_load_collisions_and_nested_loads_remain_uncertain() {
    let s = snapshot(&[
        ("/p/lib.ps1", "p", "function Get-Work {2}"),
        (
            "/p/main.ps1",
            "p",
            "function Get-Work {1}\n. \"$PSScriptRoot/lib.ps1\"\nfunction Invoke-Main {Get-Work}",
        ),
    ]);
    assert_eq!(targets(&s, "Invoke-Main", "get-work").len(), 2);
    let s = snapshot(&[
        ("/p/lib.ps1", "p", "function Get-Work {2}"),
        (
            "/p/main.ps1",
            "p",
            "function Loader {. \"$PSScriptRoot/lib.ps1\"}\nfunction Invoke-Main {Get-Work}",
        ),
    ]);
    assert!(targets(&s, "Invoke-Main", "get-work").is_empty());
}
#[test]
fn powershell_body_comments_are_excluded_from_metadata() {
    let s=snapshot(&[("/p/a.ps1","p","# café\nfunction Invoke-Main { # PRIVATE_BODY\n 1 }\nclass C {[int] Run(){ # PRIVATE_BODY\n return 1 }}")]);
    assert!(!serde_json::to_string(&s.symbols)
        .unwrap()
        .contains("PRIVATE_BODY"));
}
#[test]
fn powershell_store_body_cache_stale_sync_and_legacy_history() {
    use crate::models::Document;
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("powershell.duckdb")).unwrap();
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
        "/p/base.psm1",
        "# café\nclass Base {[int] Work(){ # PRIVATE_BODY\n return 1 }}",
    );
    let mut client = doc(
        "client",
        "/p/client.ps1",
        "using module './base.psm1'\nclass Child : Base {[int] Run(){return $this.Work()}}",
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
        symbol: Some("base::Work".into()),
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
        .contains("PRIVATE_BODY"));
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
    client.content = "class Child {[int] Run(){return 1}}".into();
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
    assert_eq!(diff["powershell_relations_available"], true);
    assert!(
        diff["relations_removed"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["relation"] == "powershell_base_type_candidate"),
        "{diff}"
    );
    assert!(diff["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["import_kind"] == "powershell_using_module_candidate"));
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
        .remove("powershell_syntax_format");
    conn.execute(
        "UPDATE code_index_revisions SET graph_json=? WHERE id=1",
        [graph.to_string()],
    )
    .unwrap();
    drop(conn);
    let legacy = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&query)
        .unwrap();
    assert_eq!(legacy["powershell_relations_available"], false);
    assert!(!legacy["relations_removed"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["relation"] == "powershell_base_type_candidate"
            || e["import_kind"] == "powershell_using_module_candidate"));
}

#[test]
fn powershell_literal_using_double_quotes_preserve_original_byte_anchors() {
    let source="# café\nusing module \"./child.psm1\"\n. \"$PSScriptRoot/functions.ps1\"\nfunction Invoke-Main {GET-WORK; [Child]::Call(); & $unknown}\nInvoke-Main\n";
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_pwsh::LANGUAGE.into())
        .unwrap();
    let tree = parser.parse(source, None).unwrap();
    assert!(tree.root_node().has_error());
    let normalized = normalize_using_quotes(source).unwrap();
    assert_eq!(normalized.len(), source.len());
    assert!(normalized.contains("using module './child.psm1'"));
    let doc = parse("main", "/p/main.ps1", source, 1).unwrap();
    assert!(!doc.parse_errors);
    assert!(doc
        .binding_limitations
        .iter()
        .any(|v| v == "POWERSHELL_LITERAL_USING_QUOTES_NORMALIZED"));
    let symbol = doc
        .symbols
        .iter()
        .find(|s| s.name == "Invoke-Main")
        .unwrap();
    assert_eq!(
        &source[symbol.start_byte..symbol.end_byte],
        "function Invoke-Main {GET-WORK; [Child]::Call(); & $unknown}"
    );
}
#[test]
fn powershell_using_quote_adapter_rejects_interpolation_escapes_and_non_header_text() {
    for source in [
        "using module \"$root/file.psm1\"\n",
        "function Main { 'using module \"./x.psm1\"' }\n",
        "<#\nusing module \"./x.psm1\"\n#>\n",
        "using module \"./a'b.psm1\"\n",
        "using module \"./x.psm1\" unexpected\n",
    ] {
        assert!(normalize_using_quotes(source).is_none(), "{source}");
    }
    let escaped = format!("using module \"./a{}b.psm1\"\n", char::from(96));
    assert!(normalize_using_quotes(&escaped).is_none());
    let source="# café\r\n<# comment #>\r\n  USING   MODULE   \"./types.psm1\" # trailing\r\nfunction Main {}";
    let normalized = normalize_using_quotes(source).unwrap();
    assert_eq!(normalized.len(), source.len());
    assert_eq!(normalized.lines().count(), source.lines().count());
    let doc = parse("main", "/p/main.ps1", source, 1).unwrap();
    assert!(!doc.parse_errors);
}
