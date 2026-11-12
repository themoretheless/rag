use super::*;
pub(super) fn graph(files: &[(&str, &str, &str)]) -> CodeSnapshot {
    let mut docs = HashMap::new();
    let mut out = CodeSnapshot::default();
    for (i, (project, path, body)) in files.iter().enumerate() {
        let id = i.to_string();
        let mut d = parse(&id, path, body, 1).unwrap();
        assert!(!d.parse_errors, "{path}");
        d.project = Some((*project).into());
        out.document_scopes.insert(
            id.clone(),
            DocumentScope {
                path: (*path).into(),
                project: d.project.clone(),
            },
        );
        out.symbols.extend(d.symbols.clone());
        docs.insert(id, d);
    }
    out.bound_references = resolve::references(&out.symbols, &docs);
    out.calls = out
        .bound_references
        .iter()
        .filter(|e| e.reference.is_call)
        .cloned()
        .collect();
    out.data_flow = snapshot(&docs);
    out
}
pub(super) fn q(symbol: &str) -> DataSliceQuery {
    serde_json::from_value(json!({"symbol":symbol})).unwrap()
}
pub(super) fn variables(result: &Value) -> BTreeSet<String> {
    result["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["node"]["name"].as_str().map(str::to_string))
        .collect()
}
#[test]
fn python_value_slice_uses_latest_definitions_and_exact_unicode_spans() {
    let source =
        "def result(first, second):\n    café = first\n    café = second + 1\n    return café\n";
    let s = graph(&[("p", "/p/a.py", source)]);
    let result = query::query(&s, &q("result"));
    assert_eq!(result["status"], "data_slice", "{result}");
    let names = variables(&result);
    assert!(
        names.contains("second") && names.contains("café") && !names.contains("first"),
        "{result}"
    );
    assert!(
        result["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["relation"] == "def_use")
    );
    for item in result["items"].as_array().unwrap() {
        let n = &item["node"];
        let span = &source
            [n["start_byte"].as_u64().unwrap() as usize..n["end_byte"].as_u64().unwrap() as usize];
        if let Some(name) = n["name"].as_str() {
            assert_eq!(span, name);
        }
        assert_eq!(item["source"]["content_hash"], content_hash(source));
        assert_eq!(item["witness"]["complete_to_seed"], true);
    }
    let mut ambiguous = q("result");
    ambiguous.variable = Some("café".into());
    assert_eq!(
        query::query(&s, &ambiguous)["status"],
        "ambiguous_local_definition"
    );
    ambiguous.start_byte = Some(source.rfind("café =").unwrap());
    assert_eq!(query::query(&s, &ambiguous)["status"], "data_slice");
}
#[test]
fn python_argument_return_frames_do_not_leak_other_call_sites() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def identity(value):\n    return value\ndef left(alpha):\n    result = identity(alpha)\n    return result\ndef right(beta):\n    result = identity(beta)\n    return result\n",
    )]);
    let result = query::query(&s, &q("left"));
    assert_eq!(result["status"], "data_slice", "{result}");
    let names = variables(&result);
    assert!(
        names.contains("alpha") && names.contains("value") && !names.contains("beta"),
        "{result}"
    );
    for kind in ["call_argument", "return_to_call"] {
        assert!(
            result["edges"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["relation"] == kind),
            "{result}"
        );
    }
    let mut forward = q("left");
    forward.direction = Some("forward".into());
    let forward = query::query(&s, &forward);
    assert!(!variables(&forward).contains("beta"));
    let left = s.symbols.iter().find(|s| s.name == "left").unwrap();
    let right = s.symbols.iter().find(|s| s.name == "right").unwrap();
    assert!(
        forward["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["node"]["function"] == left.id && i["node"]["kind"] == "return")
    );
    assert!(
        !forward["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["node"]["function"] == right.id),
        "{forward}"
    );
}
#[test]
fn python_constant_return_does_not_invent_argument_influence() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def constant(unused):\n    return 7\ndef entry(input):\n    output = constant(input)\n    return output\n",
    )]);
    let result = query::query(&s, &q("entry"));
    assert!(!variables(&result).contains("input"), "{result}");
    assert!(
        result["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["relation"] == "return_to_call")
    );
}
#[test]
fn python_imported_functions_keep_project_boundaries_and_unresolved_guards() {
    let s = graph(&[
        ("p", "/p/lib.py", "def work(value):\n    return value + 1\n"),
        (
            "p",
            "/p/app.py",
            "from lib import work as chosen\ndef entry(input):\n    return chosen(input)\n",
        ),
        ("q", "/q/lib.py", "def work(foreign):\n    return foreign\n"),
    ]);
    let result = query::query(&s, &q("entry"));
    assert!(variables(&result).contains("value"), "{result}");
    assert!(
        result["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["source"]["project"] == "p")
    );
    for body in [
        "def entry(x):\n    try:\n        return x\n    except build_type():\n        return 0\n",
        "def entry(x=1):\n    return x\n",
        "def entry(x):\n    y, z = x\n    return y\n",
        "def entry(x):\n    return unknown_global\n",
        "def entry(x):\n    f\"{mutate()}\"\n    return x\n",
        "async\tdef entry(x):\n    return x\n",
    ] {
        let s = graph(&[("p", "/p/a.py", body)]);
        assert_eq!(
            query::query(&s, &q("entry"))["status"],
            "unsupported_function",
            "{body}"
        );
    }
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def work(x):\n    return x\nwork = replacement\ndef entry(input):\n    return work(input)\n",
    )]);
    let result = query::query(&s, &q("entry"));
    assert!(
        result["call_boundary_diagnostics_total"].as_u64().unwrap() > 0,
        "{result}"
    );
    assert!(
        !result["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["relation"] == "call_argument")
    );
}
#[test]
fn python_data_slice_budgets_are_independent_and_stale_sources_are_excluded() {
    let mut s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(input):\n    first = input + 1\n    second = first * 2\n    return second\n",
    )]);
    for budget in [1, 5, 10, 20, 50, 100] {
        let mut q = q("entry");
        q.max_work = Some(budget);
        let result = query::query(&s, &q);
        assert!(
            result["work"].as_u64().unwrap() <= budget as u64,
            "{result}"
        );
    }
    let mut capped = q("entry");
    capped.limit = Some(1);
    assert_eq!(query::query(&s, &capped)["result_limit_reached"], true);
    capped.limit = None;
    capped.depth = Some(1);
    assert_eq!(query::query(&s, &capped)["depth_limit_reached"], true);
    s.data_flow.documents.clear();
    s.stale_documents.push("0".into());
    assert_ne!(query::query(&s, &q("entry"))["status"], "data_slice");
}

