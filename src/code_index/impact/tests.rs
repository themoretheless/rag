
use super::*;
fn snapshot(files: &[(&str, &str, &str)]) -> CodeSnapshot {
    let mut docs = HashMap::new();
    let mut out = CodeSnapshot::default();
    for (i, (path, project, source)) in files.iter().enumerate() {
        let mut doc = parse(&i.to_string(), path, source, 1).unwrap();
        assert!(!doc.parse_errors, "{path}");
        doc.project = Some((*project).into());
        out.document_scopes.insert(
            i.to_string(),
            DocumentScope {
                path: (*path).into(),
                project: Some((*project).into()),
            },
        );
        out.symbols.extend(doc.symbols.clone());
        out.references.extend(doc.references.clone());
        docs.insert(i.to_string(), doc);
    }
    out.bound_references = resolve::references(&out.symbols, &docs);
    out.calls = out
        .bound_references
        .iter()
        .filter(|e| e.reference.is_call)
        .cloned()
        .collect();
    out.sql = sql::snapshot(&docs);
    out.inheritance = inheritance::edges(&out.symbols, &docs);
    out.overrides = inheritance::overrides(&out.symbols, &docs);
    out
}
fn names(snap: &CodeSnapshot, result: &serde_json::Value) -> Vec<String> {
    result["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            snap.symbols
                .iter()
                .find(|s| s.id == r["symbol_id"].as_str().unwrap())
                .unwrap()
                .qualified_name
                .clone()
        })
        .collect()
}
#[test]
fn table_impact_keeps_direct_ownerless_uses_and_traverses_readers_writers() {
    let snap=snapshot(&[("/p/api.py","p","def load():\n    q = 'SELECT * FROM users'\ndef save():\n    q = 'INSERT INTO users(id) VALUES(1)'\ndef screen(): load()\ndef submit(): save()\n"),("/p/raw.sql","p","SELECT * FROM users;")]);
    let mut q = CodeQuery {
        impact_table: Some("users".into()),
        impact_project: Some("p".into()),
        ..Default::default()
    };
    let r = snap.query("impact", &q);
    assert_eq!(r["direct_total"], 3, "{r}");
    assert_eq!(r["ownerless_direct_uses"], 1);
    assert_eq!(r["direct_owner_symbols"], 2);
    let n = names(&snap, &r);
    assert!(n.contains(&"screen".into()) && n.contains(&"submit".into()));
    q.impact_table_mode = Some("read".into());
    let r = snap.query("impact", &q);
    assert_eq!(r["direct_total"], 2);
    assert_eq!(names(&snap, &r), vec!["screen"]);
    q.impact_table_mode = Some("write".into());
    let r = snap.query("impact", &q);
    assert_eq!(r["direct_total"], 1);
    assert_eq!(names(&snap, &r), vec!["submit"]);
    q.impact_table_mode = None;
    q.limit = Some(1);
    let r = snap.query("impact", &q);
    assert_eq!(r["direct_truncated"], true);
    assert_eq!(r["direct_owner_symbols"], 2);
    assert_eq!(r["truncated"], true);
}
#[test]
fn table_impact_requires_project_and_qualified_identity_when_ambiguous() {
    let snap = snapshot(&[
        (
            "/p/a.sql",
            "p",
            "SELECT * FROM public.users; SELECT * FROM admin.users;",
        ),
        ("/q/b.sql", "q", "SELECT * FROM public.users;"),
    ]);
    let mut q = CodeQuery {
        impact_table: Some("public.users".into()),
        ..Default::default()
    };
    assert_eq!(snap.query("impact", &q)["needs_disambiguation"], true);
    q.impact_project = Some("p".into());
    assert_eq!(snap.query("impact", &q)["direct_total"], 1);
    q.impact_table = Some("users".into());
    assert_eq!(snap.query("impact", &q)["table_candidates_total"], 2);
    q.path = Some("/q/b.sql".into());
    let ambiguous = snap.query("impact", &q);
    assert_eq!(ambiguous["needs_disambiguation"], true);
    assert_eq!(ambiguous["table_candidates_total"], 2);
    assert!(ambiguous["items"].as_array().unwrap().is_empty());
    // Once the table identity is explicit, a nonmatching source path is a
    // qualified zero-use observation rather than a way to hide ambiguity.
    q.impact_table = Some("public.users".into());
    assert_eq!(snap.query("impact", &q)["direct_total"], 0);
}
#[test]
fn table_impact_preserves_quoted_identifiers_and_rejects_mixed_or_invalid_targets() {
    let snap = snapshot(&[(
        "/p/a.sql",
        "p",
        "SELECT * FROM \"Users\"; SELECT * FROM users;",
    )]);
    let mut q = CodeQuery {
        impact_table: Some("\"Users\"".into()),
        impact_project: Some("p".into()),
        ..Default::default()
    };
    assert_eq!(snap.query("impact", &q)["direct_total"], 1);
    q.impact_table = Some("users; DROP TABLE users".into());
    assert!(snap.query("impact", &q)["error"].is_string());
    q.impact_table = Some("users".into());
    q.symbol = Some("fn".into());
    assert!(snap.query("impact", &q)["error"].is_string());
    q.symbol = None;
    q.impact_table_mode = Some("invalid".into());
    assert!(snap.query("impact", &q)["error"].is_string());
}
#[test]
fn impact_file_summaries_group_document_identity_and_count_only_returned_symbols() {
    let snap = snapshot(&[
        ("/p/a.py", "p", "def core(): pass\n"),
        (
            "/p/b.py",
            "p",
            "from a import core\ndef one(): return core\ndef two(): return core\n",
        ),
        (
            "/p/c.py",
            "p",
            "from b import one\ndef wrapper(): return one()\n",
        ),
    ]);
    let mut q = CodeQuery {
        symbol: Some("core".into()),
        path: Some("/p/a.py".into()),
        impact_relations: Some("references".into()),
        ..Default::default()
    };
    let r = snap.query("impact", &q);
    let f = r["affected_files"].as_array().unwrap();
    assert_eq!(f.len(), 2, "{r}");
    assert_eq!(f[0]["path"], "/p/b.py");
    assert_eq!(f[0]["symbol_count"], 2);
    assert_eq!(f[0]["relation_counts"]["bound_reference"], 2);
    assert_eq!(f[1]["min_depth"], 2);
    assert_eq!(f[1]["relation_counts"]["call"], 1);
    assert_eq!(f[0]["project"], "p");
    assert!(f[0]["content_hash"].as_str().is_some_and(|s| !s.is_empty()));
    q.limit = Some(1);
    let capped = snap.query("impact", &q);
    assert_eq!(
        capped["affected_files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["symbol_count"].as_u64().unwrap())
            .sum::<u64>(),
        1
    );
    assert_eq!(capped["file_summary_scope"], "returned_symbols");
    assert_eq!(capped["program_impact_complete"], false);
}
#[test]
fn impact_file_summaries_do_not_merge_identical_paths_across_projects() {
    let snap = snapshot(&[
        (
            "/same/a.py",
            "alpha",
            "def core(): pass\ndef user(): return core\n",
        ),
        (
            "/same/a.py",
            "beta",
            "def core(): pass\ndef user(): return core\n",
        ),
    ]);
    let r = snap.query(
        "impact",
        &CodeQuery {
            symbol: Some("core".into()),
            impact_relations: Some("references".into()),
            ..Default::default()
        },
    );
    let files = r["affected_files"].as_array().unwrap();
    assert_eq!(files.len(), 2, "{r}");
    assert_ne!(files[0]["document_id"], files[1]["document_id"]);
    assert_ne!(files[0]["project"], files[1]["project"]);
}
#[test]
fn impact_rejects_cross_project_structural_edges_even_with_colliding_paths() {
    let mut snap = snapshot(&[
        ("/same/a.py", "alpha", "class Base: pass\n"),
        ("/same/a.py", "beta", "class Foreign: pass\n"),
    ]);
    let base = snap
        .symbols
        .iter()
        .find(|s| s.document_id == "0")
        .unwrap()
        .id
        .clone();
    let foreign = snap
        .symbols
        .iter()
        .find(|s| s.document_id == "1")
        .unwrap()
        .id
        .clone();
    snap.inheritance.push(inheritance::InheritanceEdge {
        derived: foreign,
        candidates: vec![base.clone()],
        base_expression: "Base".into(),
        path: "/same/a.py".into(),
        line: 1,
        relation: "extends".into(),
        resolution: "resolved_static".into(),
        hierarchy_status: "complete_static".into(),
        provenance: "derived",
    });
    let result = snap.query(
        "impact",
        &CodeQuery {
            symbol: Some(base),
            impact_relations: Some("all".into()),
            ..Default::default()
        },
    );
    assert!(names(&snap, &result).is_empty(), "{result}");
}
#[test]
fn impact_bound_values_propagate_into_callers_without_inventing_calls() {
    let snap=snapshot(&[("/p/a.py","p","def core(): pass\ndef callback():\n    return core\ndef wrapper():\n    return callback()\n")]);
    let mut q = CodeQuery {
        symbol: Some("core".into()),
        ..Default::default()
    };
    assert!(names(&snap, &snap.query("impact", &q)).is_empty());
    q.impact_relations = Some("references".into());
    let result = snap.query("impact", &q);
    assert_eq!(names(&snap, &result), vec!["callback", "wrapper"]);
    assert_eq!(result["items"][0]["via_kind"], "bound_reference");
    assert_eq!(result["items"][1]["via_kind"], "call");
    assert_eq!(result["items"][1]["depth"], 2);
    assert_eq!(result["runtime_impact_proven"], false);
    assert_eq!(snap.query("callers", &q)["total"], 0);
}
#[test]
fn impact_all_includes_override_candidates_and_class_dependencies() {
    let snap=snapshot(&[("/p/a.py","p","class Base:\n    def work(self): pass\nclass Child(Base):\n    def work(self): pass\ndef typed(value: Base):\n    return value\n")]);
    let mut q = CodeQuery {
        symbol: Some("Base::work".into()),
        impact_relations: Some("references".into()),
        ..Default::default()
    };
    assert!(names(&snap, &snap.query("impact", &q)).is_empty());
    q.impact_relations = Some("all".into());
    let result = snap.query("impact", &q);
    assert_eq!(names(&snap, &result), vec!["Child::work"]);
    assert_eq!(result["items"][0]["via_kind"], "override_candidate");
    assert_eq!(result["items"][0]["via"]["semantic_override_proven"], false);
    q.symbol = Some("Base".into());
    let result = snap.query("impact", &q);
    let found = names(&snap, &result);
    assert!(found.contains(&"Child".into()), "{result}");
    assert!(found.contains(&"typed".into()), "{result}");
}
#[test]
fn impact_cycles_depth_limits_and_result_caps_are_independent_and_stable() {
    let mut snap=snapshot(&[("/p/a.py","p","def core():\n    return wrapper()\ndef callback():\n    return core\ndef wrapper():\n    return callback()\ndef another():\n    return core\n")]);
    let mut q = CodeQuery {
        symbol: Some("core".into()),
        impact_relations: Some("references".into()),
        ..Default::default()
    };
    let result = snap.query("impact", &q);
    assert_eq!(names(&snap, &result).len(), 3);
    assert_eq!(result["depth_limited"], false);
    snap.bound_references.reverse();
    snap.calls.reverse();
    assert_eq!(snap.query("impact", &q), result);
    q.depth = Some(1);
    assert_eq!(snap.query("impact", &q)["depth_limited"], true);
    q.depth = Some(10);
    q.limit = Some(1);
    let capped = snap.query("impact", &q);
    assert_eq!(capped["items"].as_array().unwrap().len(), 1);
    assert_eq!(capped["truncated"], true);
    assert_eq!(capped["traversal_complete"], false);
    q.impact_relations = Some("invalid".into());
    assert!(snap.query("impact", &q)["error"].is_string());
}
#[test]
fn impact_bound_edges_stay_inside_binding_projects() {
    let snap = snapshot(&[
        (
            "/same/a.py",
            "alpha",
            "def core(): pass\ndef user(): return core\n",
        ),
        (
            "/same/a.py",
            "beta",
            "def core(): pass\ndef foreign(): return core\n",
        ),
    ]);
    let seed = snap
        .symbols
        .iter()
        .find(|s| s.document_id == "0" && s.name == "core")
        .unwrap();
    let result = snap.query(
        "impact",
        &CodeQuery {
            symbol: Some(seed.id.clone()),
            impact_relations: Some("references".into()),
            ..Default::default()
        },
    );
    assert_eq!(names(&snap, &result), vec!["user"]);
}
