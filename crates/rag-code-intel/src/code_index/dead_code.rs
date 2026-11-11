//! Possible liveness, deliberately distinct from proven runtime execution.
use super::*;
use std::collections::BTreeSet;

pub(super) fn candidates(
    snapshot: &CodeSnapshot,
    selected: &HashSet<&str>,
    limit: usize,
) -> (Vec<serde_json::Value>, usize, bool, serde_json::Value) {
    let symbols: HashMap<_, _> = snapshot
        .symbols
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();
    let project = |document: &str| {
        snapshot
            .document_scopes
            .get(document)
            .and_then(|s| s.project.as_deref())
    };
    let same_project = |source: &str, target: &str| {
        symbols
            .get(source)
            .zip(symbols.get(target))
            .is_some_and(|(a, b)| project(&a.document_id) == project(&b.document_id))
    };
    let mut reachable: BTreeSet<&str> = snapshot
        .entrypoints
        .iter()
        .filter_map(|e| e.symbol_id.as_deref())
        .filter(|id| symbols.contains_key(id))
        .collect();
    for e in snapshot
        .calls
        .iter()
        .filter(|e| e.reference.caller.is_none())
        .chain(
            snapshot
                .bound_references
                .iter()
                .filter(|e| !e.reference.is_call),
        )
    {
        for target in &e.candidates {
            if symbols
                .get(target.as_str())
                .is_some_and(|s| project(&s.document_id) == project(&e.reference.document_id))
            {
                reachable.insert(target);
            }
        }
    }
    let unknown: HashSet<_> = snapshot
        .bound_references
        .iter()
        .filter(|e| e.candidates.is_empty())
        .map(|e| (project(&e.reference.document_id), e.reference.name.as_str()))
        .collect();
    let mut unresolved_roots = 0;
    for s in snapshot.symbols.iter().filter(|s| analysis::callable(s)) {
        if unknown.contains(&(project(&s.document_id), s.name.as_str())) && reachable.insert(&s.id)
        {
            unresolved_roots += 1;
        }
    }
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for e in &snapshot.calls {
        if let Some(source) = e.reference.caller.as_deref() {
            adjacency.entry(source).or_default().extend(
                e.candidates
                    .iter()
                    .map(String::as_str)
                    .filter(|target| same_project(source, target)),
            );
        }
    }
    // A live base member may dispatch to an overriding declaration. This is a
    // conservative liveness exclusion, never a synthetic call or dispatch proof.
    let mut structural_edges = 0;
    for e in &snapshot.overrides {
        for base in &e.candidates {
            if same_project(base, &e.derived) {
                adjacency.entry(base).or_default().push(&e.derived);
                structural_edges += 1;
            }
        }
    }
    let mut todo: Vec<_> = reachable.iter().copied().collect();
    while let Some(source) = todo.pop() {
        for target in adjacency.get(source).into_iter().flatten() {
            if reachable.insert(target) {
                todo.push(target);
            }
        }
    }
    let mut candidates: Vec<_> = snapshot
        .symbols
        .iter()
        .filter(|s| {
            selected.contains(s.id.as_str())
                && analysis::callable(s)
                && !reachable.contains(s.id.as_str())
        })
        .collect();
    candidates.sort_by(|a, b| {
        (&a.path, &a.document_id, a.start_line, &a.id).cmp(&(
            &b.path,
            &b.document_id,
            b.start_line,
            &b.id,
        ))
    });
    (candidates.iter().take(limit).map(|s|serde_json::json!({"symbol":s,"reason":"outside_possible_static_reachability","provenance":"derived","deletion_safe":false})).collect(),candidates.len(),candidates.len()>limit,
        serde_json::json!({"reachable_symbols":reachable.len(),"unresolved_name_roots":unresolved_roots,"override_candidate_edges":structural_edges,"reachability_semantics":"possible_static_not_proven_execution","lexical_fallback_scope":"same_project_unresolved_names_only","candidate_only":true,"external_usage_unknown":true,"runtime_dispatch_proven":false}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(files: &[(&str, &str, &str)]) -> CodeSnapshot {
        let mut docs = HashMap::new();
        let mut out = CodeSnapshot::default();
        for (i, (path, project, source)) in files.iter().enumerate() {
            let mut d = parse(&i.to_string(), path, source, 1).unwrap();
            assert!(!d.parse_errors, "{path}");
            d.project = Some((*project).into());
            out.document_scopes.insert(
                i.to_string(),
                DocumentScope {
                    path: (*path).into(),
                    project: Some((*project).into()),
                },
            );
            out.symbols.extend(d.symbols.clone());
            out.references.extend(d.references.clone());
            docs.insert(i.to_string(), d);
        }
        out.bound_references = resolve::references(&out.symbols, &docs);
        out.calls = out
            .bound_references
            .iter()
            .filter(|e| e.reference.is_call)
            .cloned()
            .collect();
        out.overrides = inheritance::overrides(&out.symbols, &docs);
        out.entrypoints = analysis::entrypoints(&out.symbols, &docs);
        out
    }
    fn names(snap: &CodeSnapshot, path: &str) -> Vec<String> {
        snap.query(
            "dead_code",
            &CodeQuery {
                path: Some(path.into()),
                limit: Some(500),
                ..Default::default()
            },
        )["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["symbol"]["qualified_name"].as_str().unwrap().into())
            .collect()
    }
    #[test]
    fn unresolved_calls_guard_callees_and_candidate_pages_are_deterministic() {
        let snap=snapshot(&[("/p/a.py","p","def maybe(): helper()\ndef helper(): pass\ndef unused_a(): pass\ndef unused_b(): pass\nunknown.maybe()\n")]);
        let q = CodeQuery {
            limit: Some(1),
            ..Default::default()
        };
        let r = snap.query("dead_code", &q);
        assert_eq!(r["total"], 2, "{r}");
        assert_eq!(r["truncated"], true);
        assert_eq!(r["items"][0]["symbol"]["name"], "unused_a");
        assert_eq!(r["detail"]["unresolved_name_roots"], 1);
        assert_eq!(snap.query("dead_code", &q), r);
    }
    #[test]
    fn dead_candidates_are_not_hidden_by_bound_same_names_or_other_projects() {
        let snap = snapshot(&[
            ("/p/a.py", "alpha", "def work(): pass\nregistered = work\n"),
            ("/p/b.py", "alpha", "def work(): pass\n"),
            ("/q/a.py", "beta", "unknown.work\n"),
        ]);
        assert!(names(&snap, "/p/a.py").is_empty());
        assert_eq!(names(&snap, "/p/b.py"), vec!["work"]);
    }
    #[test]
    fn unresolved_same_project_roots_protect_their_callees_without_proving_liveness() {
        let snap = snapshot(&[(
            "/p/a.py",
            "p",
            "def work(): helper()\ndef helper(): pass\ndef unused(): pass\nunknown.work\n",
        )]);
        assert_eq!(names(&snap, "/p/a.py"), vec!["unused"]);
        let r = snap.query("dead_code", &CodeQuery::default());
        assert_eq!(r["detail"]["unresolved_name_roots"], 1);
        assert_eq!(r["analysis_incomplete"], true);
        assert_eq!(r["detail"]["runtime_dispatch_proven"], false);
    }
    #[test]
    fn live_base_members_protect_override_candidates_and_their_callees() {
        let snap=snapshot(&[("/p/a.py","p","class Base:\n    def work(self): pass\nclass Child(Base):\n    def work(self): helper()\ndef helper(): pass\ndef unused(): pass\nregistered = Base.work\n")]);
        assert_eq!(names(&snap, "/p/a.py"), vec!["unused"]);
        assert_eq!(
            snap.query("dead_code", &CodeQuery::default())["detail"]["override_candidate_edges"],
            1
        );
        assert!(!snap
            .calls
            .iter()
            .any(|e| e.reference.expression == "Base.work"));
    }
}