#[test]
fn python_value_facts_persist_repair_and_freeze_with_explicit_diff_availability() {
    use crate::models::Document;
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("flow.duckdb")).unwrap();
    let doc = |body: &str| Document {
        id: "flow".into(),
        uri: "file:///p/a.py".into(),
        source_file: Some("/p/a.py".into()),
        wing: Some("p".into()),
        content: body.into(),
        ..Default::default()
    };
    store
        .upsert_document(&doc("def entry(value):\n    return value + 1\n"))
        .unwrap();
    let before = store.index_code().unwrap().index_revision.unwrap();
    let mut query = q("entry");
    query.project = Some("p".into());
    let first = store.code_data_slice(&query).unwrap();
    assert_eq!(first["status"], "data_slice");
    assert_eq!(first, store.code_data_slice(&query).unwrap());
    assert_eq!(store.index_code().unwrap().unchanged, 1);
    let payload = crate::code_index::test_get_code_doc_payload(&store, "flow");
    let mut old: Value = serde_json::from_str(&payload).unwrap();
    old.as_object_mut().unwrap().remove("data_flow");
    crate::code_index::test_set_code_doc_payload(&store, "flow", &old.to_string());
    assert_ne!(
        store.code_data_slice(&query).unwrap()["status"],
        "data_slice"
    );
    assert_eq!(store.index_code().unwrap().indexed, 1);
    let mut legacy: Value = serde_json::from_str(&payload).unwrap();
    legacy["data_flow"]["format"] = json!(6);
    crate::code_index::test_set_code_doc_payload(&store, "flow", &legacy.to_string());
    assert_ne!(
        store.code_data_slice(&query).unwrap()["status"],
        "data_slice"
    );
    assert_eq!(store.index_code().unwrap().indexed, 1);
    assert_eq!(
        store.code_data_slice(&query).unwrap()["items"],
        first["items"]
    );
    store
        .upsert_document(&doc("def entry(other):\n    return other * 2\n"))
        .unwrap();
    let stale = store.code_data_slice(&query).unwrap();
    assert_ne!(stale["status"], "data_slice");
    assert!(!stale["stale_documents"].as_array().unwrap().is_empty());
    let after = store.index_code().unwrap().index_revision.unwrap();
    let refreshed = store.code_data_slice(&query).unwrap();
    assert!(variables(&refreshed).contains("other"));
    let diff = store
        .code_index_diff(&history::DiffQuery {
            before,
            after: Some(after),
            project: Some("p".into()),
            limit: None,
        })
        .unwrap();
    assert_eq!(diff["data_flow_comparison"]["available"], true);
    assert!(
        diff["data_flow_comparison"]["nodes_changed_total"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(
        diff["data_flow_comparison"]["before_capture_available"],
        true
    );
    assert_eq!(
        diff["data_flow_comparison"]["after_capture_available"],
        true
    );
}

#[test]
fn python_rebound_export_does_not_supply_an_interprocedural_value_target() {
    let s = graph(&[
        (
            "p",
            "/p/lib.py",
            "def work(value):\n    return value\nwork = replacement\n",
        ),
        (
            "p",
            "/p/app.py",
            "from lib import work\ndef entry(input):\n    return work(input)\n",
        ),
    ]);
    let result = query::query(&s, &q("entry"));
    assert_eq!(result["call_boundary_diagnostics_total"], 1, "{result}");
    assert!(!variables(&result).contains("value"));
    assert_eq!(result["qualified_subset_traversal_complete"], false);
}

#[test]
fn python_unimported_foreign_module_does_not_supply_a_value_target() {
    let s = graph(&[
        ("p", "/p/lib.py", "def missing(value):\n    return value\n"),
        (
            "p",
            "/p/app.py",
            "def entry(input):\n    return missing(input)\n",
        ),
    ]);
    let result = query::query(&s, &q("entry"));
    assert!(
        !result["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["relation"] == "call_argument"),
        "{result}"
    );
}

#[test]
fn python_nfkc_binding_identity_preserves_source_spelling_and_latest_write() {
    let source = "def entry(x):\n    𝒙 = 7\n    return x\n";
    let s = graph(&[("p", "/p/a.py", source)]);
    let result = query::query(&s, &q("entry"));
    assert_eq!(result["status"], "data_slice", "{result}");
    assert!(
        !result["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["node"]["kind"] == "parameter"),
        "{result}"
    );
    let definition = result["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["node"]["kind"] == "definition")
        .unwrap();
    assert_eq!(definition["node"]["name"], "𝒙");
    assert_eq!(definition["node"]["binding_name"], "x");
    let mut selected = q("entry");
    selected.variable = Some("x".into());
    assert_eq!(
        query::query(&s, &selected)["status"],
        "ambiguous_local_definition"
    );
    selected.start_byte = Some(source.find("𝒙").unwrap());
    assert_eq!(query::query(&s, &selected)["status"], "data_slice");
    let shadowed = graph(&[(
        "p",
        "/p/a.py",
        "def f(value):\n    return value\ndef shadowed(𝒇, x):\n    return f(x)\n",
    )]);
    assert_eq!(
        query::query(&shadowed, &q("shadowed"))["status"],
        "unsupported_function"
    );
}

#[test]
fn branch_merges_include_both_reaching_values_and_predicates() {
    let source = "def entry(flag, left, right):\n    if flag:\n        value = left\n    else:\n        𝒗alue = right\n    return value\n";
    let s = graph(&[("p", "/p/a.py", source)]);
    let r = query::query(&s, &q("entry"));
    assert_eq!(r["status"], "data_slice", "{r}");
    let names = variables(&r);
    for name in ["flag", "left", "right", "value", "𝒗alue"] {
        assert!(names.contains(name), "{name}: {r}");
    }
    for kind in ["merge_input", "control_dependency"] {
        assert!(
            r["edges"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["relation"] == kind),
            "{r}"
        );
    }
    for item in r["items"].as_array().unwrap() {
        let n = &item["node"];
        if let Some(name) = n["name"].as_str() {
            assert_eq!(
                &source[n["start_byte"].as_u64().unwrap() as usize
                    ..n["end_byte"].as_u64().unwrap() as usize],
                name
            );
        }
    }
    assert_eq!(r["branch_path_feasibility_proven"], false);
    assert_eq!(r["branch_control_dependencies"], true);
}
#[test]
fn elif_and_returning_arms_keep_only_live_definitions() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(first, second, a, b, c):\n    if first:\n        value = a\n        return 7\n    elif second:\n        value = b\n    else:\n        value = c\n    return value\n",
    )]);
    let r = query::query(&s, &q("entry"));
    assert_eq!(r["status"], "data_slice", "{r}");
    let names = variables(&r);
    assert!(!names.contains("a"), "{r}");
    for name in ["first", "second", "b", "c"] {
        assert!(names.contains(name), "{name}: {r}");
    }
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(flag, value):\n    if flag:\n        return 1\n    else:\n        return value\n",
    )]);
    assert_eq!(query::query(&s, &q("entry"))["status"], "data_slice");
}
#[test]
fn branch_initialization_and_unrelated_controls_are_not_invented() {
    for source in [
        "def entry(flag, value):\n    if flag:\n        x = value\n    return x\n",
        "def entry(flag, value):\n    if flag:\n        x = value\n    else:\n        x = 3\n        y = value\n    return y\n",
    ] {
        let s = graph(&[("p", "/p/a.py", source)]);
        assert_eq!(
            query::query(&s, &q("entry"))["status"],
            "unsupported_function"
        );
    }
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(flag, value):\n    if flag:\n        unused = value\n    else:\n        unused = 7\n    return 9\n",
    )]);
    let r = query::query(&s, &q("entry"));
    assert_eq!(r["status"], "data_slice", "{r}");
    assert!(variables(&r).is_empty(), "{r}");
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(flag, old, new):\n    value = old\n    if flag:\n        value = new\n    return value\n",
    )]);
    let r = query::query(&s, &q("entry"));
    for name in ["flag", "old", "new"] {
        assert!(variables(&r).contains(name), "{r}");
    }
}
#[test]
fn deeply_nested_branches_fail_with_a_bounded_capture() {
    let mut source = "def entry(flag):\n".to_string();
    for n in 1..36 {
        source.push_str(&format!("{}if flag:\n", "    ".repeat(n)));
    }
    source.push_str(&format!("{}return flag\n", "    ".repeat(36)));
    source.push_str("    return 0\n");
    let s = graph(&[("p", "/p/a.py", &source)]);
    let r = query::query(&s, &q("entry"));
    assert_eq!(r["status"], "unsupported_function", "{r}");
    assert!(
        r["reason"].as_str().unwrap().contains("depth_exhausted"),
        "{r}"
    );
}

