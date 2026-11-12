//! Bounded simple paths over captured symbol relationships, never runtime traces.
use super::*;
use std::collections::{BTreeMap, VecDeque};
mod contexts;

#[derive(Debug, Default, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PathRelations {
    #[default]
    Calls,
    References,
    All,
}
#[derive(Debug, Default, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PathDirection {
    #[default]
    Forward,
    Back,
    Both,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PathQuery {
    /// Exact symbol ID, qualified name or short name. Ambiguity is never guessed.
    pub from: String,
    pub to: String,
    pub from_path: Option<String>,
    pub to_path: Option<String>,
    /// Exact project/wing. Cross-project edges are always excluded.
    pub project: Option<String>,
    pub relations: Option<PathRelations>,
    pub direction: Option<PathDirection>,
    /// Include ambiguous targets within a selected context (default false); aggregate consumer alternatives remain labelled.
    pub include_ambiguous: Option<bool>,
    /// Maximum edges per simple path, 1..20 (default 6).
    pub max_depth: Option<usize>,
    /// Maximum returned paths, 1..20 (default 3).
    pub max_paths: Option<usize>,
    /// Shared candidate inspection/traversal budget, 1..200000 (default 20000).
    pub max_work: Option<usize>,
}
#[derive(Clone, Copy)]
struct Link<'a> {
    source: &'a str,
    target: &'a str,
    evidence: impact::Evidence<'a>,
    ambiguous: bool,
}
#[derive(Clone, Copy)]
struct Step<'a> {
    next: &'a str,
    link: Link<'a>,
    reversed: bool,
    ambiguous_target: bool,
}
struct Walk<'a> {
    nodes: Vec<&'a str>,
    contexts: Vec<Option<&'a str>>,
    steps: Vec<Step<'a>>,
}
impl CodeSnapshot {
    pub fn trace_path(&self, q: &PathQuery) -> serde_json::Value {
        let depth = q.max_depth.unwrap_or(6).clamp(1, 20);
        let limit = q.max_paths.unwrap_or(3).clamp(1, 20);
        let budget = q.max_work.unwrap_or(20000).clamp(1, 200000);
        let project = |s: &Symbol| {
            self.document_scopes
                .get(&s.document_id)
                .and_then(|d| d.project.as_deref())
        };
        let select = |name: &str, path: Option<&str>| -> Vec<&Symbol> {
            let mut found: Vec<_> = self
                .symbols
                .iter()
                .filter(|s| {
                    q.project.as_deref().is_none_or(|p| project(s) == Some(p))
                        && path.is_none_or(|p| p == s.path)
                        && (name == s.id || name == s.qualified_name || name == s.name)
                })
                .collect();
            // An exact ID takes precedence over coincident short/qualified names.
            if found.iter().any(|s| s.id == name) {
                found.retain(|s| s.id == name);
            }
            found.sort_by(|a, b| {
                (&a.path, a.start_byte, &a.id).cmp(&(&b.path, b.start_byte, &b.id))
            });
            found
        };
        let from = select(&q.from, q.from_path.as_deref());
        let to = select(&q.to, q.to_path.as_deref());
        let mut report = serde_json::json!({
            "status":"endpoint_unavailable", "paths":[], "index_rev":self.index_revision,
            "from":{"candidates":from.iter().take(50).collect::<Vec<_>>(),"total":from.len(),"truncated":from.len()>50},
            "to":{"candidates":to.iter().take(50).collect::<Vec<_>>(),"total":to.len(),"truncated":to.len()>50},
            "limits":{"max_depth":depth,"max_paths":limit,"max_work":budget},
            "work":0,"work_exhausted":false,"result_limit_reached":false,"depth_limit_reached":false,
            "stale_documents":self.stale_documents.iter().take(50).collect::<Vec<_>>(),"stale_documents_total":self.stale_documents.len(),
            "unindexed_documents":self.unindexed_documents.iter().take(50).collect::<Vec<_>>(),"unindexed_documents_total":self.unindexed_documents.len(),
            "parse_errors":self.parse_errors.iter().take(50).collect::<Vec<_>>(),"parse_errors_total":self.parse_errors.len(),
            "binding_diagnostics":self.binding_diagnostics.iter().take(50).collect::<Vec<_>>(),"binding_diagnostics_total":self.binding_diagnostics.len(),
            "diagnostic_lists_truncated":self.stale_documents.len()>50 || self.unindexed_documents.len()>50 || self.parse_errors.len()>50 || self.binding_diagnostics.len()>50,
            "view":"fresh_subset_of_revision_inputs",
            "runtime_path_proven":false,"absence_proven":false,
            "scope":"bounded_simple_symbol_paths_with_captured_trait_frames; candidate relationships retain source semantics"
        });
        if from.len() != 1 || to.len() != 1 {
            if from.len() > 1 || to.len() > 1 {
                report["status"] = "ambiguous_endpoints".into();
            }
            return report;
        }
        let start = from[0];
        let finish = to[0];
        if project(start) != project(finish) {
            report["status"] = "project_mismatch".into();
            return report;
        }
        let symbols: HashMap<_, _> = self
            .symbols
            .iter()
            .filter(|s| project(s) == project(start))
            .map(|s| (s.id.as_str(), s))
            .collect();
        let excluded: HashSet<_> = self
            .stale_documents
            .iter()
            .chain(&self.unindexed_documents)
            .chain(&self.parse_errors)
            .map(String::as_str)
            .collect();
        let fresh = |id: &str| {
            symbols
                .get(id)
                .is_some_and(|s| !excluded.contains(s.document_id.as_str()))
        };
        if !fresh(&start.id) || !fresh(&finish.id) {
            return report;
        }
        let mut frames =
            contexts::Frames::new(&symbols, &excluded, q.include_ambiguous.unwrap_or(false));
        let mut work = 0;
        let mut exhausted = false;
        let mut adjacency: BTreeMap<&str, Vec<Step<'_>>> = BTreeMap::new();
        let direction = q.direction.unwrap_or_default();
        let relations = q.relations.unwrap_or_default();
        let references = if matches!(relations, PathRelations::Calls) {
            &self.calls
        } else {
            &self.bound_references
        };
        let mut candidates: Vec<Link<'_>> = Vec::new();
        for edge in references {
            if work == budget {
                exhausted = true;
                break;
            }
            work += 1;
            let Some(source) = edge.reference.caller.as_deref() else {
                continue;
            };
            if !fresh(source) || excluded.contains(edge.reference.document_id.as_str()) {
                continue;
            }
            let missing = frames.missing(edge, &mut work, budget);
            if frames.exhausted {
                exhausted = true;
                break;
            }
            if missing {
                frames.incomplete = true;
                frames.pruned += edge.candidates.len();
                continue;
            }
            if edge.context_evidence.is_none()
                && (edge.candidates.len() != 1 || edge.resolution == "ambiguous")
                && !q.include_ambiguous.unwrap_or(false)
            {
                continue;
            }
            for target in &edge.candidates {
                if work == budget {
                    exhausted = true;
                    break;
                }
                work += 1;
                if frames.missing_target(edge, target) {
                    continue;
                }
                if fresh(target) {
                    candidates.push(Link {
                        source,
                        target,
                        evidence: impact::Evidence::Reference(edge),
                        ambiguous: edge.candidates.len() != 1 || edge.resolution == "ambiguous",
                    });
                }
            }
            if exhausted {
                break;
            }
        }
        if matches!(relations, PathRelations::All) && !exhausted {
            for edge in &self.inheritance {
                if work == budget {
                    exhausted = true;
                    break;
                }
                work += 1;
                if !fresh(&edge.derived)
                    || (edge.candidates.len() != 1 && !q.include_ambiguous.unwrap_or(false))
                {
                    continue;
                }
                for target in &edge.candidates {
                    if work == budget {
                        exhausted = true;
                        break;
                    }
                    work += 1;
                    if fresh(target) {
                        candidates.push(Link {
                            source: &edge.derived,
                            target,
                            evidence: impact::Evidence::Inheritance(edge),
                            ambiguous: edge.candidates.len() != 1,
                        });
                    }
                }
                if exhausted {
                    break;
                }
            }
            for edge in &self.overrides {
                if work == budget {
                    exhausted = true;
                    break;
                }
                work += 1;
                if !fresh(&edge.derived)
                    || (edge.candidates.len() != 1 && !q.include_ambiguous.unwrap_or(false))
                {
                    continue;
                }
                for target in &edge.candidates {
                    if work == budget {
                        exhausted = true;
                        break;
                    }
                    work += 1;
                    if fresh(target) {
                        candidates.push(Link {
                            source: &edge.derived,
                            target,
                            evidence: impact::Evidence::Override(edge),
                            ambiguous: edge.candidates.len() != 1,
                        });
                    }
                }
                if exhausted {
                    break;
                }
            }
        }
        // Stable across HashMap extraction order and publication reloads.
        candidates.sort_by_key(|l| (l.source, l.target, l.evidence.location()));
        for link in candidates {
            if !matches!(direction, PathDirection::Back) {
                adjacency.entry(link.source).or_default().push(Step {
                    next: link.target,
                    link,
                    reversed: false,
                    ambiguous_target: link.ambiguous,
                });
            }
            if !matches!(direction, PathDirection::Forward) {
                adjacency.entry(link.target).or_default().push(Step {
                    next: link.source,
                    link,
                    reversed: true,
                    ambiguous_target: link.ambiguous,
                });
            }
        }
        let mut queue = VecDeque::from([Walk {
            nodes: vec![start.id.as_str()],
            contexts: vec![None],
            steps: vec![],
        }]);
        let mut paths = Vec::new();
        let mut depth_hit = false;
        let mut result_hit = false;
        while let Some(walk) = queue.pop_front() {
            let last = *walk.nodes.last().unwrap();
            if last == finish.id {
                let steps: Vec<_> = walk
                    .steps
                    .iter()
                    .enumerate()
                    .map(|(i,s)| {
                        let (source_context,target_context)=if s.reversed {(walk.contexts[i+1],walk.contexts[i])} else {(walk.contexts[i],walk.contexts[i+1])};
                        serde_json::json!({
                            "source":symbols[s.link.source],"target":symbols[s.link.target],
                            "relation":s.link.evidence.location().0,"reversed":s.reversed,
                            "ambiguous_target":s.ambiguous_target,"aggregate_binding_ambiguous":s.link.ambiguous,"source_context":source_context,"target_context":target_context,"context_conditioned":source_context.is_some() || target_context.is_some(),"evidence":s.link.evidence.value()
                        })
                    })
                    .collect();
                let owners: std::collections::BTreeSet<_> =
                    walk.contexts.iter().flatten().copied().collect();
                paths.push(serde_json::json!({"symbols":walk.nodes.iter().map(|id|symbols[id]).collect::<Vec<_>>(),"node_contexts":walk.contexts,"trait_context_symbols":owners.into_iter().map(|id|symbols[id]).collect::<Vec<_>>(),"steps":steps,"length":steps.len(),"captured_trait_frames_matched":true,"runtime_path_proven":false}));
                if paths.len() == limit {
                    result_hit = true;
                    break;
                }
                continue;
            }
            let next = adjacency.get(last).map(Vec::as_slice).unwrap_or(&[]);
            if walk.steps.len() == depth {
                for step in next.iter().filter(|s| !walk.nodes.contains(&s.next)) {
                    if !frames
                        .choices(step, *walk.contexts.last().unwrap(), &mut work, budget)
                        .is_empty()
                    {
                        depth_hit = true;
                        break;
                    }
                    if frames.exhausted {
                        exhausted = true;
                        break;
                    }
                }
                if exhausted {
                    break;
                }
                continue;
            }
            for step in next {
                if work == budget {
                    exhausted = true;
                    break;
                }
                work += 1;
                if walk.nodes.contains(&step.next) {
                    continue;
                }
                for choice in
                    frames.choices(step, *walk.contexts.last().unwrap(), &mut work, budget)
                {
                    let mut nodes = walk.nodes.clone();
                    nodes.push(step.next);
                    let mut contexts = walk.contexts.clone();
                    *contexts.last_mut().unwrap() = choice.current;
                    contexts.push(choice.next);
                    let mut steps = walk.steps.clone();
                    let mut selected = *step;
                    selected.ambiguous_target = choice.ambiguous;
                    steps.push(selected);
                    queue.push_back(Walk {
                        nodes,
                        contexts,
                        steps,
                    });
                }
                if frames.exhausted {
                    exhausted = true;
                    break;
                }
            }
            if exhausted {
                break;
            }
        }
        report["status"] = if !paths.is_empty() {
            "found"
        } else if exhausted {
            "work_limit"
        } else {
            "not_found_in_bounded_graph"
        }
        .into();
        report["trait_context_traversal"] = serde_json::json!({"matching":"captured_lexical_trait_frames","transitions_pruned":frames.pruned,"metadata_incomplete":frames.incomplete,"context_correlation_proven":false,"runtime_contexts_complete":false,"ambiguity_policy":"target_uniqueness_within_selected_consumer"});
        report["paths"] = paths.into();
        report["work"] = work.into();
        report["work_exhausted"] = exhausted.into();
        report["result_limit_reached"] = result_hit.into();
        report["depth_limit_reached"] = depth_hit.into();
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> CodeSnapshot {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("paths.duckdb")).unwrap();
        for (id, project, path, source) in [
            (
                "a",
                "one",
                "/one/src/lib.rs",
                "pub fn start(){left();right();} fn left(){leaf();} fn right(){leaf();} pub fn leaf(){} fn value(){let cb=leaf;} mod a{fn duplicate(){}} mod b{fn duplicate(){}}",
            ),
            ("b", "two", "/two/src/lib.rs", "pub fn foreign(){}"),
            (
                "c",
                "one",
                "/one/classes.py",
                "class Parent:\n    def method(self): pass\nclass Child(Parent):\n    def method(self): pass\n",
            ),
        ] {
            store
                .upsert_document(&crate::models::Document {
                    id: id.into(),
                    uri: format!("file://{path}"),
                    source_file: Some(path.into()),
                    wing: Some(project.into()),
                    content: source.into(),
                    revision: 1,
                    ..Default::default()
                })
                .unwrap();
        }
        store.index_code().unwrap();
        store.code_snapshot().unwrap()
    }
    fn query(from: &str, to: &str) -> PathQuery {
        serde_json::from_value(serde_json::json!({"from":from,"to":to})).unwrap()
    }
    #[test]
    fn paths_are_breadth_first_deterministic_and_preserve_each_occurrence() {
        let mut graph = fixture();
        let q = query("start", "leaf");
        let result = graph.trace_path(&q);
        assert_eq!(result["status"], "found", "{result}");
        assert_eq!(result["paths"].as_array().unwrap().len(), 2);
        for path in result["paths"].as_array().unwrap() {
            assert_eq!(path["length"], 2);
            assert_eq!(path["symbols"][0]["name"], "start");
            assert_eq!(path["symbols"][2]["name"], "leaf");
            for step in path["steps"].as_array().unwrap() {
                assert_eq!(step["relation"], "call");
                assert_eq!(step["reversed"], false);
                assert_eq!(step["ambiguous_target"], false);
                assert_eq!(step["evidence"]["reference"]["revision"], 1);
                assert!(
                    step["evidence"]["reference"]["content_hash"]
                        .as_str()
                        .unwrap()
                        .len()
                        > 10
                );
            }
        }
        graph.calls.reverse();
        graph.symbols.reverse();
        assert_eq!(graph.trace_path(&q), result);
        let zero = graph.trace_path(&query("start", "start"));
        assert_eq!(zero["paths"][0]["length"], 0);
    }
    #[test]
    fn endpoints_projects_and_stale_bridges_never_guess() {
        let mut graph = fixture();
        assert_eq!(
            graph.trace_path(&query("duplicate", "leaf"))["status"],
            "ambiguous_endpoints"
        );
        assert_eq!(
            graph.trace_path(&query("absent", "leaf"))["status"],
            "endpoint_unavailable"
        );
        assert_eq!(
            graph.trace_path(&query("start", "foreign"))["status"],
            "project_mismatch"
        );
        graph.stale_documents.push("a".into());
        let result = graph.trace_path(&query("start", "leaf"));
        assert_eq!(result["status"], "endpoint_unavailable");
        assert!(result["paths"].as_array().unwrap().is_empty());
    }
    #[test]
    fn budgets_depth_reverse_and_value_relations_are_explicit() {
        let graph = fixture();
        let mut q = query("start", "leaf");
        q.max_depth = Some(1);
        let shallow = graph.trace_path(&q);
        assert_eq!(shallow["status"], "not_found_in_bounded_graph");
        assert_eq!(shallow["depth_limit_reached"], true);
        q.max_depth = None;
        q.max_work = Some(1);
        let exhausted = graph.trace_path(&q);
        assert_eq!(exhausted["status"], "work_limit");
        assert_eq!(exhausted["work"], 1);
        q.max_work = None;
        q.max_paths = Some(1);
        assert_eq!(graph.trace_path(&q)["result_limit_reached"], true);
        let mut reverse = query("leaf", "start");
        reverse.direction = Some(PathDirection::Back);
        let result = graph.trace_path(&reverse);
        assert_eq!(result["status"], "found");
        assert_eq!(result["paths"][0]["steps"][0]["reversed"], true);
        let mut value = query("value", "leaf");
        assert_eq!(
            graph.trace_path(&value)["status"],
            "not_found_in_bounded_graph"
        );
        value.relations = Some(PathRelations::References);
        let result = graph.trace_path(&value);
        assert_eq!(result["status"], "found", "{result}");
        assert_eq!(
            result["paths"][0]["steps"][0]["relation"],
            "bound_reference"
        );
    }
    #[test]
    fn ambiguous_edges_require_opt_in_and_cannot_cross_project() {
        let mut graph = fixture();
        let leaf = graph
            .symbols
            .iter()
            .find(|s| s.name == "leaf")
            .unwrap()
            .id
            .clone();
        let foreign = graph
            .symbols
            .iter()
            .find(|s| s.name == "foreign")
            .unwrap()
            .id
            .clone();
        for call in &mut graph.calls {
            if call.candidates.contains(&leaf) {
                call.candidates.push(foreign.clone());
                call.resolution = "ambiguous".into();
            }
        }
        let mut q = query("start", "leaf");
        assert_eq!(graph.trace_path(&q)["status"], "not_found_in_bounded_graph");
        q.include_ambiguous = Some(true);
        let result = graph.trace_path(&q);
        assert_eq!(result["status"], "found");
        assert_eq!(result["paths"][0]["steps"][1]["ambiguous_target"], true);
        assert!(result["paths"].as_array().unwrap().iter().all(|p| {
            p["symbols"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["name"] != "foreign")
        }));
    }
    #[test]
    fn inheritance_and_override_paths_retain_candidate_semantics() {
        let graph = fixture();
        let mut q = query("Child", "Parent");
        assert_eq!(graph.trace_path(&q)["status"], "not_found_in_bounded_graph");
        q.relations = Some(PathRelations::All);
        let result = graph.trace_path(&q);
        assert_eq!(result["status"], "found", "{result}");
        assert!(
            result["paths"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["steps"][0]["relation"] == "inheritance_candidate"),
            "{result}"
        );
        let methods: Vec<_> = graph
            .symbols
            .iter()
            .filter(|s| s.name == "method")
            .collect();
        assert_eq!(methods.len(), 2);
        let child = methods
            .iter()
            .find(|s| s.qualified_name.contains("Child"))
            .unwrap();
        let parent = methods
            .iter()
            .find(|s| s.qualified_name.contains("Parent"))
            .unwrap();
        q.from = child.id.clone();
        q.to = parent.id.clone();
        let result = graph.trace_path(&q);
        assert_eq!(result["status"], "found", "{result}");
        assert!(
            result["paths"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["steps"][0]["relation"] == "override_candidate"),
            "{result}"
        );
        assert_eq!(result["runtime_path_proven"], false);
    }
    fn php_graph(files: &[(&str, &str, &str)]) -> CodeSnapshot {
        let mut docs = HashMap::new();
        let mut s = CodeSnapshot::default();
        for (i, (path, project, source)) in files.iter().enumerate() {
            let id = i.to_string();
            let mut d = parse(&id, path, source, 1).unwrap();
            assert!(!d.parse_errors, "{path}");
            d.project = Some((*project).into());
            s.document_scopes.insert(
                id.clone(),
                DocumentScope {
                    path: (*path).into(),
                    project: Some((*project).into()),
                },
            );
            s.symbols.extend(d.symbols.clone());
            s.references.extend(d.references.clone());
            docs.insert(id, d);
        }
        s.bound_references = resolve::references(&s.symbols, &docs);
        s.calls = s
            .bound_references
            .iter()
            .filter(|e| e.reference.is_call)
            .cloned()
            .collect();
        s
    }
    fn php_query(from: &str, to: &str) -> PathQuery {
        let mut q = query(from, to);
        q.project = Some("one".into());
        q
    }
    #[test]
    fn php_trace_contexts_match_forward_back_and_both_without_crossing_consumers() {
        let mut s = php_graph(&[(
            "/one/a.php",
            "one",
            "<?php trait T {public function run(){self::leaf();}} class A {use T;public static function leaf(){} public function entry(){$this->run();}} class B {use T;public static function leaf(){} public function entry(){$this->run();}}",
        )]);
        let q = php_query("A::entry", "A::leaf");
        let found = s.trace_path(&q);
        assert_eq!(found["status"], "found", "{found}");
        let path = &found["paths"][0];
        assert_eq!(path["length"], 2);
        assert_eq!(
            path["steps"][0]["target_context"],
            path["steps"][1]["source_context"]
        );
        assert_eq!(path["steps"][1]["aggregate_binding_ambiguous"], true);
        assert_eq!(path["steps"][1]["ambiguous_target"], false);
        for direction in [
            PathDirection::Forward,
            PathDirection::Back,
            PathDirection::Both,
        ] {
            let (from, to) = if matches!(direction, PathDirection::Back) {
                ("B::leaf", "A::entry")
            } else {
                ("A::entry", "B::leaf")
            };
            let mut q = php_query(from, to);
            q.direction = Some(direction);
            assert_eq!(s.trace_path(&q)["status"], "not_found_in_bounded_graph");
        }
        let mut reverse = php_query("A::leaf", "A::entry");
        reverse.direction = Some(PathDirection::Back);
        assert_eq!(s.trace_path(&reverse)["status"], "found");
        s.calls.reverse();
        s.symbols.reverse();
        assert_eq!(s.trace_path(&q), found);
    }
    #[test]
    fn php_trace_contexts_root_wildcard_keeps_distinct_conditional_paths_to_shared_target() {
        let s = php_graph(&[(
            "/one/a.php",
            "one",
            "<?php trait T {public function run(){self::leaf();}} class Root {public static function leaf(){}} class A extends Root {use T;} class B extends Root {use T;}",
        )]);
        let result = s.trace_path(&php_query("T::run", "Root::leaf"));
        assert_eq!(result["status"], "found");
        let paths = result["paths"].as_array().unwrap();
        assert_eq!(paths.len(), 2);
        assert_ne!(paths[0]["node_contexts"][0], paths[1]["node_contexts"][0]);
        assert_eq!(paths[0]["steps"][0]["ambiguous_target"], false);
        assert_eq!(paths[0]["steps"][0]["aggregate_binding_ambiguous"], true);
        for p in paths {
            assert_eq!(p["trait_context_symbols"].as_array().unwrap().len(), 1);
            assert_eq!(p["runtime_path_proven"], false);
        }
    }
    #[test]
    fn php_trace_nested_inherited_trait_frames_keep_different_source_and_target_owners() {
        let s = php_graph(&[(
            "/one/a.php",
            "one",
            "<?php trait Inner {public function inner(){self::leaf();}} trait Outer {public function outer(){$this->inner();}} class P1 {use Inner;public static function leaf(){}} class P2 {use Inner;public static function leaf(){}} class A extends P1 {use Outer;public function entry(){$this->outer();}} class B extends P2 {use Outer;public function entry(){$this->outer();}}",
        )]);
        let result = s.trace_path(&php_query("A::entry", "P1::leaf"));
        assert_eq!(result["status"], "found", "{result}");
        let steps = result["paths"][0]["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 3);
        assert_ne!(steps[1]["source_context"], steps[1]["target_context"]);
        assert_eq!(
            s.trace_path(&php_query("A::entry", "P2::leaf"))["status"],
            "not_found_in_bounded_graph"
        );
        let mut back = php_query("P1::leaf", "A::entry");
        back.direction = Some(PathDirection::Back);
        assert_eq!(s.trace_path(&back)["status"], "found");
    }
    #[test]
    fn php_trace_values_and_real_target_ambiguity_keep_existing_opt_in() {
        let mut s = php_graph(&[(
            "/one/a.php",
            "one",
            "<?php trait T {public function run(){self::leaf();}} class A {use T;public static function leaf(){} public static function other(){} public function entry(){$f=$this->run(...);}} class B {use T;public static function leaf(){} public function entry(){$this->run();}}",
        )]);
        let mut q = php_query("A::entry", "A::leaf");
        assert_eq!(s.trace_path(&q)["status"], "not_found_in_bounded_graph");
        q.relations = Some(PathRelations::References);
        assert_eq!(s.trace_path(&q)["status"], "found");
        let other = s
            .symbols
            .iter()
            .find(|s| s.qualified_name == "A::other")
            .unwrap()
            .id
            .clone();
        let owner = s
            .symbols
            .iter()
            .find(|s| s.qualified_name == "A")
            .unwrap()
            .id
            .clone();
        for edge in &mut s.bound_references {
            if edge.reference.expression == "self::leaf" {
                edge.candidates.push(other.clone());
                let row = edge
                    .context_evidence
                    .as_mut()
                    .unwrap()
                    .contexts
                    .iter_mut()
                    .find(|c| c.owner == owner)
                    .unwrap();
                row.candidates.push(other.clone());
                row.resolution = "ambiguous".into();
            }
        }
        assert_eq!(s.trace_path(&q)["status"], "not_found_in_bounded_graph");
        q.include_ambiguous = Some(true);
        let result = s.trace_path(&q);
        assert_eq!(result["status"], "found");
        assert_eq!(result["paths"][0]["steps"][1]["ambiguous_target"], true);
    }
    #[test]
    fn php_trace_stale_consumer_metadata_is_not_a_fresh_path_bridge() {
        let mut s = php_graph(&[
            (
                "/one/t.php",
                "one",
                "<?php trait T {public function run(){Helper::leaf();}}",
            ),
            (
                "/one/hosts.php",
                "one",
                "<?php class A {use T;} class B {use T;}",
            ),
            (
                "/one/helper.php",
                "one",
                "<?php class Helper {public static function leaf(){}}",
            ),
        ]);
        let q = php_query("T::run", "Helper::leaf");
        assert_eq!(s.trace_path(&q)["status"], "found");
        s.stale_documents.push("1".into());
        let result = s.trace_path(&q);
        assert_eq!(result["status"], "not_found_in_bounded_graph");
        assert_eq!(
            result["trait_context_traversal"]["metadata_incomplete"],
            true
        );
    }
    #[test]
    fn php_trace_missing_entry_or_body_context_never_uses_union_as_a_bridge() {
        let original = php_graph(&[(
            "/one/a.php",
            "one",
            "<?php trait T {public function run(){self::leaf();}} class A {use T;public static function leaf(){} public function entry(){$this->run();}} class B {use T;public static function leaf(){}}",
        )]);
        for kind in ["php_captured_trait_entry", "php_captured_trait_consumers"] {
            let mut s = php_graph(&[(
                "/one/a.php",
                "one",
                "<?php trait T {public function run(){self::leaf();}} class A {use T;public static function leaf(){} public function entry(){$this->run();}} class B {use T;public static function leaf(){}}",
            )]);
            assert_eq!(
                original.trace_path(&php_query("A::entry", "A::leaf"))["status"],
                "found"
            );
            for e in &mut s.calls {
                if e.context_evidence.as_ref().is_some_and(|m| m.kind == kind) {
                    e.context_evidence = None;
                }
            }
            for target in ["A::leaf", "B::leaf"] {
                let result = s.trace_path(&php_query("A::entry", target));
                assert_eq!(result["status"], "not_found_in_bounded_graph");
                assert_eq!(
                    result["trait_context_traversal"]["metadata_incomplete"],
                    true
                );
            }
        }
    }
    #[test]
    fn php_trace_enum_and_reverse_wildcard_root_retain_consumer_identity() {
        let s = php_graph(&[(
            "/one/a.php",
            "one",
            "<?php trait T {public static function run(){self::leaf();}} enum E {case One;use T;public static function leaf(){}} function start(){E::run();}",
        )]);
        let result = s.trace_path(&php_query("start", "E::leaf"));
        assert_eq!(result["status"], "found", "{result}");
        assert_eq!(
            result["paths"][0]["trait_context_symbols"][0]["kind"],
            "enum_declaration"
        );
        let mut q = php_query("T::run", "start");
        q.direction = Some(PathDirection::Back);
        let back = s.trace_path(&q);
        assert_eq!(back["status"], "found");
        assert!(back["paths"][0]["node_contexts"][0].as_str().is_some());
    }
    #[test]
    fn php_trace_context_work_inspection_is_shared_with_graph_and_result_budgets() {
        let s = php_graph(&[(
            "/one/a.php",
            "one",
            "<?php trait T {public function run(){self::leaf();}} class A {use T;public static function leaf(){} public function entry(){$this->run();}} class B {use T;public static function leaf(){} public function entry(){$this->run();}}",
        )]);
        for budget in 1..=40 {
            let mut q = php_query("A::entry", "A::leaf");
            q.max_work = Some(budget);
            let result = s.trace_path(&q);
            assert!(result["work"].as_u64().unwrap() <= budget as u64);
        }
        let mut q = php_query("A::entry", "B::leaf");
        q.max_depth = Some(1);
        let no = s.trace_path(&q);
        assert_eq!(no["depth_limit_reached"], true);
        q.max_depth = Some(2);
        let no = s.trace_path(&q);
        assert_eq!(no["depth_limit_reached"], false);
    }
    #[test]
    fn missing_trait_frame_does_not_discard_independent_nontrait_ambiguity_candidates() {
        let mut s = fixture();
        let d = parse(
            "trait",
            "/one/trait.php",
            "<?php trait T {public function leaf(){}}",
            1,
        )
        .unwrap();
        let trait_method = d
            .symbols
            .iter()
            .find(|s| s.qualified_name == "T::leaf")
            .unwrap()
            .id
            .clone();
        s.document_scopes.insert(
            "trait".into(),
            DocumentScope {
                path: "/one/trait.php".into(),
                project: Some("one".into()),
            },
        );
        s.symbols.extend(d.symbols);
        let leaf = s
            .symbols
            .iter()
            .find(|s| s.name == "leaf" && s.path.ends_with("lib.rs"))
            .unwrap()
            .id
            .clone();
        for edge in &mut s.calls {
            if edge.candidates.contains(&leaf) {
                edge.candidates.push(trait_method.clone());
                edge.resolution = "ambiguous".into();
            }
        }
        let mut q = query("start", &leaf);
        q.include_ambiguous = Some(true);
        let result = s.trace_path(&q);
        assert_eq!(result["status"], "found");
        assert_eq!(result["paths"].as_array().unwrap().len(), 2);
        assert_eq!(
            result["trait_context_traversal"]["metadata_incomplete"],
            true
        );
        assert!(result["paths"].as_array().unwrap().iter().all(|p| {
            p["symbols"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["qualified_name"] != "T::leaf")
        }));
    }
}
