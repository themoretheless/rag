use super::tests::{graph, q, variables};
use super::*;
fn flow(source: &str) -> Value {
    query::query(&graph(&[("p", "/p/a.py", source)]), &q("entry"))
}
#[test]
fn finally_assignment_does_not_rewrite_an_already_selected_return_value() {
    let r=flow("def entry(first, second):\n    value = first\n    try:\n        return value\n    finally:\n        value = second\n");
    assert_eq!(r["status"], "data_slice", "{r}");
    assert!(variables(&r).contains("first"), "{r}");
    assert!(!variables(&r).contains("second"), "{r}");
}
#[test]
fn unconditional_finally_return_overrides_saved_values_and_exception_inputs() {
    for source in ["def entry(first, second):\n    try:\n        return first\n    finally:\n        return second\n", "def entry(first, second):\n    try:\n        return first + 1\n    finally:\n        return second\n"] {
        let r=flow(source);assert_eq!(r["status"],"data_slice","{r}");assert!(variables(&r).contains("second"),"{r}");assert!(!variables(&r).contains("first"),"{r}");
    }
}
#[test]
fn nested_finally_reads_updated_locals_but_keeps_pending_returns_distinct() {
    let r=flow("def entry(first, second):\n    value = first\n    try:\n        try:\n            return value\n        finally:\n            value = second\n    finally:\n        return value\n");
    assert_eq!(r["status"], "data_slice", "{r}");
    assert!(variables(&r).contains("second"), "{r}");
    assert!(!variables(&r).contains("first"), "{r}");
}
#[test]
fn finally_updates_break_and_continue_states_before_loop_merging() {
    let r=flow("def entry(flag, initial, replacement):\n    value = initial\n    while flag:\n        try:\n            break\n        finally:\n            value = replacement\n    return value\n");
    assert_eq!(r["status"], "data_slice", "{r}");
    for name in ["initial", "replacement"] {
        assert!(variables(&r).contains(name), "{r}");
    }
    let r=flow("def entry(flag, initial, overwritten, replacement):\n    value = initial\n    while flag:\n        try:\n            value = overwritten\n            continue\n        finally:\n            value = replacement\n    return value\n");
    assert_eq!(r["status"], "data_slice", "{r}");
    for name in ["initial", "replacement"] {
        assert!(variables(&r).contains(name), "{r}");
    }
    assert!(!variables(&r).contains("overwritten"), "{r}");
}
#[test]
fn finally_break_overrides_a_function_return() {
    let r=flow("def entry(flag, discarded, result):\n    while flag:\n        try:\n            return discarded\n        finally:\n            break\n    return result\n");
    assert_eq!(r["status"], "data_slice", "{r}");
    assert!(variables(&r).contains("result"), "{r}");
    assert!(!variables(&r).contains("discarded"), "{r}");
}
#[test]
fn conditional_finally_return_retains_the_non_overridden_completion() {
    let r=flow("def entry(flag, first, second):\n    try:\n        return first\n    finally:\n        if flag:\n            return second\n");
    assert_eq!(r["status"], "data_slice", "{r}");
    for name in ["flag", "first", "second"] {
        assert!(variables(&r).contains(name), "{r}");
    }
}
#[test]
fn possible_rhs_failure_preserves_the_pre_assignment_value_for_finally() {
    let r=flow("def entry(initial, numerator, denominator):\n    value = initial\n    try:\n        value = numerator / denominator\n    finally:\n        return value\n");
    assert_eq!(r["status"], "data_slice", "{r}");
    for name in ["initial", "numerator", "denominator"] {
        assert!(variables(&r).contains(name), "{r}");
    }
    assert!(
        r["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["relation"] == "completion_value"),
        "{r}"
    );
    assert_eq!(r["full_exception_semantics"], false);
}
#[test]
fn finally_constant_is_not_controlled_by_an_ignored_pending_exception() {
    let r = flow(
        "def entry(input):\n    try:\n        value = input / 0\n    finally:\n        return 7\n",
    );
    assert_eq!(r["status"], "data_slice", "{r}");
    assert!(variables(&r).is_empty(), "{r}");
}
#[test]
fn explicit_raise_can_be_overridden_by_finally_return() {
    let r=flow("def entry(problem, result):\n    try:\n        raise problem\n    finally:\n        return result\n");
    assert_eq!(r["status"], "data_slice", "{r}");
    assert!(variables(&r).contains("result"), "{r}");
    assert!(!variables(&r).contains("problem"), "{r}");
    let r=flow("def entry(problem, discarded):\n    try:\n        return discarded\n    finally:\n        raise problem\n");
    assert_eq!(r["status"], "seed_unavailable", "{r}");
}
#[test]
fn variable_selection_accepts_multiple_contexts_of_one_finally_occurrence() {
    let source="def entry(input):\n    try:\n        value = input / 0\n    finally:\n        result = input\n        return result\n";
    let s = graph(&[("p", "/p/a.py", source)]);
    let mut query = q("entry");
    query.variable = Some("result".into());
    let r = query::query(&s, &query);
    assert_eq!(r["status"], "data_slice", "{r}");
    assert!(r["seeds"].as_array().unwrap().len() > 1, "{r}");
    let ids: HashSet<_> = r["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["node"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), r["items"].as_array().unwrap().len());
}
#[test]
fn handlers_bare_raise_and_uninitialized_finally_reads_remain_explicit() {
    for source in [
        "def entry(x):\n    try:\n        return x\n    except build_type():\n        return 0\n",
        "def entry(x):\n    try:\n        raise\n    finally:\n        return x\n",
        "def entry(x):\n    try:\n        value = x / 0\n    finally:\n        return value\n",
    ] {
        let r = flow(source);
        assert_eq!(r["status"], "unsupported_function", "{r}");
    }
}
#[test]
fn repeated_finally_calls_have_distinct_frames_for_their_completion_contexts() {
    let source="def identity(value):\n    return value\ndef entry(flag, left, right):\n    result = left\n    try:\n        if flag:\n            return 0\n        result = right\n    finally:\n        final = identity(result)\n        return final\n";
    let r = flow(source);
    assert_eq!(r["status"], "data_slice", "{r}");
    for name in ["left", "right"] {
        assert!(variables(&r).contains(name), "{r}");
    }
    let frames: HashSet<_> = r["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["relation"] == "call_argument")
        .map(|e| e["call_context"].as_str().unwrap())
        .collect();
    assert!(frames.len() >= 2, "{r}");
    assert!(
        r["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["node"]["flow_context"]
                .as_array()
                .is_some_and(|c| !c.is_empty())),
        "{r}"
    );
}
#[test]
fn caller_module_binding_guards_are_honored_at_value_boundaries() {
    let s=graph(&[("p","/p/lib.py","def identity(value):\n    return value\n"),("p","/p/a.py","from lib import identity\ndef unsafe(default=1):\n    return default\ndef entry(input):\n    return identity(input)\n")]);
    let r = query::query(&s, &q("entry"));
    assert_eq!(r["status"], "data_slice", "{r}");
    assert!(
        r["call_boundary_diagnostics_total"].as_u64().unwrap() > 0,
        "{r}"
    );
    assert!(!variables(&r).contains("value"), "{r}");
}
#[test]
fn exception_state_and_finally_depth_budgets_are_explicit() {
    let mut source = "def entry(x):\n    try:\n".to_string();
    for _ in 0..130 {
        source.push_str("        value = x + 1\n");
    }
    source.push_str("    finally:\n        return x\n");
    let r = flow(&source);
    assert_eq!(r["status"], "unsupported_function", "{r}");
    assert!(
        r["reason"]
            .as_str()
            .unwrap()
            .contains("exception_state_budget_exhausted"),
        "{r}"
    );
    let mut source = "def entry(x):\n".to_string();
    for n in 1..35 {
        source.push_str(&format!("{}try:\n", "    ".repeat(n)));
    }
    source.push_str(&format!("{}return x\n", "    ".repeat(35)));
    for n in (1..35).rev() {
        source.push_str(&format!(
            "{}finally:\n{}pass\n",
            "    ".repeat(n),
            "    ".repeat(n + 1)
        ));
    }
    let r = flow(&source);
    assert_eq!(r["status"], "unsupported_function", "{r}");
    assert!(
        r["reason"].as_str().unwrap().contains("depth_exhausted"),
        "{r}"
    );
}