#[test]
fn nested_branch_merges_and_all_returning_paths_preserve_reaching_values() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(outer, inner, a, b, c):\n    if outer:\n        if inner:\n            value = a\n        else:\n            value = b\n    else:\n        value = c\n    return value\n",
    )]);
    let r = query::query(&s, &q("entry"));
    assert_eq!(r["status"], "data_slice", "{r}");
    for name in ["outer", "inner", "a", "b", "c"] {
        assert!(variables(&r).contains(name), "{name}: {r}");
    }
    assert_eq!(
        r["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["node"]["kind"] == "merge")
            .count(),
        2
    );
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(flag, left, right):\n    if flag:\n        return left\n    else:\n        return right\n",
    )]);
    let r = query::query(&s, &q("entry"));
    assert_eq!(r["status"], "data_slice", "{r}");
    assert_eq!(r["seeds"].as_array().unwrap().len(), 2);
    for name in ["flag", "left", "right"] {
        assert!(variables(&r).contains(name), "{r}");
    }
}

#[test]
fn while_headers_preserve_entry_backedge_and_zero_iterations() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(flag, first, next_value):\n    value = first\n    while flag:\n        value = next_value\n        flag = 0\n    return value\n",
    )]);
    let r = query::query(&s, &q("entry"));
    assert_eq!(r["status"], "data_slice", "{r}");
    for name in ["flag", "first", "next_value"] {
        assert!(variables(&r).contains(name), "{name}: {r}");
    }
    for kind in ["loop_entry", "loop_backedge"] {
        assert!(
            r["edges"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["relation"] == kind),
            "{r}"
        );
    }
    assert_eq!(r["termination_proven"], false);
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(flag, value):\n    while flag:\n        initialized = value\n    return initialized\n",
    )]);
    assert_eq!(
        query::query(&s, &q("entry"))["status"],
        "unsupported_function"
    );
}
#[test]
fn loop_break_and_continue_keep_their_local_states() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(flag, stop, skip, initial, first, second):\n    value = initial\n    while flag:\n        if stop:\n            value = first\n            break\n        if skip:\n            value = second\n            continue\n        value = 7\n    return value\n",
    )]);
    let mut query = q("entry");
    query.depth = Some(30);
    query.limit = Some(500);
    let r = query::query(&s, &query);
    assert_eq!(r["status"], "data_slice", "{r}");
    for name in ["flag", "stop", "skip", "initial", "first", "second"] {
        assert!(variables(&r).contains(name), "{name}: {r}");
    }
    assert!(
        r["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["node"]["kind"] == "loop_exit"),
        "{r}"
    );
    for budget in [1, 10, 50, 200, 1000] {
        query.max_work = Some(budget);
        assert!(query::query(&s, &query)["work"].as_u64().unwrap() <= budget as u64);
    }
}
#[test]
fn loop_else_skips_breaks_but_captures_normal_exits() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(flag, stop, initial, broken, normal):\n    value = initial\n    while flag:\n        if stop:\n            value = broken\n            break\n        flag = 0\n    else:\n        value = normal\n    return value\n",
    )]);
    let r = query::query(&s, &q("entry"));
    assert_eq!(r["status"], "data_slice", "{r}");
    for name in ["broken", "normal", "stop", "flag"] {
        assert!(variables(&r).contains(name), "{name}: {r}");
    }
    assert!(!variables(&r).contains("initial"), "{r}");
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(flag, returned, broken):\n    while flag:\n        break\n    else:\n        return returned\n    return broken\n",
    )]);
    let r = query::query(&s, &q("entry"));
    assert_eq!(r["seeds"].as_array().unwrap().len(), 2, "{r}");
}
#[test]
fn for_iteration_values_are_abstract_and_iterables_are_evaluated_once() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(values, initial, replacement):\n    result = initial\n    for 𝒊tem in values:\n        values = replacement\n        result = item\n    return result\n",
    )]);
    let r = query::query(&s, &q("entry"));
    assert_eq!(r["status"], "data_slice", "{r}");
    for name in ["values", "initial", "𝒊tem", "item"] {
        assert!(variables(&r).contains(name), "{name}: {r}");
    }
    assert!(!variables(&r).contains("replacement"), "{r}");
    assert!(
        r["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["relation"] == "iteration_value"),
        "{r}"
    );
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(values):\n    for item in values:\n        pass\n    return item\n",
    )]);
    assert_eq!(
        query::query(&s, &q("entry"))["status"],
        "unsupported_function"
    );
}
#[test]
fn nested_loops_keep_jumps_in_the_innermost_body() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(outer, inner, initial, left, right):\n    value = initial\n    while outer:\n        while inner:\n            value = left\n            break\n        value = right\n        break\n    return value\n",
    )]);
    let r = query::query(&s, &q("entry"));
    assert_eq!(r["status"], "data_slice", "{r}");
    let names = variables(&r);
    assert!(names.contains("right") && names.contains("initial"), "{r}");
    assert!(!names.contains("left"), "{r}");
}
#[test]
fn loop_return_paths_cannot_supply_backedge_definitions() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(flag, stop, initial, dead, live):\n    value = initial\n    while flag:\n        if stop:\n            value = dead\n            return 0\n        value = live\n    return value\n",
    )]);
    let r = query::query(&s, &q("entry"));
    assert_eq!(r["status"], "data_slice", "{r}");
    assert!(!variables(&r).contains("dead"), "{r}");
    for name in ["initial", "live", "flag", "stop"] {
        assert!(variables(&r).contains(name), "{r}");
    }
}

