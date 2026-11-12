//! Static related-test discovery with witnesses; never a safe CI omission policy.
use super::impact::Incoming;
use super::*;
mod hosts;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

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
    /// Include captured PHP test-host/base/trait availability dependencies (default true), separately from call mode.
    pub include_host_dependencies: Option<bool>,
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
                "test_function"
                    | "go_test_candidate"
                    | "java_test_candidate"
                    | "phpunit_test_candidate"
            )
        })
        .filter_map(|e| {
            let body = e.symbol_id.as_deref().filter(|id| known.contains_key(id))?;
            if let Some(context) = &e.test_context {
                let host = known.get(context.host_symbol_id.as_str())?;
                if !known.contains_key(context.lexical_owner_symbol_id.as_str())
                    || host.document_id != context.host_document_id
                    || host.content_hash != context.host_content_hash
                    || host.path != context.host_path
                {
                    return None;
                }
                Some((
                    (
                        body,
                        Some(context.host_symbol_id.as_str()),
                        Some(context.effective_method.as_str()),
                    ),
                    e,
                ))
            } else {
                Some(((body, None, None), e))
            }
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
        edges.sort_by_key(|e| (e.source, e.evidence.location()));
    }
    let known_ids: HashSet<_> = known.keys().copied().collect();
    let owner_projects: HashMap<_, _> = known
        .iter()
        .map(|(id, s)| (*id, project(s).flatten()))
        .collect();
    let test_ids: HashSet<_> = tests.keys().map(|key| key.0).collect();
    let fresh_seeds: Vec<_> = seeds
        .iter()
        .copied()
        .filter(|s| seed_ids.contains(s.id.as_str()))
        .collect();
    let walked = impact::context_walk::walk(
        &fresh_seeds,
        &incoming,
        &known_ids,
        &owner_projects,
        impact::context_walk::Options {
            limit: usize::MAX,
            depth_limit: depth,
            include_paths: true,
            work_limit: budget.saturating_sub(work),
            output_symbols: Some(&test_ids),
            deduplicate_symbols: false,
        },
    );
    work += walked.diagnostics["work_steps"].as_u64().unwrap_or(0) as usize;
    exhausted |= walked.diagnostics["work_limited"] == true;
    let depth_limited = walked.depth_limited;
    let mut tests_by_body: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for (key, entry) in &tests {
        tests_by_body.entry(key.0).or_default().push((*key, *entry));
    }
    let mut reached = BTreeMap::new();
    let mut invocation_match_work = 0;
    'match_tests: for item in &walked.items {
        let Some(id) = item["symbol_id"].as_str() else {
            continue;
        };
        let path = &item["state_path"];
        let context = path["steps"][0]["source_context"].as_str();
        for (key, entry) in tests_by_body.get(id).into_iter().flatten() {
            if work == budget {
                exhausted = true;
                break 'match_tests;
            }
            work += 1;
            invocation_match_work += 1;
            let compatible = entry.test_context.as_ref().is_none_or(|test| {
                if test.trait_body {
                    context.is_none_or(|owner| owner == test.lexical_owner_symbol_id)
                } else {
                    context.is_none()
                }
            });
            if compatible {
                reached.entry(*key).or_insert_with(|| path.clone());
            }
        }
    }
    let host_ids: HashSet<_> = tests.keys().filter_map(|key| key.1).collect();
    let host_walk = if q.include_host_dependencies.unwrap_or(true) {
        hosts::walk(
            snapshot,
            &fresh_seeds,
            &known,
            &host_ids,
            depth.saturating_sub(1),
            budget.saturating_sub(work),
        )
    } else {
        hosts::Hosts {
            paths: BTreeMap::new(),
            work: 0,
            exhausted: false,
            depth_limited: false,
            truncated: false,
            diagnostics: json!({"applied":false,"explicitly_disabled":true}),
        }
    };
    work += host_walk.work;
    exhausted |= host_walk.exhausted;
    let mut host_match_work = 0;
    let mut host_matching_exhausted = false;
    let mut direct_host_matches = 0;
    for (key, entry) in &tests {
        let Some(host_id) = key.1 else {
            continue;
        };
        let Some(path) = host_walk.paths.get(host_id) else {
            continue;
        };
        // Direct source-identity joins remain available like directly changed test bodies.
        let direct = seed_ids.contains(host_id);
        if direct {
            direct_host_matches += 1;
        } else {
            if work == budget {
                exhausted = true;
                host_matching_exhausted = true;
                continue;
            }
            work += 1;
            host_match_work += 1;
        }
        let path = hosts::attach(entry, known[key.0], known[host_id], path);
        let length = path["steps"].as_array().map_or(usize::MAX, Vec::len);
        let replace = reached
            .get(key)
            .is_none_or(|old| old["steps"].as_array().map_or(usize::MAX, Vec::len) > length);
        if replace {
            reached.insert(*key, path);
        }
    }
    for key in tests.keys().filter(|key| seed_ids.contains(key.0)) {
        reached.insert(*key,json!({"steps":[],"seed_symbol_id":key.0,"seed_context":null,"complete_to_seed":true,"captured_trait_frames_matched":true,"runtime_context_correlation_proven":false}));
    }
    let items: Vec<_> = reached.iter().take(limit).map(|(key,path)| {
        let id=key.0;
        let entry=tests[key];
        let symbol=known[id];
        let steps=path["steps"].as_array().cloned().unwrap_or_default();
        let mut current=id;
        let mut nodes=vec![symbol];
        let mut contexts=BTreeSet::new();
        let mut expected_context=None;
        let mut contiguous=true;
        for (index,step) in steps.iter().enumerate() {
            if step["source_symbol_id"].as_str()!=Some(current) || index>0 && step["source_context"].as_str()!=expected_context {contiguous=false;break;}
            let Some(target)=step["target_symbol_id"].as_str().filter(|id|known.contains_key(id)) else {contiguous=false;break;};
            for field in ["source_context","target_context"] {
                if let Some(owner)=step[field].as_str() {
                    if !known.contains_key(owner) {contiguous=false;break;}
                    contexts.insert(owner);
                }
            }
            expected_context=step["target_context"].as_str();
            current=target;nodes.push(known[target]);
        }
        let complete=contiguous && path["complete_to_seed"]==true && path["seed_symbol_id"].as_str()==Some(current) && path["seed_context"].is_null() && expected_context.is_none() && seed_ids.contains(current);
        let mut commands=snapshot.test_commands.get(id).cloned().unwrap_or_default();
        if commands.is_empty()&&symbol.path.ends_with(".py") {commands.push(json!({"kind":"pytest_candidate","argv":["python","-m","pytest",symbol.path,"-k",symbol.name],"filter_mode":"pytest_expression","configuration_review_required":true,"executed":false}));}
        // No guessed Composer root, PHPUnit executable, filter or configuration.
        json!({"symbol":symbol,"entrypoint":entry,"test_host":key.1.map(|host|known[host]),"effective_test_method":key.2,"reason":if seed_ids.contains(id){"changed_test"}else if path["test_host_dependency"]==true{"captured_test_host_dependency"}else{"static_dependency"},"seed_symbol_id":current,"path":{"symbols":nodes,"steps":steps,"complete_to_seed":complete,"trait_context_symbols":contexts.into_iter().map(|id|known[id]).collect::<Vec<_>>(),"captured_trait_frames_matched":true,"runtime_context_correlation_proven":false},"commands":commands.iter().take(8).collect::<Vec<_>>(),"commands_total":commands.len(),"commands_truncated":commands.len()>8,"command_status":if commands.is_empty(){"no_qualified_command_candidate"}else{"candidate_requires_review"}})
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
        .filter(|e| e.has_unknown_targets())
        .count();
    let ambiguous = references
        .iter()
        .filter(|e| e.candidates.len() > 1 || e.resolution == "ambiguous")
        .count();
    let mut report = json!({"status":if seed_ids.is_empty(){"no_valid_seeds"}else{"related_tests"},"items":items,"related_tests_total_in_examined_graph":reached.len(),"truncated":reached.len()>limit,"observed_test_entrypoints":tests.len(),
        "seeds":seeds,"index_rev":snapshot.index_revision,"view":"test_cfg_overlay_of_fresh_captured_inputs","limits":{"depth":depth,"tests":limit,"work":budget},"work":work,"work_exhausted":exhausted,"depth_limited":depth_limited||host_walk.depth_limited,
        "coverage_complete":false,"test_discovery_complete":false,"runtime_coverage_measured":false,"safe_to_skip_unlisted_tests":false,"absence_proven":false,
        "filesystem_freshness_checked":false,"commands_require_matching_captured_sources":true,
        "recommendation":"run_full_suite_for_relevant_build_configurations; listed commands are investigation candidates",
        "fallback_commands":fallback,"fallback_manifests_total":manifests.len(),"fallback_commands_truncated":manifests.len()>20,
        "unresolved_reference_sites":unresolved,"ambiguous_reference_sites":ambiguous,"ambiguous_candidates_included":true,
        "stale_documents":snapshot.stale_documents.iter().take(50).collect::<Vec<_>>(),"stale_documents_total":snapshot.stale_documents.len(),"unindexed_documents":snapshot.unindexed_documents.iter().take(50).collect::<Vec<_>>(),"unindexed_documents_total":snapshot.unindexed_documents.len(),"parse_errors":snapshot.parse_errors.iter().take(50).collect::<Vec<_>>(),"parse_errors_total":snapshot.parse_errors.len(),
        "binding_diagnostics":snapshot.binding_diagnostics.iter().take(50).collect::<Vec<_>>(),"binding_diagnostics_total":snapshot.binding_diagnostics.len(),"rust_build_contexts":snapshot.rust_build_contexts.iter().take(50).collect::<Vec<_>>(),"rust_build_contexts_total":snapshot.rust_build_contexts.len(),
        "limitations":["static relationships only; generated/dynamic/unsupported tests may be absent","unknown cfg stays uncertain; explicit features/target retain captured scope","test executors and build success are not verified by this query","Rust commands use substring filters; custom harness commands are unfiltered","no historical command is safe to run against a different source state"]});
    report["host_dependency_traversal"] = host_walk.diagnostics;
    report["host_invocation_match_work"] = host_match_work.into();
    report["direct_test_host_matches"] = direct_host_matches.into();
    report["host_dependencies_enabled"] = q.include_host_dependencies.unwrap_or(true).into();
    report["invocation_match_work"] = invocation_match_work.into();
    report["trait_context_traversal"] = walked.diagnostics;
    report["traversal_truncated"] =
        (walked.truncated || host_walk.truncated || host_matching_exhausted).into();
    report["traversal_metadata_incomplete"] = walked.metadata_incomplete.into();
    report
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
    fn php_snapshot(body: &str) -> CodeSnapshot {
        let mut doc = parse("php", "/fixture/tests.php", body, 1).unwrap();
        assert!(!doc.parse_errors);
        doc.project = Some("fixture".into());
        let mut snapshot = CodeSnapshot::default();
        snapshot.document_scopes.insert(
            "php".into(),
            DocumentScope {
                path: doc.path.clone(),
                project: doc.project.clone(),
            },
        );
        snapshot.symbols = doc.symbols.clone();
        let docs = HashMap::from([("php".into(), doc)]);
        snapshot.bound_references = resolve::references(&snapshot.symbols, &docs);
        snapshot.calls = snapshot
            .bound_references
            .iter()
            .filter(|e| e.reference.is_call)
            .cloned()
            .collect();
        snapshot.entrypoints = analysis::entrypoints(&snapshot.symbols, &docs);
        snapshot.inheritance = inheritance::edges(&snapshot.symbols, &docs);
        snapshot
    }
    const PHP_TESTS: &str = r"<?php
trait T {public function run(){self::leaf();}}
class A extends \PHPUnit\Framework\TestCase {use T;public static function leaf(){} public function testA(){$this->run();}}
class B extends \PHPUnit\Framework\TestCase {use T;public static function leaf(){} public function testB(){$this->run();}}
";
    fn php_query(symbol: &str) -> AffectedTestsQuery {
        serde_json::from_value(json!({"symbols":[symbol],"project":"fixture"})).unwrap()
    }
    #[test]
    fn phpunit_affected_tests_keep_compatible_trait_paths_and_changed_tests() {
        let snapshot = php_snapshot(PHP_TESTS);
        for (root, test, owner) in [("A::leaf", "testA", "A"), ("B::leaf", "testB", "B")] {
            let result = query(&snapshot, &php_query(root)).unwrap();
            assert_eq!(names(&result), [test], "{result}");
            let path = &result["items"][0]["path"];
            assert_eq!(path["complete_to_seed"], true);
            assert_eq!(path["steps"].as_array().unwrap().len(), 2);
            assert_eq!(path["symbols"][1]["qualified_name"], "T::run");
            assert_eq!(path["trait_context_symbols"][0]["qualified_name"], owner);
            assert_eq!(
                path["steps"][0]["target_context"],
                path["steps"][1]["source_context"]
            );
            assert_eq!(result["safe_to_skip_unlisted_tests"], false);
            assert!(result["items"][0]["commands"]
                .as_array()
                .unwrap()
                .is_empty());
        }
        let direct = query(&snapshot, &php_query("A::testA")).unwrap();
        assert_eq!(direct["items"][0]["reason"], "changed_test");
        assert_eq!(direct["items"][0]["path"]["steps"], json!([]));
        assert_eq!(direct["items"][0]["path"]["complete_to_seed"], true);
    }
    #[test]
    fn phpunit_shared_trait_states_do_not_consume_test_limit() {
        let snapshot = php_snapshot(
            r"<?php
class Root {public static function leaf(){}}
trait T {public function run(){self::leaf();}}
class A extends \PHPUnit\Framework\TestCase {use T;public static function leaf(){Root::leaf();} public function testA(){$this->run();}}
class B extends \PHPUnit\Framework\TestCase {use T;public static function leaf(){Root::leaf();} public function testB(){$this->run();}}
",
        );
        let result = query(&snapshot, &php_query("Root::leaf")).unwrap();
        assert_eq!(names(&result), ["testA", "testB"], "{result}");
        for item in result["items"].as_array().unwrap() {
            assert_eq!(item["path"]["steps"].as_array().unwrap().len(), 3);
            assert_eq!(item["path"]["complete_to_seed"], true);
            assert_eq!(
                item["path"]["steps"][0]["target_context"],
                item["path"]["steps"][1]["source_context"]
            );
        }
        let mut q = php_query("Root::leaf");
        q.limit = Some(1);
        let capped = query(&snapshot, &q).unwrap();
        assert_eq!(capped["related_tests_total_in_examined_graph"], 2);
        assert_eq!(capped["items"].as_array().unwrap().len(), 1);
        assert_eq!(capped["truncated"], true);
    }
    #[test]
    fn phpunit_trait_witnesses_guard_missing_metadata_modes_and_budgets() {
        let mut snapshot = php_snapshot(&PHP_TESTS.replace("$this->run();", "$f=$this->run(...);"));
        let q = php_query("A::leaf");
        assert_eq!(names(&query(&snapshot, &q).unwrap()), ["testA"]);
        let mut calls = php_query("A::leaf");
        calls.relations = Some(PathRelations::Calls);
        assert!(names(&query(&snapshot, &calls).unwrap()).is_empty());
        for edge in &mut snapshot.bound_references {
            if edge
                .context_evidence
                .as_ref()
                .is_some_and(|e| e.kind == "php_captured_trait_entry")
            {
                edge.context_evidence = None;
            }
        }
        let broken = query(&snapshot, &q).unwrap();
        assert!(names(&broken).is_empty(), "{broken}");
        assert_eq!(broken["traversal_metadata_incomplete"], true);
        let snapshot = php_snapshot(PHP_TESTS);
        for budget in [1, 5, 10, 15, 20, 30, 50, 100] {
            let mut q = php_query("A::leaf");
            q.max_work = Some(budget);
            let result = query(&snapshot, &q).unwrap();
            assert!(
                result["work"].as_u64().unwrap() <= budget as u64,
                "{result}"
            );
        }
        let mut shallow = php_query("A::leaf");
        shallow.depth = Some(1);
        let result = query(&snapshot, &shallow).unwrap();
        assert!(names(&result).is_empty());
        assert_eq!(result["depth_limited"], true);
    }
    #[test]
    fn affected_test_witnesses_preserve_depth_twenty_and_exclude_stale_bridges() {
        let mut body = String::from("<?php function f0(){}");
        for n in 1..20 {
            body.push_str(&format!("function f{n}(){{f{}();}}", n - 1));
        }
        body.push_str("class Tests extends \\PHPUnit\\Framework\\TestCase {public function testDeep(){f19();}}");
        let mut snapshot = php_snapshot(&body);
        let mut q = php_query("f0");
        q.depth = Some(20);
        let result = query(&snapshot, &q).unwrap();
        assert_eq!(names(&result), ["testDeep"]);
        assert_eq!(
            result["items"][0]["path"]["steps"]
                .as_array()
                .unwrap()
                .len(),
            20
        );
        assert_eq!(result["items"][0]["path"]["complete_to_seed"], true);
        snapshot.stale_documents.push("php".into());
        let stale = query(&snapshot, &q).unwrap();
        assert!(names(&stale).is_empty());
        assert_eq!(stale["status"], "no_valid_seeds");
    }
    #[test]
    fn phpunit_trait_tests_are_distinct_host_and_alias_invocations() {
        let snapshot = php_snapshot(
            r"<?php class Root {public static function leaf(){}} trait Tests {public function testWorks(){self::leaf();}} class A extends \PHPUnit\Framework\TestCase {use Tests {testWorks as testAlias;}public static function leaf(){Root::leaf();}} class B extends \PHPUnit\Framework\TestCase {use Tests;public static function leaf(){Root::leaf();}} ",
        );
        let result = query(&snapshot, &php_query("A::leaf")).unwrap();
        assert_eq!(result["items"].as_array().unwrap().len(), 2, "{result}");
        let mut methods: Vec<_> = result["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                assert_eq!(item["symbol"]["qualified_name"], "Tests::testWorks");
                assert_eq!(item["test_host"]["qualified_name"], "A");
                assert_eq!(item["path"]["complete_to_seed"], true);
                assert_eq!(
                    item["path"]["steps"][0]["source_context"],
                    item["entrypoint"]["test_context"]["lexical_owner_symbol_id"]
                );
                item["effective_test_method"].as_str().unwrap()
            })
            .collect();
        methods.sort();
        assert_eq!(methods, ["testAlias", "testWorks"]);
        let b = query(&snapshot, &php_query("B::leaf")).unwrap();
        assert_eq!(b["items"].as_array().unwrap().len(), 1);
        assert_eq!(b["items"][0]["test_host"]["qualified_name"], "B");
        let shared = query(&snapshot, &php_query("Root::leaf")).unwrap();
        assert_eq!(shared["items"].as_array().unwrap().len(), 3, "{shared}");
        let changed = query(&snapshot, &php_query("Tests::testWorks")).unwrap();
        assert_eq!(changed["items"].as_array().unwrap().len(), 3);
        assert!(changed["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["reason"] == "changed_test"));
    }
    #[test]
    fn phpunit_inherited_tests_preserve_overrides_and_lexical_trait_owner() {
        let snapshot = php_snapshot(
            r"<?php function work(){} function other(){} trait Checks {public function testTrait(){self::leaf();}} abstract class Base extends \PHPUnit\Framework\TestCase {use Checks;public static function leaf(){work();}public function testBase(){work();}} class A extends Base {} class B extends Base {public function testBase(){other();}}",
        );
        let result = query(&snapshot, &php_query("work")).unwrap();
        let mut cases: Vec<_> = result["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                assert_eq!(item["path"]["complete_to_seed"], true);
                (
                    item["test_host"]["qualified_name"].as_str().unwrap(),
                    item["effective_test_method"].as_str().unwrap(),
                )
            })
            .collect();
        cases.sort();
        assert_eq!(
            cases,
            [("A", "testBase"), ("A", "testTrait"), ("B", "testTrait")]
        );
        let other = query(&snapshot, &php_query("other")).unwrap();
        assert_eq!(other["items"].as_array().unwrap().len(), 1);
        assert_eq!(other["items"][0]["test_host"]["qualified_name"], "B");
        let mut invalid = php_snapshot(PHP_TESTS);
        for entry in &mut invalid.entrypoints {
            if let Some(c) = &mut entry.test_context {
                c.host_content_hash = "outdated".into();
            }
        }
        assert!(query(&invalid, &php_query("A::leaf")).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    fn invocations(result: &Value) -> BTreeSet<(String, String)> {
        result["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                (
                    item["test_host"]["qualified_name"].as_str().unwrap().into(),
                    item["effective_test_method"].as_str().unwrap().into(),
                )
            })
            .collect()
    }
    const HOST_TESTS: &str = r"<?php trait Tests {public function testWorks(){self::leaf();}} abstract class Base extends \PHPUnit\Framework\TestCase {} class A extends Base {use Tests {testWorks as testAlias;}public static function leaf(){}} class B extends Base {use Tests;public static function leaf(){}}";
    #[test]
    fn phpunit_changed_host_recommends_only_its_effective_test_invocations() {
        let snapshot = php_snapshot(HOST_TESTS);
        let result = query(&snapshot, &php_query("A")).unwrap();
        assert_eq!(
            invocations(&result),
            BTreeSet::from([
                ("A".into(), "testAlias".into()),
                ("A".into(), "testWorks".into())
            ]),
            "{result}"
        );
        for item in result["items"].as_array().unwrap() {
            assert_eq!(item["reason"], "captured_test_host_dependency");
            assert_eq!(item["path"]["complete_to_seed"], true);
            assert_eq!(item["path"]["steps"].as_array().unwrap().len(), 1);
            let step = &item["path"]["steps"][0];
            assert_eq!(step["relation"], "phpunit_host_candidate");
            assert_eq!(step["evidence"]["call_edge"], false);
            assert!(step["evidence"].get("reference").is_none());
            assert_eq!(
                step["source_context"],
                item["entrypoint"]["test_context"]["lexical_owner_symbol_id"]
            );
        }
        let mut disabled = php_query("A");
        disabled.include_host_dependencies = Some(false);
        assert!(query(&snapshot, &disabled).unwrap()["items"]
            .as_array()
            .unwrap()
            .is_empty());
        let mut bounded = php_query("A");
        bounded.max_work = Some(1);
        let direct = query(&snapshot, &bounded).unwrap();
        assert_eq!(invocations(&direct), invocations(&result));
        assert!(direct["work"].as_u64().unwrap() <= 1);
    }
    #[test]
    fn phpunit_base_and_nested_trait_changes_keep_bounded_structural_witnesses() {
        let snapshot = php_snapshot(&HOST_TESTS.replace(
            "class A extends Base {use Tests",
            "trait Outer {use Tests;} class A extends Base {use Outer",
        ));
        let base = query(&snapshot, &php_query("Base")).unwrap();
        assert_eq!(invocations(&base).len(), 3, "{base}");
        assert!(base["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["path"]["steps"].as_array().unwrap().len() == 2));
        let trait_change = query(&snapshot, &php_query("Tests")).unwrap();
        assert_eq!(invocations(&trait_change).len(), 3, "{trait_change}");
        let mut q = php_query("Tests");
        q.depth = Some(2);
        let shallow = query(&snapshot, &q).unwrap();
        assert_eq!(
            invocations(&shallow),
            BTreeSet::from([("B".into(), "testWorks".into())])
        );
        assert_eq!(shallow["depth_limited"], true);
        let mut capped = php_query("Base");
        capped.limit = Some(1);
        let result = query(&snapshot, &capped).unwrap();
        assert_eq!(result["related_tests_total_in_examined_graph"], 3);
        assert_eq!(result["items"].as_array().unwrap().len(), 1);
        assert_eq!(result["truncated"], true);
        for budget in [1, 5, 10, 20, 50, 100] {
            let mut q = php_query("Base");
            q.max_work = Some(budget);
            let result = query(&snapshot, &q).unwrap();
            assert!(
                result["work"].as_u64().unwrap() <= budget as u64,
                "{result}"
            );
        }
    }
    #[test]
    fn phpunit_host_identity_does_not_merge_shared_inherited_nontrait_bodies() {
        let mut snapshot = php_snapshot(
            r"<?php function dependency(){} abstract class Base extends \PHPUnit\Framework\TestCase {public function testInherited(){dependency();}} class A extends Base {} class B extends Base {}",
        );
        let a = query(&snapshot, &php_query("A")).unwrap();
        assert_eq!(
            invocations(&a),
            BTreeSet::from([("A".into(), "testInherited".into())]),
            "{a}"
        );
        let body = query(&snapshot, &php_query("Base::testInherited")).unwrap();
        assert_eq!(invocations(&body).len(), 2);
        assert!(body["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["reason"] == "changed_test" && i["path"]["steps"] == json!([])));
        for edge in &mut snapshot.inheritance {
            edge.candidates.clear();
        }
        let invalid = query(&snapshot, &php_query("Base")).unwrap();
        assert!(invalid["items"].as_array().unwrap().is_empty(), "{invalid}");
    }
}
