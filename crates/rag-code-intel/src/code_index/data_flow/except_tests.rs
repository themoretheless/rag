use super::tests::{graph, q, variables};
use super::*;
fn flow(source: &str) -> Value {
    query::query(&graph(&[("p", "/p/a.py", source)]), &q("entry"))
}
#[test]
fn bare_handler_joins_normal_and_handled_definitions_without_old_value_leak() {
    let r = flow(
        "def entry(initial, numerator, denominator, fallback):\n    value = initial\n    try:\n        value = numerator / denominator\n    except:\n        value = fallback\n    return value\n",
    );
    assert_eq!(r["status"], "data_slice", "{r}");
    for n in ["numerator", "denominator", "fallback"] {
        assert!(variables(&r).contains(n), "{n}: {r}");
    }
    assert!(!variables(&r).contains("initial"), "{r}");
    assert!(
        r["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["relation"] == "try_merge_input"),
        "{r}"
    );
    assert_eq!(r["exception_type_matching_proven"], false);
}
#[test]
fn else_runs_only_after_normal_body_and_its_errors_do_not_retry_handlers() {
    let r = flow(
        "def entry(problem, handled, normal):\n    try:\n        raise problem\n    except:\n        return handled\n    else:\n        return normal\n",
    );
    assert_eq!(r["status"], "data_slice", "{r}");
    assert!(variables(&r).contains("handled"), "{r}");
    assert!(!variables(&r).contains("normal"), "{r}");
    let r = flow(
        "def entry(problem, wrong, result):\n    try:\n        value = 1\n    except:\n        return wrong\n    else:\n        raise problem\n    finally:\n        return result\n",
    );
    assert_eq!(r["status"], "data_slice", "{r}");
    assert!(variables(&r).contains("result"), "{r}");
    assert!(!variables(&r).contains("wrong"), "{r}");
    assert!(!variables(&r).contains("problem"), "{r}");
}
#[test]
fn handler_errors_propagate_outward_instead_of_retrying_the_same_handler() {
    let r = flow(
        "def entry(original, replacement):\n    try:\n        raise original\n    except:\n        raise replacement\n",
    );
    assert_eq!(r["status"], "seed_unavailable", "{r}");
    let r = flow(
        "def entry(original, replacement, result):\n    try:\n        try:\n            raise original\n        except:\n            raise replacement\n    except:\n        return result\n",
    );
    assert_eq!(r["status"], "data_slice", "{r}");
    assert!(variables(&r).contains("result"), "{r}");
}
#[test]
fn typed_handlers_are_distinct_unproven_candidates_with_call_contexts() {
    let source = "def identity(value):\n    return value\ndef entry(problem, left, right, fallback):\n    try:\n        raise problem\n    except First:\n        return identity(left)\n    except Second:\n        return identity(right)\n    except:\n        return fallback\n";
    let r = flow(source);
    assert_eq!(r["status"], "data_slice", "{r}");
    for n in ["left", "right", "fallback"] {
        assert!(variables(&r).contains(n), "{n}: {r}");
    }
    let frames: HashSet<_> = r["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["relation"] == "call_argument")
        .map(|e| e["call_context"].as_str().unwrap())
        .collect();
    assert_eq!(frames.len(), 2, "{r}");
    assert_eq!(r["exception_filter_evaluation_proven"], false);
}
#[test]
fn caught_alias_values_survive_return_evaluation_but_alias_names_are_cleared() {
    let r = flow(
        "def entry(problem):\n    try:\n        raise problem\n    except Exception as err:\n        return err\n",
    );
    assert_eq!(r["status"], "data_slice", "{r}");
    for n in ["problem", "err"] {
        assert!(variables(&r).contains(n), "{r}");
    }
    assert!(
        r["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["relation"] == "exception_binding"),
        "{r}"
    );
    for source in [
        "def entry(problem, err):\n    try:\n        raise problem\n    except Exception as err:\n        pass\n    return err\n",
        "def entry(problem):\n    try:\n        raise problem\n    except Exception as err:\n        return err\n    finally:\n        return err\n",
    ] {
        let r = flow(source);
        assert_eq!(r["status"], "unsupported_function", "{r}");
    }
}
#[test]
fn captured_bare_reraise_reaches_the_outer_handler() {
    let r = flow(
        "def entry(problem, result):\n    try:\n        try:\n            raise problem\n        except Exception as err:\n            raise\n    except:\n        return result\n",
    );
    assert_eq!(r["status"], "data_slice", "{r}");
    assert!(variables(&r).contains("result"), "{r}");
    let count = r["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["node"]["kind"] == "raise")
        .count();
    assert!(count >= 2, "{r}");
}
#[test]
fn finally_overrides_handler_returns_and_updates_handler_break_states() {
    let r = flow(
        "def entry(problem, result):\n    try:\n        raise problem\n    except Exception as err:\n        return err\n    finally:\n        return result\n",
    );
    assert_eq!(r["status"], "data_slice", "{r}");
    assert!(variables(&r).contains("result"), "{r}");
    assert!(!variables(&r).contains("problem"), "{r}");
    let r = flow(
        "def entry(flag, problem, initial, overwritten, updated):\n    value = initial\n    while flag:\n        try:\n            raise problem\n        except:\n            value = overwritten\n            break\n        finally:\n            value = updated\n    return value\n",
    );
    assert_eq!(r["status"], "data_slice", "{r}");
    for n in ["initial", "updated"] {
        assert!(variables(&r).contains(n), "{r}");
    }
    assert!(!variables(&r).contains("overwritten"), "{r}");
}
#[test]
fn successful_else_and_handler_paths_merge_their_final_values() {
    let r = flow(
        "def entry(numerator, denominator, handled, normal):\n    try:\n        value = numerator / denominator\n    except:\n        value = handled\n    else:\n        value = normal\n    return value\n",
    );
    assert_eq!(r["status"], "data_slice", "{r}");
    for n in ["handled", "normal"] {
        assert!(variables(&r).contains(n), "{r}");
    }
}
#[test]
fn handler_route_expansion_is_bounded() {
    let mut source = "def entry(x):\n    try:\n".to_string();
    for _ in 0..30 {
        source.push_str("        value = x / 1\n");
    }
    for n in 0..5 {
        source.push_str(&format!("    except Error{n}:\n        return x\n"));
    }
    source.push_str("    return x\n");
    let r = flow(&source);
    assert_eq!(r["status"], "unsupported_function", "{r}");
    assert!(
        r["reason"]
            .as_str()
            .unwrap()
            .contains("handler_route_budget_exhausted"),
        "{r}"
    );
}
#[test]
fn attribute_filter_names_do_not_read_an_unrelated_same_named_local() {
    let r = flow(
        "def entry(problem, errors, TypeError, result):\n    try:\n        raise problem\n    except errors.TypeError:\n        return result\n",
    );
    assert_eq!(r["status"], "data_slice", "{r}");
    assert!(variables(&r).contains("errors"), "{r}");
    assert!(!variables(&r).contains("TypeError"), "{r}");
    let r = flow(
        "def entry(problem, result):\n    try:\n        raise problem\n    except (ValueError, # comment in the selector\n            TypeError):\n        return result\n",
    );
    assert_eq!(r["status"], "data_slice", "{r}");
}