#[test]
fn loop_cycles_are_visible_with_checked_transition_contexts() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def entry(flag, value):\n    while flag:\n        value = value + 1\n    return value\n",
    )]);
    let r = query::query(&s, &q("entry"));
    assert_eq!(
        r["edge_scope"],
        "one_checked_reachable_context_per_physical_edge"
    );
    let edges = r["edges"].as_array().unwrap();
    assert!(edges.len() <= 2000);
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for edge in edges {
        adjacency
            .entry(edge["source"].as_str().unwrap().into())
            .or_default()
            .push(edge["target"].as_str().unwrap().into());
        assert_eq!(edge["transition"]["source_state_reachable"], true);
        assert_eq!(
            edge["transition"]["from_stack"],
            edge["transition"]["to_stack"]
        );
    }
    let header = r["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["node"]["kind"] == "loop_header")
        .unwrap()["node"]["id"]
        .as_str()
        .unwrap();
    let mut work = adjacency[header].clone();
    let mut seen = HashSet::new();
    let mut cycle = false;
    while let Some(node) = work.pop() {
        if node == header {
            cycle = true;
            break;
        }
        if seen.insert(node.clone()) {
            work.extend(adjacency.get(&node).into_iter().flatten().cloned());
        }
    }
    assert!(cycle, "{r}");
}

#[test]
fn dense_control_edges_have_an_explicit_independent_response_cap() {
    let parameters = (0..24)
        .map(|n| format!("p{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut source = format!("def entry(value, {parameters}):\n");
    for n in 0..24 {
        source.push_str(&format!("{}if p{n}:\n", "    ".repeat(n + 1)));
    }
    for _ in 0..40 {
        source.push_str(&format!("{}value = value + 1\n", "    ".repeat(25)));
    }
    source.push_str("    return value\n");
    let s = graph(&[("p", "/p/a.py", &source)]);
    let mut query = q("entry");
    query.direction = Some("forward".into());
    query.depth = Some(30);
    query.limit = Some(500);
    query.max_work = Some(200000);
    let r = query::query(&s, &query);
    assert_eq!(r["status"], "data_slice", "{r}");
    assert_eq!(r["edge_limit_reached"], true, "{r}");
    assert_eq!(r["edges"].as_array().unwrap().len(), 2000);
    assert_eq!(r["qualified_subset_traversal_complete"], false);
}
