use super::*;
fn graph(files: &[(&str, &str, &str)]) -> CodeSnapshot {
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
    out
}
fn targets(s: &CodeSnapshot, caller: &str) -> Vec<String> {
    s.calls
        .iter()
        .filter(|e| {
            e.reference.caller.as_ref().is_some_and(|id| {
                s.symbols
                    .iter()
                    .any(|s| &s.id == id && s.qualified_name == caller)
            })
        })
        .flat_map(|e| e.candidates.iter())
        .filter_map(|id| s.symbols.iter().find(|s| &s.id == id))
        .map(|s| s.qualified_name.clone())
        .collect()
}
#[test]
fn normalized_python_parameters_shadow_globals_without_rewriting_evidence() {
    let source = "def f(value):\n    return value\ndef shadowed(𝒇, x):\n    return f(x)\ndef caller(x):\n    return 𝒇(x)\n";
    let s = graph(&[("p", "/p/a.py", source)]);
    assert!(targets(&s, "shadowed").is_empty(), "{:?}", s.calls);
    assert_eq!(targets(&s, "caller"), ["f"]);
    let call = s
        .calls
        .iter()
        .find(|e| e.reference.expression == "𝒇")
        .unwrap();
    assert_eq!(
        &source[call.reference.start_byte..call.reference.start_byte + "𝒇".len()],
        "𝒇"
    );
    assert_eq!(call.reference.content_hash, content_hash(source));
}
#[test]
fn normalized_import_aliases_use_python_module_identifier_spelling() {
    let s = graph(&[
        ("p", "/p/lib.py", "def 𝒇(value):\n    return value\n"),
        (
            "p",
            "/p/app.py",
            "from 𝒍ib import f as 𝒈\ndef entry(x):\n    return g(x)\n",
        ),
        ("q", "/q/lib.py", "def f(x):\n    return x\n"),
    ]);
    assert_eq!(targets(&s, "entry"), ["𝒇"]);
    let call = s
        .calls
        .iter()
        .find(|e| e.reference.expression == "g")
        .unwrap();
    assert_eq!(call.reference.path, "/p/app.py");
}
#[test]
fn python_locals_are_function_scoped_and_parameters_keep_their_initial_values() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def f():\n    return 1\ndef early():\n    f()\n    𝒇 = unknown\ndef parameter(f):\n    f()\n    𝒇 = unknown\ndef block(flag):\n    if flag:\n        𝒇 = unknown\n    f()\n",
    )]);
    for name in ["early", "parameter", "block"] {
        assert!(targets(&s, name).is_empty(), "{name}: {:?}", s.calls);
    }
}
#[test]
fn class_dictionaries_do_not_shadow_method_globals_and_defaults_use_parent_scope() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def f():\n    return 1\nclass C:\n    𝒇 = 3\n    def method(self):\n        return f()\ndef defaults(𝒇=f()):\n    return 𝒇\n",
    )]);
    assert_eq!(targets(&s, "C::method"), ["f"]);
    assert_eq!(targets(&s, "defaults"), ["f"]);
}

#[test]
fn class_body_and_method_default_names_follow_the_actual_evaluation_scope() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def f():\n    return 1\nclass C:\n    before = f()\n    def f():\n        return 2\n    def method(self):\n        return f()\n    def defaults(self, x=f()):\n        return x\n    def missing(other):\n        return self.f()\n",
    )]);
    assert_eq!(targets(&s, "C::method"), ["f"]);
    assert_eq!(targets(&s, "C::defaults"), ["C::f"]);
    assert!(targets(&s, "C::missing").is_empty());
    let before = s.calls.iter().find(|e| e.reference.line == 4).unwrap();
    assert!(
        s.symbols
            .iter()
            .any(|target| target.qualified_name == "f" && before.candidates.contains(&target.id))
    );
}
#[test]
fn a_typed_parameter_keeps_its_value_before_a_later_local_assignment() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "class C:\n    def work(self):\n        return 1\ndef entry(item:C):\n    item.work()\n    𝒊tem = unknown\n    item.work()\n",
    )]);
    let calls: Vec<_> = s
        .calls
        .iter()
        .filter(|e| e.reference.expression == "item.work")
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].candidates.len(), 1);
    assert!(calls[1].candidates.is_empty());
}

#[test]
fn opaque_python_binders_and_shadowed_super_do_not_gain_false_targets() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def f():\n    return 1\nclass B:\n    def work(self):\n        return 1\nclass C(B):\n    def entry(self, 𝒔uper):\n        return super().work()\ndef lambdas():\n    return lambda 𝒇: f()\ndef comprehensions(xs):\n    return [f() for 𝒇 in xs]\ndef globals():\n    global f\n    f = unknown\n    return f()\n",
    )]);
    for name in ["C::entry", "lambdas", "comprehensions", "globals"] {
        assert!(targets(&s, name).is_empty(), "{name}: {:?}", s.calls);
    }
}

