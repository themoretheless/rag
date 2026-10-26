//! Static related-test discovery with witnesses; never a safe CI omission policy.
use super::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AffectedTestsQuery {
    /// Changed symbol IDs, qualified names or short names. Each must be unique.
    pub symbols: Vec<String>,
    pub path: Option<String>,
    pub project: Option<String>,
    /// Incoming depth, 1..20 (default 10).
    pub depth: Option<usize>,
    /// Tests returned, 1..500 (default 100).
    pub limit: Option<usize>,
    /// Shared graph-candidate/traversal budget, 1..200000 (default 50000).
    pub max_work: Option<usize>,
    /// references by default (includes callback/value uses); all adds inheritance candidates.
    pub relations: Option<PathRelations>,
}
#[derive(Clone, Copy)]
struct Incoming<'a> {
    source: &'a str,
    target: &'a str,
    evidence: impact::Evidence<'a>,
}

pub(super) fn query(snapshot: &CodeSnapshot, q: &AffectedTestsQuery) -> Result<Value> {
    if q.symbols.is_empty() || q.symbols.len() > 1000 || q.symbols.iter().any(|s| s.is_empty()) {
        return Err(AppError::config(
            "symbols must contain 1..1000 nonempty selectors",
        ));
    }
    let mut seeds = BTreeMap::new();
    let mut selectors = Vec::new();
    let mut ambiguous = false;
    let mut unavailable = false;
    for selector in &q.symbols {
        let mapped = snapshot.test_seed_aliases.get(selector).unwrap_or(selector);
        let mut found: Vec<_> = snapshot
            .symbols
            .iter()
            .filter(|s| {
                q.project.as_deref().is_none_or(|p| {
                    snapshot
                        .document_scopes
                        .get(&s.document_id)
                        .and_then(|d| d.project.as_deref())
                        == Some(p)
                }) && q.path.as_deref().is_none_or(|p| s.path == p)
                    && (s.id == *mapped || s.name == *mapped || s.qualified_name == *mapped)
            })
            .collect();
        if found.iter().any(|s| s.id == *mapped) {
            found.retain(|s| s.id == *mapped);
        }
        found.sort_by(|a, b| a.id.cmp(&b.id));
        ambiguous |= found.len() > 1;
        unavailable |= found.is_empty();
        selectors.push(json!({"selector":selector,"candidates":found.iter().take(20).collect::<Vec<_>>(),"total":found.len(),"truncated":found.len()>20,"test_context_id_remapped":mapped!=selector}));
        if found.len() == 1 {
            seeds.insert(found[0].id.as_str(), found[0]);
        }
    }
    let selected: Vec<_> = seeds.into_values().collect();
    let mut result = if ambiguous || unavailable {
        selection(snapshot, &[], q)
    } else {
        selection(snapshot, &selected, q)
    };
    result["selectors"] = selectors.into();
    if ambiguous {
        result["status"] = "ambiguous_seeds".into();
    } else if unavailable {
        result["status"] = "seed_unavailable".into();
    }
    Ok(result)
}

