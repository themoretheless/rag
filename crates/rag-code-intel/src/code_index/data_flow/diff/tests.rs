use super::*;
fn frozen(sources: &[(&str, &str, &str)]) -> (Value, Vec<File>) {
    let mut docs = HashMap::new();
    let mut files = Vec::new();
    let mut symbols = Vec::new();
    for (i, (project, path, source)) in sources.iter().enumerate() {
        let id = i.to_string();
        let mut doc = parse(&id, path, source, 1).unwrap();
        doc.project = Some((*project).into());
        symbols.extend(doc.symbols.clone());
        files.push(File {
            id: id.clone(),
            path: (*path).into(),
            project: doc.project.clone(),
            revision: 1,
            hash: content_hash(source),
            source: (*source).into(),
            analysis: Some(serde_json::to_string(&doc).unwrap()),
        });
        docs.insert(id, doc);
    }
    let refs = resolve::references(&symbols, &docs);
    (
        json!({"data_flow_format":7,"data_flow":snapshot(&docs),"calls":refs.iter().filter(|r|r.reference.is_call).collect::<Vec<_>>()}),
        files,
    )
}
fn totals(result: &Value) -> (u64, u64, u64) {
    (
        result["nodes_changed_total"].as_u64().unwrap(),
        result["edges_added_total"].as_u64().unwrap(),
        result["edges_removed_total"].as_u64().unwrap(),
    )
}
#[test]
fn formatting_comments_and_nfkc_spelling_do_not_change_normalized_values() {
    let (a, fa) = frozen(&[(
        "p",
        "/p/a.py",
        "def entry(x):\n    result = x + 1\n    return result\n",
    )]);
    let (b, fb) = frozen(&[(
        "p",
        "/p/a.py",
        "# prefix shifts all byte positions\ndef entry(𝒙):\n    # ignore this comment\n    result  =  𝒙 + 1  # another comment\n    return result\n",
    )]);
    let r = compare(&a, &b, &fa, &fb, 20);
    assert_eq!(r["available"], true, "{r}");
    assert_eq!(totals(&r), (0, 0, 0), "{r}");
}
#[test]
fn literal_and_operator_changes_keep_topology_but_change_node_fingerprints() {
    let (a, fa) = frozen(&[("p", "/p/a.py", "def entry(x):\n    return x + 1\n")]);
    for source in [
        "def entry(x):\n    return x + 2\n",
        "def entry(x):\n    return x - 1\n",
    ] {
        let (b, fb) = frozen(&[("p", "/p/a.py", source)]);
        let r = compare(&a, &b, &fa, &fb, 20);
        assert_eq!(r["available"], true, "{r}");
        let (nodes, added, removed) = totals(&r);
        assert!(nodes > 0, "{r}");
        assert_eq!((added, removed), (0, 0), "{r}");
        assert_eq!(r["source_semantic_change_proven"], false);
    }
}
#[test]
fn reassignment_and_import_redirect_change_actual_candidate_edges() {
    let (a, fa) = frozen(&[(
        "p",
        "/p/a.py",
        "def entry(left, right):\n    result = left\n    result = right\n    return result\n",
    )]);
    let (b, fb) = frozen(&[(
        "p",
        "/p/a.py",
        "def entry(left, right):\n    result = right\n    result = left\n    return result\n",
    )]);
    let r = compare(&a, &b, &fa, &fb, 20);
    let (_, added, removed) = totals(&r);
    assert!(added > 0 && removed > 0, "{r}");
    let lib = "def first(value):\n    return value\ndef second(value):\n    return value\n";
    let (a, fa) = frozen(&[
        ("p", "/p/lib.py", lib),
        (
            "p",
            "/p/a.py",
            "from lib import first as chosen\ndef entry(x):\n    return chosen(x)\n",
        ),
    ]);
    let (b, fb) = frozen(&[
        ("p", "/p/lib.py", lib),
        (
            "p",
            "/p/a.py",
            "from lib import second as chosen\ndef entry(x):\n    return chosen(x)\n",
        ),
    ]);
    let r = compare(&a, &b, &fa, &fb, 20);
    let (nodes, added, removed) = totals(&r);
    assert_eq!(nodes, 0, "{r}");
    assert_eq!((added, removed), (2, 2), "{r}");
    assert!(
        r["edges_added"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["edge"]["relation"] == "call_argument")
    );
}
#[test]
fn unsupported_function_transitions_are_coverage_gaps_not_deleted_value_edges() {
    let (a, fa) = frozen(&[("p", "/p/a.py", "def entry(x):\n    return x\n")]);
    let (b, fb) = frozen(&[(
        "p",
        "/p/a.py",
        "def entry(x):\n    try:\n        return x\n    except build_type():\n        return 0\n",
    )]);
    let r = compare(&a, &b, &fa, &fb, 20);
    assert_eq!(r["available"], true, "{r}");
    assert_eq!(totals(&r), (0, 0, 0), "{r}");
    assert_eq!(r["excluded_functions_total"], 1);
    assert_eq!(r["complete_for_captured_scope"], false);
}
#[test]
fn legacy_partial_corrupt_and_foreign_captures_cannot_prove_deletions() {
    let (a, fa) = frozen(&[("p", "/p/a.py", "def entry(x):\n    return x\n")]);
    let mut b = a.clone();
    b["data_flow_format"] = 4.into();
    let r = compare(&a, &b, &fa, &fa, 20);
    assert_eq!(r["available"], false);
    assert!(r.get("nodes_changed_total").is_none());
    for changed in [json!(null), json!([])] {
        let mut b = a.clone();
        b["data_flow"]["documents"] = changed;
        let r = compare(&a, &b, &fa, &fa, 20);
        if r["available"] == true {
            assert_eq!(r["excluded_documents_total"], 1);
            assert_eq!(totals(&r), (0, 0, 0));
        } else {
            assert!(r.get("nodes_changed_total").is_none());
        }
    }
    let mut b = a.clone();
    b["data_flow"]["documents"][0]["facts"]["functions"][0]["nodes"][0]["start_byte"] =
        99999.into();
    let r = compare(&a, &b, &fa, &fa, 20);
    assert_eq!(r["excluded_documents_total"], 1, "{r}");
    assert_eq!(totals(&r), (0, 0, 0));
    let mut b = a.clone();
    b["data_flow"]["documents"]
        .as_array_mut()
        .unwrap()
        .push(json!({"document_id":"foreign","invalid":true}));
    assert_eq!(totals(&compare(&a, &b, &fa, &fa, 20)), (0, 0, 0));
}
#[test]
fn function_addition_removal_and_output_limits_remain_visible() {
    let (a, fa) = frozen(&[("p", "/p/a.py", "def first(x):\n    return x\n")]);
    let (b, fb) = frozen(&[("p", "/p/a.py", "def second(y):\n    return y + 1\n")]);
    let r = compare(&a, &b, &fa, &fb, 1);
    let (n, a, d) = totals(&r);
    assert!(n > 1 && a > 1 && d > 1, "{r}");
    assert_eq!(r["truncated"], true);
    assert_eq!(r["nodes_changed"].as_array().unwrap().len(), 1);
}
#[test]
fn exhausted_normalization_work_is_explicit() {
    let (a, fa) = frozen(&[("p", "/p/a.py", "def entry(x):\n    return x\n")]);
    let mut budget = Budget { used: BUDGET };
    let m = model(&a, &fa, &mut budget).unwrap();
    assert_eq!(
        m.documents["0"].as_deref(),
        Some("normalization_budget_exhausted")
    );
    assert!(m.nodes.is_empty());
    assert_eq!(budget.used, BUDGET);
}

