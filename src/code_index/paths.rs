//! Bounded simple paths over captured symbol relationships, never runtime traces.
use super::*;
use std::collections::{BTreeMap, VecDeque};

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
    /// Include ambiguous target candidates (default false); each edge stays labelled.
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
}
struct Walk<'a> {
    nodes: Vec<&'a str>,
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
            "scope":"bounded_simple_symbol_paths; candidate relationships retain source semantics"
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
            if edge.candidates.len() != 1 && !q.include_ambiguous.unwrap_or(false) {
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
                        source,
                        target,
                        evidence: impact::Evidence::Reference(edge),
                        ambiguous: edge.candidates.len() != 1,
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
                });
            }
            if !matches!(direction, PathDirection::Forward) {
                adjacency.entry(link.target).or_default().push(Step {
                    next: link.source,
                    link,
                    reversed: true,
                });
            }
        }
        let mut queue = VecDeque::from([Walk {
            nodes: vec![start.id.as_str()],
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
                    .map(|s| {
                        serde_json::json!({
                            "source":symbols[s.link.source],"target":symbols[s.link.target],
                            "relation":s.link.evidence.location().0,"reversed":s.reversed,
                            "ambiguous_target":s.link.ambiguous,"evidence":s.link.evidence.value()
                        })
                    })
                    .collect();
                paths.push(serde_json::json!({"symbols":walk.nodes.iter().map(|id|symbols[id]).collect::<Vec<_>>(),"steps":steps,"length":steps.len()}));
                if paths.len() == limit {
                    result_hit = true;
                    break;
                }
                continue;
            }
            let next = adjacency.get(last).map(Vec::as_slice).unwrap_or(&[]);
            if walk.steps.len() == depth {
                depth_hit |= next.iter().any(|s| !walk.nodes.contains(&s.next));
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
                let mut nodes = walk.nodes.clone();
                nodes.push(step.next);
                let mut steps = walk.steps.clone();
                steps.push(*step);
                queue.push_back(Walk { nodes, steps });
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
            ("a", "one", "/one/src/lib.rs", "pub fn start(){left();right();} fn left(){leaf();} fn right(){leaf();} pub fn leaf(){} fn value(){let cb=leaf;} mod a{fn duplicate(){}} mod b{fn duplicate(){}}"),
            ("b", "two", "/two/src/lib.rs", "pub fn foreign(){}"),
            ("c", "one", "/one/classes.py", "class Parent:\n    def method(self): pass\nclass Child(Parent):\n    def method(self): pass\n"),
        ] {
            store.upsert_document(&crate::models::Document {id:id.into(),uri:format!("file://{path}"),source_file:Some(path.into()),wing:Some(project.into()),content:source.into(),revision:1,..Default::default()}).unwrap();
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
        assert!(result["paths"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["symbols"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s["name"] != "foreign")));
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
}