pub(super) fn selection(
    snapshot: &CodeSnapshot,
    seeds: &[&Symbol],
    q: &AffectedTestsQuery,
) -> Value {
    let depth = q.depth.unwrap_or(10).clamp(1, 20);
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let budget = q.max_work.unwrap_or(50000).clamp(1, 200000);
    let projects: HashSet<_> = seeds
        .iter()
        .filter_map(|s| {
            snapshot
                .document_scopes
                .get(&s.document_id)
                .map(|d| d.project.as_deref())
        })
        .collect();
    let project = |s: &Symbol| {
        snapshot
            .document_scopes
            .get(&s.document_id)
            .map(|d| d.project.as_deref())
    };
    let excluded: HashSet<_> = snapshot
        .stale_documents
        .iter()
        .chain(&snapshot.unindexed_documents)
        .chain(&snapshot.parse_errors)
        .map(String::as_str)
        .collect();
    let known: BTreeMap<_, _> = snapshot
        .symbols
        .iter()
        .filter(|s| {
            !excluded.contains(s.document_id.as_str())
                && project(s).is_some_and(|p| projects.contains(&p))
        })
        .map(|s| (s.id.as_str(), s))
        .collect();
    let seed_ids: BTreeSet<_> = seeds
        .iter()
        .filter(|s| known.contains_key(s.id.as_str()))
        .map(|s| s.id.as_str())
        .collect();
    let tests: BTreeMap<_, _> = snapshot
        .entrypoints
        .iter()
        .filter(|e| {
            matches!(
                e.kind.as_str(),
                "test_function" | "go_test_candidate" | "java_test_candidate"
            )
        })
        .filter_map(|e| {
            e.symbol_id
                .as_deref()
                .filter(|id| known.contains_key(id))
                .map(|id| (id, e))
        })
        .collect();
    let mut work = 0;
    let mut exhausted = false;
    let mut incoming: BTreeMap<&str, Vec<Incoming<'_>>> = BTreeMap::new();
    // Snapshot facts are already project-scoped; each candidate is additionally
    // checked against both endpoint identities before entering the traversal.
    let relations = q.relations.unwrap_or(PathRelations::References);
    let references = if matches!(relations, PathRelations::Calls) {
        &snapshot.calls
    } else {
        &snapshot.bound_references
    };
    for edge in references {
        if work == budget {
            exhausted = true;
            break;
        }
        work += 1;
        let Some(source) = edge
            .reference
            .caller
            .as_deref()
            .filter(|id| known.contains_key(id))
        else {
            continue;
        };
        if excluded.contains(edge.reference.document_id.as_str()) {
            continue;
        }
        for target in &edge.candidates {
            if work == budget {
                exhausted = true;
                break;
            }
            work += 1;
            if known.contains_key(target.as_str())
                && project(known[source]) == project(known[target.as_str()])
            {
                incoming.entry(target).or_default().push(Incoming {
                    source,
                    target,
                    evidence: impact::Evidence::Reference(edge),
                });
            }
        }
        if exhausted {
            break;
        }
    }
    if matches!(relations, PathRelations::All) && !exhausted {
        for edge in &snapshot.inheritance {
            if work == budget {
                exhausted = true;
                break;
            }
            work += 1;
            let source = edge.derived.as_str();
            if !known.contains_key(source) {
                continue;
            }
            for target in &edge.candidates {
                if work == budget {
                    exhausted = true;
                    break;
                }
                work += 1;
                if known.contains_key(target.as_str())
                    && project(known[source]) == project(known[target.as_str()])
                {
                    incoming.entry(target).or_default().push(Incoming {
                        source,
                        target,
                        evidence: impact::Evidence::Inheritance(edge),
                    });
                }
            }
            if exhausted {
                break;
            }
        }
        for edge in &snapshot.overrides {
            if work == budget {
                exhausted = true;
                break;
            }
            work += 1;
            let source = edge.derived.as_str();
            if !known.contains_key(source) {
                continue;
            }
            for target in &edge.candidates {
                if work == budget {
                    exhausted = true;
                    break;
                }
                work += 1;
                if known.contains_key(target.as_str())
                    && project(known[source]) == project(known[target.as_str()])
                {
                    incoming.entry(target).or_default().push(Incoming {
                        source,
                        target,
                        evidence: impact::Evidence::Override(edge),
                    });
                }
            }
            if exhausted {
                break;
            }
        }
    }
    for edges in incoming.values_mut() {
        edges.sort_by_key(|e| (e.source, e.target, e.evidence.location()));
    }
    let mut visited = seed_ids.clone();
    let mut queue: VecDeque<_> = seed_ids.iter().map(|id| (*id, 0)).collect();
    let mut witness: BTreeMap<&str, Incoming<'_>> = BTreeMap::new();
    let mut depths: BTreeMap<_, _> = seed_ids.iter().map(|id| (*id, 0)).collect();
    let mut depth_limited = false;
    while let Some((target, level)) = queue.pop_front() {
        depths.insert(target, level);
        let next = incoming.get(target).map(Vec::as_slice).unwrap_or(&[]);
        if level == depth {
            depth_limited |= next.iter().any(|e| !visited.contains(e.source));
            continue;
        }
        for edge in next {
            if work == budget {
                exhausted = true;
                break;
            }
            work += 1;
            if visited.insert(edge.source) {
                witness.insert(edge.source, *edge);
                queue.push_back((edge.source, level + 1));
            }
        }
        if exhausted {
            break;
        }
    }
    let reached: Vec<_> = tests
        .keys()
        .filter(|id| depths.contains_key(**id))
        .copied()
        .collect();
    let items:Vec<_>=reached.iter().take(limit).map(|id|{
        let mut at=*id;let mut nodes=vec![known[at]];let mut steps=Vec::new();
        while !seed_ids.contains(at){let Some(edge)=witness.get(at)else{break;};steps.push(json!({"source_symbol_id":edge.source,"target_symbol_id":edge.target,"relation":edge.evidence.location().0,"evidence":edge.evidence.value()}));at=edge.target;nodes.push(known[at]);}
        let symbol=known[*id];
        let mut commands=snapshot.test_commands.get(*id).cloned().unwrap_or_default();
        if commands.is_empty()&&symbol.path.ends_with(".py") {commands.push(json!({"kind":"pytest_candidate","argv":["python","-m","pytest",symbol.path,"-k",symbol.name],"filter_mode":"pytest_expression","configuration_review_required":true,"executed":false}));}
        json!({"symbol":symbol,"entrypoint":tests[*id],"reason":if seed_ids.contains(*id){"changed_test"}else{"static_dependency"},"seed_symbol_id":at,"path":{"symbols":nodes,"steps":steps,"complete_to_seed":seed_ids.contains(at)},"commands":commands.iter().take(8).collect::<Vec<_>>(),"commands_total":commands.len(),"commands_truncated":commands.len()>8})
    }).collect();
    let manifests: BTreeSet<_> = snapshot
        .cargo_targets
        .iter()
        .filter(|t| {
            seeds.is_empty()
                || seeds.iter().any(|s| {
                    snapshot
                        .document_scopes
                        .get(&s.document_id)
                        .and_then(|d| d.project.as_deref())
                        == t["project"].as_str()
                })
        })
        .filter_map(|t| t["manifest_path"].as_str())
        .collect();
    let fallback:Vec<_>=manifests.iter().take(20).map(|manifest|json!({"kind":"cargo_full_suite_candidate","argv":["cargo","test","--manifest-path",manifest,"--workspace","--all-targets"],"configuration_review_required":true,"executed":false})).collect();
    let unresolved = references
        .iter()
        .filter(|e| e.candidates.is_empty())
        .count();
    let ambiguous = references.iter().filter(|e| e.candidates.len() > 1).count();
    json!({"status":if seed_ids.is_empty(){"no_valid_seeds"}else{"related_tests"},"items":items,"related_tests_total_in_examined_graph":reached.len(),"truncated":reached.len()>limit,"observed_test_entrypoints":tests.len(),
        "seeds":seeds,"index_rev":snapshot.index_revision,"view":"test_cfg_overlay_of_fresh_captured_inputs","limits":{"depth":depth,"tests":limit,"work":budget},"work":work,"work_exhausted":exhausted,"depth_limited":depth_limited,
        "coverage_complete":false,"test_discovery_complete":false,"runtime_coverage_measured":false,"safe_to_skip_unlisted_tests":false,"absence_proven":false,
        "filesystem_freshness_checked":false,"commands_require_matching_captured_sources":true,
        "recommendation":"run_full_suite_for_relevant_build_configurations; listed commands are investigation candidates",
        "fallback_commands":fallback,"fallback_manifests_total":manifests.len(),"fallback_commands_truncated":manifests.len()>20,
        "unresolved_reference_sites":unresolved,"ambiguous_reference_sites":ambiguous,"ambiguous_candidates_included":true,
        "stale_documents":snapshot.stale_documents.iter().take(50).collect::<Vec<_>>(),"stale_documents_total":snapshot.stale_documents.len(),"unindexed_documents":snapshot.unindexed_documents.iter().take(50).collect::<Vec<_>>(),"unindexed_documents_total":snapshot.unindexed_documents.len(),"parse_errors":snapshot.parse_errors.iter().take(50).collect::<Vec<_>>(),"parse_errors_total":snapshot.parse_errors.len(),
        "binding_diagnostics":snapshot.binding_diagnostics.iter().take(50).collect::<Vec<_>>(),"binding_diagnostics_total":snapshot.binding_diagnostics.len(),"rust_build_contexts":snapshot.rust_build_contexts.iter().take(50).collect::<Vec<_>>(),"rust_build_contexts_total":snapshot.rust_build_contexts.len(),
        "limitations":["static relationships only; generated/dynamic/unsupported tests may be absent","unknown cfg stays uncertain; explicit features/target retain captured scope","test executors and build success are not verified by this query","Rust commands use substring filters; custom harness commands are unfiltered","no historical command is safe to run against a different source state"]})
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        store: Store,
        _dir: tempfile::TempDir,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(&dir.path().join("tests.duckdb")).unwrap();
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/code-intelligence/affected-tests");
            for relative in [
                "Cargo.toml",
                "src/lib.rs",
                "src/worker.rs",
                "src/external_tests.rs",
                "tests/with-hyphen.rs",
            ] {
                Self::put(
                    &store,
                    relative,
                    &std::fs::read_to_string(root.join(relative)).unwrap(),
                    1,
                    "fixture",
                );
            }
            store.index_code().unwrap();
            Self { store, _dir: dir }
        }
        fn put(store: &Store, id: &str, source: &str, revision: i64, project: &str) {
            store
                .upsert_document(&crate::models::Document {
                    id: id.into(),
                    uri: format!("file:///{project}/{id}"),
                    source_file: Some(format!("/{project}/{id}")),
                    content: source.into(),
                    revision,
                    wing: Some(project.into()),
                    ..Default::default()
                })
                .unwrap();
        }
        fn q(&self, symbol: &str) -> AffectedTestsQuery {
            serde_json::from_value(json!({"symbols":[symbol],"project":"fixture"})).unwrap()
        }
        fn run(&self, q: &AffectedTestsQuery) -> Value {
            self.store.code_affected_tests(q).unwrap()
        }
    }
    use std::path::PathBuf;
    fn names(result: &Value) -> Vec<String> {
        let mut names: Vec<_> = result["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["symbol"]["name"].as_str().unwrap().to_string())
            .collect();
        names.sort();
        names
    }
    fn labels(key: &str) -> Vec<String> {
        serde_json::from_str::<Value>(include_str!(
            "../../tests/fixtures/code-intelligence/affected-tests/expected.json"
        ))
        .unwrap()[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().into())
            .collect()
    }
    #[test]
    fn unit_external_integration_and_callback_tests_have_real_witnesses() {
        let f = Fixture::new();
        let result = f.run(&f.q("api"));
        assert_eq!(names(&result), labels("references_tests"), "{result}");
        assert_eq!(result["coverage_complete"], false);
        assert_eq!(result["safe_to_skip_unlisted_tests"], false);
        assert!(!result["fallback_commands"].as_array().unwrap().is_empty());
        for item in result["items"].as_array().unwrap() {
            assert_eq!(item["path"]["complete_to_seed"], true);
            assert_eq!(item["path"]["symbols"][1]["name"], "api");
            assert!(!item["commands"].as_array().unwrap().is_empty());
            for (step, node) in item["path"]["steps"]
                .as_array()
                .unwrap()
                .iter()
                .zip(item["path"]["symbols"].as_array().unwrap())
            {
                assert_eq!(
                    step["evidence"]["reference"]["content_hash"],
                    node["content_hash"]
                );
            }
        }
        let mut calls = f.q("api");
        calls.relations = Some(PathRelations::Calls);
        assert_eq!(names(&f.run(&calls)), labels("calls_tests"));
        let deeper = f.run(&f.q("value"));
        assert_eq!(names(&deeper), labels("references_tests"));
        assert!(deeper["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["path"]["steps"].as_array().unwrap().len() == 2));
    }
    #[test]
    fn command_vectors_preserve_hyphen_targets_ignore_and_custom_harness() {
        let f = Fixture::new();
        let result = f.run(&f.q("api"));
        let integration = result["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["symbol"]["name"] == "integration_case")
            .unwrap();
        let argv = &integration["commands"][0]["argv"];
        assert!(argv
            .as_array()
            .unwrap()
            .windows(2)
            .any(|v| v[0] == "--test" && v[1] == "with-hyphen"));
        let ignored = result["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["symbol"]["name"] == "ignored_case")
            .unwrap();
        assert!(ignored["commands"][0]["argv"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "--ignored"));
        Fixture::put(
            &f.store,
            "Cargo.toml",
            "[package]\nname='affected-fixture'\nversion='0.1.0'\n[lib]\nharness=false\n",
            2,
            "fixture",
        );
        f.store.index_code().unwrap();
        let result = f.run(&f.q("api"));
        let unit = result["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["symbol"]["name"] == "unit_direct")
            .unwrap();
        assert_eq!(unit["commands"][0]["filter_applied"], false);
        assert!(!unit["commands"][0]["argv"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "unit_direct"));
    }
    #[test]
    fn test_attribute_names_are_exact_and_never_inferred_from_doc_strings() {
        let f = Fixture::new();
        Fixture::put(&f.store,"src/lib.rs","pub fn api(){} #[doc=\"[test]\"]fn doc_fake(){api();} #[tokio::test_fake]fn prefix_fake(){api();} #[::tokio::test]async fn real_async(){api();}",2,"fixture");
        f.store.index_code().unwrap();
        let result = f.run(&f.q("api"));
        let names = names(&result);
        assert!(names.contains(&"real_async".into()));
        assert!(!names.contains(&"doc_fake".into()) && !names.contains(&"prefix_fake".into()));
    }
    #[test]
    fn direct_test_seeds_limits_and_stale_inputs_cannot_authorize_ci_omissions() {
        let f = Fixture::new();
        let mut q = f.q("unit_direct");
        q.max_work = Some(1);
        let result = f.run(&q);
        assert_eq!(result["items"][0]["reason"], "changed_test");
        assert_eq!(result["items"][0]["path"]["steps"], json!([]));
        q = f.q("api");
        q.limit = Some(1);
        let result = f.run(&q);
        assert_eq!(result["truncated"], true);
        assert_eq!(result["items"].as_array().unwrap().len(), 1);
        q = f.q("value");
        q.depth = Some(1);
        let result = f.run(&q);
        assert!(names(&result).is_empty());
        assert_eq!(result["depth_limited"], true);
        q = f.q("api");
        q.max_work = Some(1);
        let result = f.run(&q);
        assert_eq!(result["work_exhausted"], true);
        assert_eq!(result["safe_to_skip_unlisted_tests"], false);
        Fixture::put(&f.store, "src/lib.rs", "pub fn api(){}", 2, "fixture");
        let result = f.run(&f.q("api"));
        assert_eq!(result["status"], "seed_unavailable");
        assert!(result["stale_documents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "src/lib.rs"));
    }
    #[test]
    fn project_isolation_ambiguity_and_test_cfg_symbol_identity_are_explicit() {
        let f = Fixture::new();
        Fixture::put(
            &f.store,
            "other.rs",
            "pub fn api(){} #[test]fn foreign_test(){api();}",
            1,
            "other",
        );
        f.store.index_code().unwrap();
        assert_eq!(names(&f.run(&f.q("api"))), labels("references_tests"));
        let mut q = f.q("api");
        q.project = None;
        assert_eq!(f.run(&q)["status"], "ambiguous_seeds");
        let manifest="[package]\nname='affected-fixture'\nversion='0.1.0'\n[features]\nfast=[]\n[package.metadata.rag.graph]\nversion=1\ntarget_kind='lib'\nfeatures=['fast']\ndefault_features=false\n";
        Fixture::put(&f.store, "Cargo.toml", manifest, 2, "fixture");
        f.store.index_code().unwrap();
        let normal = f.store.code_snapshot_in_project(Some("fixture")).unwrap();
        let api = normal.symbols.iter().find(|s| s.name == "api").unwrap();
        let old_id = api.id.clone();
        let result = f.run(&f.q(&old_id));
        assert_eq!(result["selectors"][0]["test_context_id_remapped"], true);
        assert!(result["rust_build_contexts"][0]["cfg_flags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "test"));
        assert!(result["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["symbol"]["name"] == "external_case"));
        Fixture::put(
            &f.store,
            "Cargo.toml",
            &manifest.replace("features=['fast']", "features=[]"),
            3,
            "fixture",
        );
        f.store.index_code().unwrap();
        assert_eq!(f.run(&f.q(&old_id))["status"], "seed_unavailable");
    }
    #[test]
    fn test_cfg_imports_do_not_bind_the_later_inactive_production_alias() {
        let f = Fixture::new();
        let source="pub fn api(){} pub fn wrong(){} #[cfg(test)]use crate::api as selected; #[cfg(not(test))]use crate::wrong as selected; #[cfg(test)]#[test]fn checks_cfg(){selected();}";
        Fixture::put(&f.store, "src/lib.rs", source, 2, "fixture");
        f.store.index_code().unwrap();
        assert!(names(&f.run(&f.q("api"))).contains(&"checks_cfg".into()));
        assert!(names(&f.run(&f.q("wrong"))).is_empty());
        let manifest="[package]\nname='affected-fixture'\nversion='0.1.0'\n[package.metadata.rag.graph]\nversion=1\ntarget_kind='lib'\nfeatures=[]\ndefault_features=false\n";
        Fixture::put(&f.store, "Cargo.toml", manifest, 2, "fixture");
        f.store.index_code().unwrap();
        assert_eq!(names(&f.run(&f.q("api"))), ["checks_cfg"]);
        assert!(names(&f.run(&f.q("wrong"))).is_empty());
    }
}