#[test]
fn frozen_call_provenance_mismatch_is_unavailable_not_an_edge_deletion() {
    let (a, fa) = frozen(&[(
        "p",
        "/p/a.py",
        "def identity(x):\n    return x\ndef entry(value):\n    return identity(value)\n",
    )]);
    let mut b = a.clone();
    b["calls"][0]["reference"]["content_hash"] = "wrong".into();
    let r = compare(&a, &b, &fa, &fa, 20);
    assert_eq!(r["available"], false, "{r}");
    assert_eq!(r["reason"], "mismatched_frozen_call_provenance");
    assert!(r.get("edges_removed_total").is_none());
}

#[test]
fn omitted_function_inventory_cannot_look_like_a_source_deletion() {
    let (a, fa) = frozen(&[("p", "/p/a.py", "def entry(x):\n    return x\n")]);
    let mut b = a.clone();
    b["data_flow"]["documents"][0]["facts"]["functions"] = json!([]);
    let r = compare(&a, &b, &fa, &fa, 20);
    assert_eq!(r["available"], true, "{r}");
    assert_eq!(r["excluded_documents_total"], 1);
    assert_eq!(totals(&r), (0, 0, 0));
}
#[test]
fn legacy_plain_capture_five_compares_with_six_without_fabricated_changes() {
    let (a, fa) = frozen(&[("p", "/p/a.py", "def entry(x):\n    return x + 1\n")]);
    let mut old = a.clone();
    old["data_flow_format"] = 5.into();
    old["data_flow"]["documents"][0]["facts"]["format"] = 5.into();
    let r = compare(&old, &a, &fa, &fa, 20);
    assert_eq!(r["available"], true, "{r}");
    assert_eq!(totals(&r), (0, 0, 0));
    assert_eq!(r["normalization_format"], 3);
}
#[test]
fn frozen_finally_contexts_normalize_without_position_churn() {
    let source = "def entry(first, second):\n    value = first\n    try:\n        value = second + 1\n    finally:\n        return value\n";
    let (a, fa) = frozen(&[("p", "/p/a.py", source)]);
    let shifted = format!("# prefix\n{source}");
    let (b, fb) = frozen(&[("p", "/p/a.py", &shifted)]);
    let r = compare(&a, &b, &fa, &fb, 100);
    assert_eq!(r["available"], true, "{r}");
    assert_eq!(r["excluded_documents_total"], 0, "{r}");
    assert_eq!(totals(&r), (0, 0, 0), "{r}");
}
#[test]
fn changed_context_capture_formats_are_excluded_instead_of_analysis_enrichment_deltas() {
    let source = "def entry(first, second):\n    value = first\n    try:\n        value = second + 1\n    finally:\n        return value\n";
    let (a, fa) = frozen(&[("p", "/p/a.py", source)]);
    let mut old = a.clone();
    old["data_flow_format"] = 6.into();
    old["data_flow"]["documents"][0]["facts"]["format"] = 6.into();
    let r = compare(&old, &a, &fa, &fa, 20);
    assert_eq!(r["available"], true, "{r}");
    assert_eq!(totals(&r), (0, 0, 0));
    assert_eq!(r["excluded_functions_total"], 1);
    assert_eq!(
        r["excluded_functions"][0]["capture_context_format_changed"],
        true
    );
}
#[test]
fn handler_context_graphs_ignore_source_position_only_changes() {
    let source = "def entry(problem):\n    try:\n        raise problem\n    except Exception as err:\n        return err\n";
    let (a, fa) = frozen(&[("p", "/p/a.py", source)]);
    let shifted = format!("# prefix\n{source}");
    let (b, fb) = frozen(&[("p", "/p/a.py", &shifted)]);
    let r = compare(&a, &b, &fa, &fb, 50);
    assert_eq!(r["available"], true, "{r}");
    assert_eq!(r["excluded_documents_total"], 0, "{r}");
    assert_eq!(totals(&r), (0, 0, 0), "{r}");
}
