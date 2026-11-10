
use super::*;
fn graph(manifest: &str, source: &str) -> (CodeSnapshot, HashMap<String, IndexedDocument>) {
    let mut docs = HashMap::new();
    for (id, path, body) in [
        ("manifest", "/p/Cargo.toml", manifest),
        ("lib", "/p/src/lib.rs", source),
        ("enabled", "/p/src/enabled.rs", "pub fn leaf(){}"),
        ("bin", "/p/src/main.rs", "fn exclusive_bin(){}"),
    ] {
        let mut d = parse(id, path, body, 1).unwrap();
        assert!(!d.parse_errors, "{path}");
        d.project = Some("fixture".into());
        docs.insert(id.into(), d);
    }
    let applied = apply(&mut docs);
    let mut snapshot = CodeSnapshot {
        rust_build_contexts: applied.contexts,
        binding_diagnostics: applied.diagnostics,
        ..Default::default()
    };
    snapshot.symbols = docs.values().flat_map(|d| d.symbols.clone()).collect();
    snapshot.bound_references = resolve::references(&snapshot.symbols, &docs);
    snapshot.calls = snapshot
        .bound_references
        .iter()
        .filter(|e| e.reference.is_call)
        .cloned()
        .collect();
    (snapshot, docs)
}
fn manifest(features: &str, defaults: bool) -> String {
    format!("[package]\nname='fixture'\nversion='0.1.0'\n[features]\ndefault=['slow']\nslow=[]\nfast=['leaf']\nleaf=[]\n[package.metadata.rag.graph]\nversion=1\ntarget_kind='lib'\nfeatures=[{features}]\ndefault_features={defaults}\n")
}
const SOURCE:&str="#[cfg(feature=\"fast\")] pub fn fast(){} #[cfg(not(feature=\"fast\"))]pub fn slow(){} #[cfg(feature=\"leaf\")]pub mod enabled; #[cfg(feature=\"fast\")]pub fn run(){fast();enabled::leaf();} #[cfg(not(feature=\"fast\"))]pub fn run(){slow();}";
#[test]
fn explicit_features_select_symbols_modules_and_callers_in_one_target() {
    let (g, _) = graph(&manifest("'fast'", false), SOURCE);
    assert_eq!(g.rust_build_contexts.len(), 1);
    assert_eq!(
        g.rust_build_contexts[0]["features"],
        serde_json::json!(["fast", "leaf"])
    );
    assert!(g.symbols.iter().any(|s| s.name == "fast"));
    assert!(!g
        .symbols
        .iter()
        .any(|s| s.name == "slow" || s.name == "exclusive_bin"));
    assert_eq!(g.symbols.iter().filter(|s| s.name == "run").count(), 1);
    assert_eq!(g.calls.len(), 2);
    assert!(
        g.calls.iter().all(|c| c.candidates.len() == 1),
        "{:?}",
        g.calls
    );
    assert!(g
        .symbols
        .iter()
        .all(|s| s.build_context.is_some() && s.id.contains('@')));
}
#[test]
fn empty_compiler_cfg_values_are_valid_but_empty_flag_and_feature_names_are_not() {
    let configured =
        manifest("", false) + "[package.metadata.rag.graph.cfg_values]\ntarget_env=['']\n";
    let source="#[cfg(target_env=\"\")]fn selected(){} #[cfg(target_env=\"gnu\")]fn wrong(){} fn run(){selected();}";
    let (g, _) = graph(&configured, source);
    assert_eq!(g.rust_build_contexts[0]["available"], true);
    assert!(g.symbols.iter().any(|s| s.name == "selected"));
    assert!(!g.symbols.iter().any(|s| s.name == "wrong"));
    assert_eq!(g.calls[0].candidates.len(), 1);
    for bad in [
        manifest("''", false),
        manifest("", false) + "cfg_flags=['']\n",
    ] {
        let (g, _) = graph(&bad, source);
        assert_eq!(g.rust_build_contexts[0]["available"], false);
    }
}
#[test]
fn compiler_target_feature_sets_larger_than_32_are_admitted_with_a_finite_limit() {
    for count in [34, 128, 129] {
        let values = (0..count)
            .map(|i| format!("'feature{i}'"))
            .collect::<Vec<_>>()
            .join(",");
        let configured = manifest("", false)
            + &format!("[package.metadata.rag.graph.cfg_values]\ntarget_feature=[{values}]\n");
        let (g, _) = graph(
            &configured,
            "#[cfg(target_feature=\"feature33\")]fn enabled(){} fn run(){enabled();}",
        );
        assert_eq!(g.rust_build_contexts[0]["available"], count <= 128);
        if count <= 128 {
            assert_eq!(g.calls[0].candidates.len(), 1);
        }
    }
}
#[test]
fn default_features_and_context_identity_change_without_rust_source_change() {
    let (fast, _) = graph(&manifest("'fast'", false), SOURCE);
    let (slow, _) = graph(&manifest("", true), SOURCE);
    assert_ne!(
        fast.rust_build_contexts[0]["configuration_id"],
        slow.rust_build_contexts[0]["configuration_id"]
    );
    assert!(slow.symbols.iter().any(|s| s.name == "slow"));
    assert!(!slow
        .symbols
        .iter()
        .any(|s| s.name == "fast" || s.name == "leaf"));
    assert_eq!(slow.calls.len(), 1);
    assert_eq!(slow.calls[0].reference.expression, "slow");
    assert_eq!(slow.calls[0].candidates.len(), 1);
}
#[test]
fn cfg_unknowns_and_unknown_configuration_keys_do_not_guess() {
    let source = "#[cfg(target_os=\"linux\")]fn hidden(){} fn open(){}";
    let (unknown, _) = graph(&manifest("", false), source);
    assert!(!unknown.symbols.iter().any(|s| s.name == "hidden"));
    assert_eq!(unknown.rust_build_contexts[0]["unknown_conditions"], 1);
    let (invalid, _) = graph(&(manifest("", false) + "feautres=[]\n"), "fn run(){run();}");
    assert_eq!(invalid.rust_build_contexts[0]["available"], false);
    assert!(invalid.calls.iter().all(|e| e.candidates.is_empty()));
}
#[test]
fn three_valued_predicates_preserve_false_and_true_short_circuits() {
    let input = Input {
        target_kind: "lib".into(),
        target_name: None,
        features: BTreeSet::new(),
        default_features: false,
        flags: BTreeSet::new(),
        values: BTreeMap::new(),
        target_triple: None,
    };
    for (raw, expected) in [
        ("all(feature=\"off\",target_os=\"linux\")", Some(false)),
        ("any(not(feature=\"off\"),target_os=\"linux\")", Some(true)),
        ("target_os=\"linux\"", None),
    ] {
        assert_eq!(
            evaluate(&predicate(raw).unwrap(), &input, &BTreeSet::new()),
            expected
        );
    }
    assert!(predicate("not(a,b)").is_none());
    assert!(predicate("all(a) trailing").is_none());
}
#[test]
fn configured_fields_select_all_raw_alternatives_and_the_active_owner() {
    for declarations in [
            "struct Holder{#[cfg(feature=\"fast\")]value:User,#[cfg(not(feature=\"fast\"))]value:Other}",
            "#[cfg(feature=\"fast\")]struct Holder{value:User}#[cfg(not(feature=\"fast\"))]struct Holder{value:Other}",
        ] {
            let source = format!("struct User;impl User{{fn save(&self){{}}}}struct Other;impl Other{{fn save(&self){{}}}}{declarations}impl Holder{{fn run(&self){{self.value.save();}}}}fn make_user()->User{{User}}fn make_other()->Other{{Other}}fn make()->Holder{{Holder{{#[cfg(feature=\"fast\")]value:make_user(),#[cfg(not(feature=\"fast\"))]value:make_other()}}}}");
            for (features, expected) in [("'fast'", "User::save"), ("", "Other::save")] {
                let (g, docs) = graph(&manifest(features, false), &source);
                assert!(docs["lib"].binding_limitations.is_empty(), "{:?}", docs["lib"].rust_cfg.conditions);
                let edge = g.calls.iter().find(|c| c.reference.expression == "self.value.save").unwrap();
                assert_eq!(edge.candidates.len(), 1, "{edge:?}");
                assert_eq!(g.symbols.iter().find(|s| s.id == edge.candidates[0]).unwrap().qualified_name, expected);
                let (present, absent) = if features.is_empty() { ("make_other", "make_user") } else { ("make_user", "make_other") };
                assert!(!g.calls.iter().any(|c| c.reference.expression == absent));
                assert_eq!(g.calls.iter().find(|c| c.reference.expression == present).unwrap().candidates.len(), 1);
            }
        }
}
#[test]
fn unknown_or_duplicate_field_variants_do_not_leave_a_partial_unique_type() {
    for fields in [
        "value:User,#[cfg(target_os=\"linux\")]value:Other",
        "#[cfg(target_os=\"linux\")]value:Other,value:User",
    ] {
        let source = format!("struct User;impl User{{fn save(&self){{}}}}struct Other;impl Other{{fn save(&self){{}}}}struct Holder{{{fields}}}impl Holder{{fn run(&self){{self.value.save();}}}}fn alive(){{}}fn run(){{alive();}}");
        for (cfg, expected) in [
            ("", 0),
            (
                "[package.metadata.rag.graph.cfg_values]\ntarget_os=['macos']\n",
                1,
            ),
            (
                "[package.metadata.rag.graph.cfg_values]\ntarget_os=['linux']\n",
                0,
            ),
        ] {
            let (g, docs) = graph(&(manifest("", false) + cfg), &source);
            assert!(docs["lib"].binding_limitations.is_empty());
            assert_eq!(
                g.calls
                    .iter()
                    .find(|c| c.reference.expression == "self.value.save")
                    .unwrap()
                    .candidates
                    .len(),
                expected
            );
            assert_eq!(
                g.calls
                    .iter()
                    .find(|c| c.reference.expression == "alive")
                    .unwrap()
                    .candidates
                    .len(),
                1
            );
        }
    }
}
#[test]
fn extern_aliases_and_unknown_imports_do_not_fabricate_standard_wrappers() {
    let body = "struct User;impl User{fn save(&self){}}fn run(values:standard::vec::Vec<User>){values[0].save();}#[cfg(feature=\"fast\")]extern crate std as standard;";
    let (g, _) = graph(&manifest("'fast'", false), body);
    assert_eq!(
        g.calls
            .iter()
            .find(|c| c.reference.expression == "values[0].save")
            .unwrap()
            .candidates
            .len(),
        1
    );
    for declaration in [
        "extern crate vendor as std;",
        "#[cfg(target_os=\"linux\")]extern crate vendor as std;",
        "#[cfg(target_os=\"linux\")]use crate::custom::Vec;",
    ] {
        let ty = if declaration.contains("::Vec") {
            "Vec<User>"
        } else {
            "std::vec::Vec<User>"
        };
        let source = format!("struct User;impl User{{fn save(&self){{}}}}fn run(values:{ty}){{values[0].save();}}{declaration}");
        let (g, docs) = graph(&manifest("", false), &source);
        assert!(docs["lib"].binding_limitations.is_empty());
        assert!(g
            .calls
            .iter()
            .find(|c| c.reference.expression == "values[0].save")
            .unwrap()
            .candidates
            .is_empty());
    }
    let (g, _) = graph(&(manifest("", false) + "[dependencies]\nstd='1'\n"), "struct User;impl User{fn save(&self){}}fn run(values:std::vec::Vec<User>){values[0].save();}");
    assert!(g
        .calls
        .iter()
        .find(|c| c.reference.expression == "values[0].save")
        .unwrap()
        .candidates
        .is_empty());
}
#[test]
fn unsupported_parameters_in_disabled_modules_do_not_block_live_calls() {
    let source = "#[cfg(test)]mod hidden{fn conditional(#[cfg(feature=\"fast\")]x:u8){}}fn alive(){}fn run(){alive();}";
    let (g, docs) = graph(&manifest("", false), source);
    assert!(docs["lib"].binding_limitations.is_empty());
    assert_eq!(
        g.calls
            .iter()
            .find(|c| c.reference.expression == "alive")
            .unwrap()
            .candidates
            .len(),
        1
    );
    let (g, docs) = graph(&(manifest("", false) + "cfg_flags=['test']\n"), source);
    assert!(docs["lib"]
        .binding_limitations
        .iter()
        .any(|c| c == "RUST_CFG_NESTED_SEMANTICS_UNMODELED"));
    assert!(g.calls.iter().all(|c| c.candidates.is_empty()));
}
#[test]
fn known_absent_macros_are_removed_but_active_macro_expansion_is_not_assumed() {
    for (predicate, expected) in [("any()", 1), ("all()", 0)] {
        let source = format!("#[cfg({predicate})]compile_error!(\"configuration unavailable\");fn alive(){{}}fn run(){{alive();}}");
        let (g, _) = graph(&manifest("", false), &source);
        assert_eq!(
            g.calls
                .iter()
                .find(|c| c.reference.expression == "alive")
                .unwrap()
                .candidates
                .len(),
            expected
        );
    }
}
#[test]
fn inactive_type_aliases_use_declaration_anchors_not_their_hoisted_scope_start() {
    let source = "struct User;impl User{fn save(&self){}}#[cfg(feature=\"fast\")]type std=Bad;struct Bad;fn run(values:std::vec::Vec<User>){values[0].save();}";
    for (features, expected) in [("", 1), ("'fast'", 0)] {
        let (g, _) = graph(&manifest(features, false), source);
        assert_eq!(
            g.calls
                .iter()
                .find(|c| c.reference.expression == "values[0].save")
                .unwrap()
                .candidates
                .len(),
            expected
        );
    }
}
#[test]
fn conditional_field_initializer_returns_do_not_publish_unprojected_permission_syntax() {
    let source = "struct Holder{item:()}fn allow()->bool{let _holder=Holder{#[cfg(feature=\"fast\")]item:{return true;},#[cfg(not(feature=\"fast\"))]item:()};false}fn alive(){}fn run(){alive();}";
    assert!(!parse("raw", "/p/src/lib.rs", source, 1)
        .unwrap()
        .policies
        .is_empty());
    let (g, docs) = graph(&manifest("", false), source);
    assert!(docs["lib"].binding_limitations.is_empty());
    assert!(docs["lib"].policies.is_empty());
    assert_eq!(
        g.calls
            .iter()
            .find(|c| c.reference.expression == "alive")
            .unwrap()
            .candidates
            .len(),
        1
    );
}
#[test]
fn conditional_statement_blocks_keep_only_active_body_calls() {
    let source = "fn left()->u32{1} fn right()->u32{2} fn unrelated(){} fn run()->u32{#[cfg(feature=\"fast\")]{left()} #[cfg(not(feature=\"fast\"))]{right()}} fn other(){unrelated();}";
    for (features, present, absent) in [("'fast'", "left", "right"), ("", "right", "left")] {
        let (g, docs) = graph(&manifest(features, false), source);
        assert!(
            docs["lib"].binding_limitations.is_empty(),
            "{:?}",
            docs["lib"].rust_cfg.conditions
        );
        assert!(!g.calls.iter().any(|c| c.reference.expression == absent));
        for expected in [present, "unrelated"] {
            assert_eq!(
                g.calls
                    .iter()
                    .find(|c| c.reference.expression == expected)
                    .unwrap()
                    .candidates
                    .len(),
                1
            );
        }
    }
}
#[test]
fn conditional_argument_return_syntax_does_not_publish_unprojected_policy() {
    let source = "fn sink(){} fn allow()->bool{sink(#[cfg(feature=\"fast\")]{return false;});true}";
    let (_, docs) = graph(&manifest("", false), source);
    assert!(docs["lib"].policies.is_empty());
    assert!(docs["lib"].binding_limitations.is_empty());
}
#[test]
fn configured_argument_expressions_filter_calls_without_blocking_the_file() {
    let source = "fn left()->u32{1} fn right()->u32{2} fn sink(_:u32){} fn after(){} fn run(){sink(#[cfg(feature=\"fast\")]left(), #[cfg(not(feature=\"fast\"))]{right()});after();}";
    for (features, present, absent) in [("'fast'", "left", "right"), ("", "right", "left")] {
        let (g, docs) = graph(&manifest(features, false), source);
        assert!(docs["lib"].binding_limitations.is_empty());
        assert!(!g.calls.iter().any(|c| c.reference.expression == absent));
        for expected in [present, "sink", "after"] {
            let call = g
                .calls
                .iter()
                .find(|c| c.reference.expression == expected)
                .unwrap();
            assert_eq!(call.candidates.len(), 1, "{call:?}");
            assert_eq!(
                g.symbols
                    .iter()
                    .find(|s| s.id == call.candidates[0])
                    .unwrap()
                    .name,
                expected
            );
        }
    }
}
#[test]
fn unknown_argument_mutations_invalidate_receiver_typing_but_disabled_ones_do_not() {
    let source = "struct A; impl A{fn method(&self){}} fn factory()->A{A} fn sink(){} fn after(){} fn run(){let mut x=factory();sink(#[cfg(target_os=\"linux\")]{x=factory();});x.method();after();}";
    for (cfg, resolved) in [
        ("", false),
        (
            "[package.metadata.rag.graph.cfg_values]\ntarget_os=['macos']\n",
            true,
        ),
    ] {
        let (g, docs) = graph(&(manifest("", false) + cfg), source);
        assert!(docs["lib"].binding_limitations.is_empty());
        let call = g
            .calls
            .iter()
            .find(|c| c.reference.expression == "x.method")
            .unwrap();
        assert_eq!(call.candidates.len(), usize::from(resolved), "{call:?}");
        assert_eq!(
            g.calls
                .iter()
                .find(|c| c.reference.expression == "after")
                .unwrap()
                .candidates
                .len(),
            1
        );
    }
}
#[test]
fn test_overlay_preserves_feature_imports_for_actual_configuration_evaluation() {
    let source = "mod left {pub fn selected(){}} mod right {pub fn selected(){}} #[cfg(feature=\"fast\")]use crate::left::selected; #[cfg(not(feature=\"fast\"))]use crate::right::selected; #[cfg(test)]fn test_run(){selected();}";
    for (features, expected) in [("'fast'", "left::selected"), ("", "right::selected")] {
        let mut docs = HashMap::new();
        for (id, path, body) in [
            ("manifest", "/p/Cargo.toml", manifest(features, false)),
            ("lib", "/p/src/lib.rs", source.into()),
        ] {
            let mut doc = parse(id, path, &body, 1).unwrap();
            doc.project = Some("fixture".into());
            docs.insert(id.into(), doc);
        }
        prepare_test_view(&mut docs);
        apply(&mut docs);
        let symbols: Vec<_> = docs.values().flat_map(|d| d.symbols.clone()).collect();
        let edges = resolve::references(&symbols, &docs);
        let call = edges
            .iter()
            .find(|c| c.reference.expression == "selected" && c.reference.is_call)
            .unwrap();
        assert_eq!(call.candidates.len(), 1, "{call:?}");
        assert_eq!(
            symbols
                .iter()
                .find(|s| s.id == call.candidates[0])
                .unwrap()
                .qualified_name,
            expected
        );
    }
}
#[test]
fn conditional_public_imports_select_only_the_active_local_binding() {
    let source = "mod left {pub fn selected(){}} mod right {pub fn selected(){}} #[cfg(feature=\"fast\")]pub use crate::left::selected; #[cfg(not(feature=\"fast\"))]pub use crate::right::selected; pub fn run(){selected();}";
    for (features, expected) in [("'fast'", "left::selected"), ("", "right::selected")] {
        let (g, docs) = graph(&manifest(features, false), source);
        let call = g
            .calls
            .iter()
            .find(|c| c.reference.expression == "selected")
            .unwrap();
        assert_eq!(call.candidates.len(), 1, "{call:?}");
        assert_eq!(
            g.symbols
                .iter()
                .find(|s| s.id == call.candidates[0])
                .unwrap()
                .qualified_name,
            expected
        );
        assert!(!docs["lib"]
            .binding_limitations
            .iter()
            .any(|s| s == "RUST_CFG_NESTED_SEMANTICS_UNMODELED"));
    }
    let unknown = "mod left {pub fn selected(){}} #[cfg(target_os=\"linux\")]pub use crate::left::selected; pub fn run(){selected();}";
    let (g, _) = graph(&manifest("", false), unknown);
    assert!(g.calls.iter().all(|c| c.candidates.is_empty()));
}
#[test]
fn conditional_attributes_preserve_inactive_items_and_gate_active_unknown_expansions() {
    let source="#![cfg_attr(feature=\"fast\", unknown_macro)] #[cfg_attr(feature=\"slow\", no_panic)] fn keep(){} fn run(){keep();}";
    let (g, _) = graph(&manifest("", false), source);
    assert!(g.symbols.iter().any(|s| s.name == "keep"));
    assert_eq!(g.calls[0].candidates.len(), 1);
    let (g, _) = graph(&manifest("'fast'", false), source);
    assert!(g.symbols.is_empty());
    assert!(g.calls.is_empty());
    let source="#[cfg_attr(feature=\"fast\", cfg(feature=\"slow\"))] fn conditional(){} #[cfg_attr(target_os=\"unknown\", inline)]fn harmless(){} #[cfg_attr(feature=\"fast\", cfg_attr(feature=\"leaf\",cfg(feature=\"slow\")))] fn nested(){}";
    let (g, _) = graph(&manifest("'fast'", false), source);
    assert!(!g
        .symbols
        .iter()
        .any(|s| s.name == "conditional" || s.name == "nested"));
    assert!(g.symbols.iter().any(|s| s.name == "harmless"));
}
#[test]
fn configured_history_and_manifest_only_switch_reuse_raw_extraction() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("configured.duckdb")).unwrap();
    let document = |id: &str, path: &str, content: &str, revision: i64| crate::models::Document {
        id: id.into(),
        uri: format!("file://{path}"),
        source_file: Some(path.into()),
        content: content.into(),
        wing: Some("fixture".into()),
        revision,
        ..Default::default()
    };
    for doc in [
        document("manifest", "/p/Cargo.toml", &manifest("'fast'", false), 1),
        document("lib", "/p/src/lib.rs", SOURCE, 1),
        document("enabled", "/p/src/enabled.rs", "pub fn leaf(){}", 1),
    ] {
        store.documents().upsert_document(&doc).unwrap();
    }
    let first = crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    let initial = crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap();
    assert!(initial.symbols.iter().any(|s| s.name == "fast"));
    assert!(!initial.symbols.iter().any(|s| s.name == "slow"));
    let before = initial.rust_build_contexts[0]["configuration_id"].clone();
    assert!(
        !crate::code_index::CodeIndex::new(&store)
            .index_code()
            .unwrap()
            .revision_created
    );
    store
        .documents()
        .upsert_document(&document(
            "manifest",
            "/p/Cargo.toml",
            &manifest("", true),
            2,
        ))
        .unwrap();
    let stale = crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap();
    assert_eq!(stale.stale_documents, ["manifest"]);
    assert!(stale.calls.iter().all(|c| c.candidates.is_empty()));
    let changed = crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    assert_eq!(changed.indexed, 1);
    assert_eq!(changed.unchanged, 2);
    let current = crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap();
    assert_ne!(current.rust_build_contexts[0]["configuration_id"], before);
    assert!(!current.symbols.iter().any(|s| s.name == "fast"));
    assert!(current.symbols.iter().any(|s| s.name == "slow"));
    assert_eq!(current.calls.len(), 1);
    assert_eq!(current.calls[0].candidates.len(), 1);
    let query = history::DiffQuery {
        before: first.index_revision.unwrap(),
        after: changed.index_revision,
        project: Some("fixture".into()),
        limit: Some(100),
    };
    let diff = crate::code_index::CodeIndex::new(&store)
        .code_index_diff(&query)
        .unwrap();
    assert!(diff["edges_added_total"].as_u64().unwrap() > 0, "{diff}");
    assert!(diff["edges_removed_total"].as_u64().unwrap() > 0, "{diff}");
    assert!(
        diff["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["symbol_key"].as_str().unwrap().contains("::build:")),
        "{diff}"
    );
    store
        .documents()
        .upsert_document(&document(
            "manifest",
            "/p/Cargo.toml",
            "[package]\nname='elsewhere'",
            3,
        ))
        .unwrap();
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .code_index_diff(&query)
            .unwrap(),
        diff
    );
}
#[test]
fn invalid_configuration_does_not_publish_permission_or_sql_facts() {
    let source = "pub fn allow()->bool{true} fn migrate(){let query=\"SELECT * FROM example\";}";
    let raw = parse("raw", "/p/src/lib.rs", source, 1).unwrap();
    assert!(!raw.policies.is_empty());
    assert!(!serde_json::to_value(&raw.sql).unwrap()["uses"]
        .as_array()
        .unwrap()
        .is_empty());
    let (_, docs) = graph(&(manifest("", false) + "feautres=[]\n"), source);
    let configured = &docs["lib"];
    assert!(configured.policies.is_empty());
    assert_eq!(
        serde_json::to_value(&configured.sql).unwrap()["uses"],
        serde_json::json!([])
    );
    assert!(configured
        .binding_limitations
        .iter()
        .any(|s| s == "RUST_BUILD_CONFIGURATION_INVALID"));
}
#[test]
fn duplicate_manifest_identity_never_reports_an_available_configuration() {
    let (_, mut docs) = graph(&manifest("'fast'", false), SOURCE);
    // Build a fresh raw view; applying configuration is not idempotent on an
    // already namespaced graph and production always starts from raw caches.
    let mut raw = parse("lib", "/p/src/lib.rs", SOURCE, 1).unwrap();
    raw.project = Some("fixture".into());
    docs.insert("lib".into(), raw);
    let duplicate = docs
        .values()
        .find(|d| d.path == "/p/Cargo.toml")
        .unwrap()
        .clone();
    docs.insert("duplicate".into(), duplicate);
    let applied = apply(&mut docs);
    assert_eq!(applied.contexts.len(), 2);
    assert!(applied
        .contexts
        .iter()
        .all(|c| c["available"] == false && c["reason"] == "manifest_ambiguous"));
    assert!(docs["lib"]
        .binding_limitations
        .iter()
        .any(|s| s == "RUST_BUILD_MANIFEST_AMBIGUOUS"));
}