#[test]
fn for_targets_shadow_global_calls_but_not_other_imports() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def f(x):\n    return x\ndef g(x):\n    return x\ndef entry(values):\n    for 𝒇 in values:\n        f(1)\n        g(1)\n    f(1)\n",
    )]);
    assert_eq!(targets(&s, "entry"), ["g"]);
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def f():\n    return 1\ndef entry():\n    for f in f():\n        pass\n    return 1\n",
    )]);
    assert!(targets(&s, "entry").is_empty());
}
#[test]
fn handler_aliases_shadow_globals_and_cleanup_masks_inner_reassignments() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def f():\n    return 1\ndef g():\n    return 2\ndef entry(problem):\n    try:\n        raise problem\n    except Exception as 𝒇:\n        f()\n        g()\n    f()\n",
    )]);
    assert_eq!(targets(&s, "entry"), ["g"]);
    let s = graph(&[(
        "p",
        "/p/a.py",
        "class C:\n    def work(self):\n        return 1\ndef entry(receiver:C, problem):\n    receiver.work()\n    try:\n        raise problem\n    except Exception as receiver:\n        receiver = C()\n        receiver.work()\n    receiver.work()\n",
    )]);
    let calls: Vec<_> = s
        .calls
        .iter()
        .filter(|c| c.reference.expression == "receiver.work")
        .collect();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].candidates.len(), 1);
    assert_eq!(calls[1].candidates.len(), 1);
    assert!(calls[2].candidates.is_empty(), "{:?}", s.calls);
}
#[test]
fn handler_alias_parser_shape_keeps_one_type_and_one_identifier() {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .unwrap();
    let tree = parser
        .parse("try:\n    pass\nexcept Exception as err:\n    pass\n", None)
        .unwrap();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "except_clause" {
            let (types, alias) = handler_parts(node).unwrap();
            assert_eq!(
                types.len(),
                1,
                "{:?}",
                types.iter().map(|n| n.kind()).collect::<Vec<_>>()
            );
            assert!(alias.is_some());
        }
        stack.extend(children(node));
    }
}
#[test]
fn assignment_rhs_uses_the_preceding_receiver_and_commits_the_new_type_afterwards() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "class Old:\n    def work(self):\n        return 1\nclass New:\n    def work(self):\n        return 2\ndef entry(receiver:Old):\n    receiver = New(receiver.work())\n    receiver.work()\n",
    )]);
    let calls: Vec<_> = s
        .calls
        .iter()
        .filter(|e| e.reference.expression == "receiver.work")
        .collect();
    assert_eq!(calls.len(), 2);
    for (call, name) in calls.iter().zip(["Old::work", "New::work"]) {
        let names: Vec<_> = call
            .candidates
            .iter()
            .filter_map(|id| s.symbols.iter().find(|s| &s.id == id))
            .map(|s| s.qualified_name.as_str())
            .collect();
        assert_eq!(names, [name], "{call:?}");
    }
}
#[test]
fn first_assignment_rhs_does_not_use_the_value_being_created() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "class New:\n    def work(self):\n        return 1\ndef entry():\n    value = New(value.work())\n    value.work()\n",
    )]);
    let calls: Vec<_> = s
        .calls
        .iter()
        .filter(|e| e.reference.expression == "value.work")
        .collect();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].candidates.is_empty(), "{:?}", s.calls);
    assert_eq!(calls[1].candidates.len(), 1);
}
#[test]
fn module_and_class_assignment_rhs_can_still_use_the_preceding_global() {
    for source in [
        "def f():\n    return 1\nclass Other:\n    pass\nf = Other(f())\n",
        "def f():\n    return 1\nclass Other:\n    pass\nclass C:\n    f = Other(f())\n",
    ] {
        let s = graph(&[("p", "/p/a.py", source)]);
        let call = s
            .calls
            .iter()
            .find(|c| c.reference.expression == "f")
            .unwrap();
        assert_eq!(call.candidates.len(), 1, "{call:?}");
        assert!(
            s.symbols
                .iter()
                .any(|s| s.qualified_name == "f" && call.candidates.contains(&s.id))
        );
    }
}
#[test]
fn annotation_only_locals_shadow_globals_without_overwriting_existing_values() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def f():\n    return 1\nclass Old:\n    def work(self):\n        return 1\nclass New:\n    pass\ndef uninitialized():\n    f:New\n    return f()\ndef retained(value:Old):\n    value:New\n    return value.work()\n",
    )]);
    assert!(targets(&s, "uninitialized").is_empty(), "{:?}", s.calls);
    assert_eq!(targets(&s, "retained"), ["Old::work"]);
    let s = graph(&[(
        "p",
        "/p/a.py",
        "def f():\n    return 1\nf:Other\nf()\nclass C:\n    f:Other\n    f()\n",
    )]);
    let calls: Vec<_> = s
        .calls
        .iter()
        .filter(|c| c.reference.expression == "f")
        .collect();
    assert_eq!(calls.len(), 2);
    assert!(
        calls.iter().all(|c| c.candidates.len() == 1),
        "{:?}",
        s.calls
    );
}
#[test]
fn augmented_assignment_reads_the_old_receiver_then_forgets_unverified_result_type() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "class Old:\n    def work(self):\n        return 1\nclass New:\n    pass\ndef entry(value:Old):\n    value += New(value.work())\n    value.work()\n",
    )]);
    let calls: Vec<_> = s
        .calls
        .iter()
        .filter(|c| c.reference.expression == "value.work")
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].candidates.len(), 1);
    assert!(
        s.symbols
            .iter()
            .any(|s| s.qualified_name == "Old::work" && calls[0].candidates.contains(&s.id))
    );
    assert!(calls[1].candidates.is_empty(), "{:?}", s.calls);
}
#[test]
fn chained_and_nfkc_assignment_commits_follow_rhs_evaluation() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "class Old:\n    def work(self):\n        return 1\nclass New:\n    def work(self):\n        return 2\ndef entry(receiver:Old):\n    other = 𝒓eceiver = New(receiver.work())\n    receiver.work()\n",
    )]);
    let calls: Vec<_> = s
        .calls
        .iter()
        .filter(|c| c.reference.expression == "receiver.work")
        .collect();
    assert_eq!(calls.len(), 2);
    for (call, expected) in calls.iter().zip(["Old::work", "New::work"]) {
        assert!(
            s.symbols
                .iter()
                .any(|s| s.qualified_name == expected && call.candidates.contains(&s.id)),
            "{call:?}"
        );
    }
}
#[test]
fn receiver_after_optional_assignment_keeps_both_reaching_types() {
    let s = graph(&[(
        "p",
        "/p/a.py",
        "class Old:\n    def work(self):\n        return 1\nclass New:\n    def work(self):\n        return 2\ndef entry(receiver:Old, flag):\n    if flag:\n        receiver = New()\n    receiver.work()\n",
    )]);
    assert_eq!(
        targets(&s, "entry")
            .into_iter()
            .filter(|n| n.ends_with("::work"))
            .collect::<Vec<_>>(),
        ["Old::work", "New::work"]
    );
}
fn work_names(s: &CodeSnapshot) -> std::collections::BTreeSet<String> {
    s.calls
        .iter()
        .filter(|c| c.reference.name == "work")
        .flat_map(|c| c.candidates.iter())
        .filter_map(|id| s.symbols.iter().find(|s| &s.id == id))
        .map(|s| s.qualified_name.clone())
        .collect()
}
const RECEIVER_CLASSES: &str = "class Old:\n    def work(self):\n        return 1\nclass New:\n    def work(self):\n        return 2\nclass Third:\n    def work(self):\n        return 3\n";
#[test]
fn both_arms_kill_the_old_type_and_returned_arms_do_not_reach_later_calls() {
    let source = format!(
        "{RECEIVER_CLASSES}def entry(receiver:Old, flag):\n    if flag:\n        receiver = New()\n    else:\n        receiver = Third()\n    receiver.work()\n"
    );
    let s = graph(&[("p", "/p/a.py", &source)]);
    assert_eq!(
        work_names(&s),
        ["New::work".into(), "Third::work".into()].into()
    );
    let source = format!(
        "{RECEIVER_CLASSES}def entry(receiver:Old, flag):\n    if flag:\n        receiver = New()\n        return 0\n    receiver.work()\n"
    );
    let s = graph(&[("p", "/p/a.py", &source)]);
    assert_eq!(work_names(&s), ["Old::work".into()].into());
}
#[test]
fn loops_keep_zero_iteration_backedges_and_break_exit_types() {
    for body in [
        "def entry(receiver:Old, flag):\n    while flag:\n        receiver.work()\n        receiver = New()\n    receiver.work()\n",
        "def entry(receiver:Old, values):\n    for item in values:\n        receiver = New()\n    receiver.work()\n",
        "def entry(receiver:Old, flag, stop):\n    while flag:\n        if stop:\n            receiver = New()\n            break\n        receiver = Third()\n        continue\n    receiver.work()\n",
    ] {
        let source = format!("{RECEIVER_CLASSES}{body}");
        let s = graph(&[("p", "/p/a.py", &source)]);
        let names = work_names(&s);
        assert!(
            names.contains("Old::work") && names.contains("New::work"),
            "{names:?}"
        );
    }
}
#[test]
fn local_aliases_keep_the_origin_and_unknown_paths_remain_partial() {
    let source = format!(
        "{RECEIVER_CLASSES}def entry(receiver:Old, flag, unknown):\n    alias = receiver\n    if flag:\n        alias = New()\n    alias.work()\n    if flag:\n        receiver = unknown\n    receiver.work()\n"
    );
    let s = graph(&[("p", "/p/a.py", &source)]);
    let alias = s
        .calls
        .iter()
        .find(|c| c.reference.expression == "alias.work")
        .unwrap();
    assert_eq!(alias.candidates.len(), 2, "{alias:?}");
    let partial = s
        .calls
        .iter()
        .find(|c| c.reference.expression == "receiver.work")
        .unwrap();
    assert_eq!(partial.candidates.len(), 1);
    assert_eq!(partial.resolution, "ambiguous");
    assert!(partial.has_unknown_targets());
    let report = s.query(
        "dead_code",
        &CodeQuery {
            limit: Some(500),
            ..Default::default()
        },
    );
    assert!(
        !report["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i["symbol"]["qualified_name"] == "Third::work"),
        "{report}"
    );
}
#[test]
fn imported_types_and_inherited_methods_survive_receiver_joins() {
    let s = graph(&[
        (
            "p",
            "/p/lib.py",
            "class Base:\n    def work(self):\n        return 1\nclass Derived(Base):\n    pass\n",
        ),
        (
            "p",
            "/p/a.py",
            "from lib import Base as Old\nfrom lib import Derived as New\ndef entry(receiver:Old, flag):\n    if flag:\n        receiver = New()\n    receiver.work()\n",
        ),
        (
            "q",
            "/q/lib.py",
            "class Base:\n    def work(self):\n        return 9\n",
        ),
    ]);
    let call = s
        .calls
        .iter()
        .find(|c| c.reference.expression == "receiver.work")
        .unwrap();
    assert_eq!(call.candidates.len(), 1, "{call:?}");
    assert_eq!(call.resolution, "resolved_static");
    assert!(
        s.symbols.iter().any(|s| s.path == "/p/lib.py"
            && s.qualified_name == "Base::work"
            && call.candidates.contains(&s.id)),
        "{call:?}"
    );
}
#[test]
fn receiver_origin_budget_becomes_explicit_unknown_instead_of_a_last_type() {
    let mut source = format!("{RECEIVER_CLASSES}def entry(receiver:Old, flag):\n");
    for n in 0..8 {
        source.push_str(&format!(
            "    {} flag == {n}:\n        receiver = New()\n",
            if n == 0 { "if" } else { "elif" }
        ));
    }
    source.push_str("    receiver.work()\n");
    let s = graph(&[("p", "/p/a.py", &source)]);
    let call = s
        .calls
        .iter()
        .find(|c| c.reference.expression == "receiver.work")
        .unwrap();
    assert!(call.candidates.is_empty(), "{call:?}");
    assert_eq!(
        call.resolution_basis,
        "python_receiver_flow_capture_incomplete"
    );
}
#[test]
fn repeated_import_aliases_preserve_the_last_binding_in_one_statement() {
    let s = graph(&[
        (
            "p",
            "/p/a.py",
            "def entry():\n    import liba as receiver, libb as receiver\n    receiver.work()\n",
        ),
        ("p", "/p/liba.py", "def work():\n    return 1\n"),
        ("p", "/p/libb.py", "def work():\n    return 2\n"),
    ]);
    let call = s
        .calls
        .iter()
        .find(|c| c.reference.expression == "receiver.work")
        .unwrap();
    assert_eq!(call.candidates.len(), 1, "{call:?}");
    assert!(
        s.symbols
            .iter()
            .any(|s| s.path == "/p/libb.py" && call.candidates.contains(&s.id)),
        "{call:?}"
    );
}
#[test]
fn destructured_unknown_write_does_not_leave_a_certain_parameter_type() {
    let source = format!(
        "{RECEIVER_CLASSES}def entry(receiver:Old, flag, values):\n    if flag:\n        receiver, other = values\n    receiver.work()\n"
    );
    let s = graph(&[("p", "/p/a.py", &source)]);
    let call = s
        .calls
        .iter()
        .find(|c| c.reference.expression == "receiver.work")
        .unwrap();
    assert_eq!(call.resolution, "ambiguous", "{call:?}");
    assert!(call.has_unknown_targets());
    assert_eq!(call.candidates.len(), 1);
}
#[test]
fn missing_receiver_flow_payload_is_not_a_current_scope_capture() {
    let mut doc = parse(
        "d",
        "/p/a.py",
        "def entry(receiver):\n    receiver.work()\n",
        1,
    )
    .unwrap();
    assert!(current(&doc));
    doc.python_names.as_mut().unwrap().receiver_flows.clear();
    assert!(!current(&doc));
}
