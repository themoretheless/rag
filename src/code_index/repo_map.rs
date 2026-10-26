//! Bounded deterministic file-graph ranking and a fair, compact source map.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

/// Repository-map selection and explicit payload budgets.
#[derive(Debug, Default, serde::Deserialize, schemars::JsonSchema)]
pub struct RepoMapQuery {
    #[serde(flatten)]
    pub selection: CodeQuery,
    /// Exact project/wing. Omit to include all projects without merging their files.
    pub project: Option<String>,
    /// Component-aware captured path prefix; does not read live files.
    pub path_prefix: Option<String>,
    /// Optional map-text token estimate budget (1..8192). JSON metadata is outside it.
    pub budget_tokens: Option<usize>,
}
impl RepoMapQuery {
    fn includes(&self, snapshot: &CodeSnapshot, document_id: &str, path: &str) -> bool {
        self.project.as_deref().is_none_or(|project| {
            snapshot
                .document_scopes
                .get(document_id)
                .and_then(|scope| scope.project.as_deref())
                == Some(project)
        }) && self.path_prefix.as_ref().is_none_or(|prefix| {
            std::path::Path::new(path).starts_with(std::path::Path::new(prefix))
        })
    }
}
impl CodeSnapshot {
    /// Map the fresh, explicitly selected project/path graph and its coverage.
    pub fn repo_map(&self, q: &RepoMapQuery) -> serde_json::Value {
        let selected: Vec<_> = self
            .symbols
            .iter()
            .filter(|s| {
                q.includes(self, &s.document_id, &s.path)
                    && q.selection.path.as_ref().is_none_or(|path| path == &s.path)
                    && q.selection.symbol.as_ref().is_none_or(|name| {
                        name == &s.id || name == &s.name || name == &s.qualified_name
                    })
            })
            .collect();
        let mut result = query(
            self,
            &selected,
            &q.selection,
            q.selection.limit.unwrap_or(50).clamp(1, 500),
            q,
        );
        result["index_rev"] = serde_json::json!(self.index_revision);
        result["view"] = serde_json::json!("fresh_subset_of_revision_inputs");
        result["binding_diagnostics"] = serde_json::json!(self
            .binding_diagnostics
            .iter()
            .filter(|d| {
                if q.project.is_none() && q.path_prefix.is_none() {
                    return true;
                }
                d.get("document_id")
                    .and_then(|id| id.as_str())
                    .and_then(|id| self.document_scopes.get(id).map(|scope| (id, scope)))
                    .is_some_and(|(id, scope)| q.includes(self, id, &scope.path))
            })
            .collect::<Vec<_>>());
        result
    }
}
fn file_key<'a>(
    snapshot: &'a CodeSnapshot,
    document_id: &str,
    path: &'a str,
) -> (Option<&'a str>, &'a str) {
    (
        snapshot
            .document_scopes
            .get(document_id)
            .and_then(|s| s.project.as_deref()),
        path,
    )
}

pub(super) fn query(
    snapshot: &CodeSnapshot,
    selected: &[&Symbol],
    q: &CodeQuery,
    limit: usize,
    scope: &RepoMapQuery,
) -> serde_json::Value {
    let paths: BTreeSet<_> = snapshot
        .symbols
        .iter()
        .filter(|s| scope.includes(snapshot, &s.document_id, &s.path))
        .map(|s| file_key(snapshot, &s.document_id, &s.path))
        .chain(
            snapshot
                .bound_references
                .iter()
                .filter(|r| scope.includes(snapshot, &r.reference.document_id, &r.reference.path))
                .map(|r| file_key(snapshot, &r.reference.document_id, &r.reference.path)),
        )
        .collect();
    let paths: Vec<_> = paths.into_iter().collect();
    let positions: HashMap<_, _> = paths.iter().enumerate().map(|(i, p)| (*p, i)).collect();
    let symbols: HashMap<_, _> = snapshot
        .symbols
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();
    let mut adjacency = vec![BTreeSet::new(); paths.len()];
    let mut incoming: HashMap<&str, usize> = HashMap::new();
    for edge in snapshot
        .bound_references
        .iter()
        .filter(|e| e.resolution == "resolved_static")
    {
        for target in &edge.candidates {
            let Some(symbol) = symbols.get(target.as_str()) else {
                continue;
            };
            let from_key = file_key(snapshot, &edge.reference.document_id, &edge.reference.path);
            let to_key = file_key(snapshot, &symbol.document_id, &symbol.path);
            // A project boundary is never evidence of centrality in another project.
            if from_key.0 != to_key.0 {
                continue;
            }
            let (Some(&from), Some(&to)) = (positions.get(&from_key), positions.get(&to_key))
            else {
                continue;
            };
            *incoming.entry(symbol.id.as_str()).or_default() += 1;
            if from != to {
                adjacency[from].insert(to);
            }
        }
    }
    let n = paths.len();
    let mut scores = vec![if n == 0 { 0.0 } else { 1.0 / n as f64 }; n];
    for _ in 0..24 {
        if n == 0 {
            break;
        }
        let dangling: f64 = adjacency
            .iter()
            .enumerate()
            .filter(|(_, e)| e.is_empty())
            .map(|(i, _)| scores[i])
            .sum();
        let mut next = vec![(0.15 + 0.85 * dangling) / n as f64; n];
        for (i, edges) in adjacency.iter().enumerate() {
            if !edges.is_empty() {
                let share = 0.85 * scores[i] / edges.len() as f64;
                for j in edges {
                    next[*j] += share;
                }
            }
        }
        scores = next;
    }
    let entrypoints: HashSet<_> = snapshot
        .entrypoints
        .iter()
        .filter_map(|e| e.symbol_id.as_deref())
        .collect();
    let mut files: BTreeMap<_, Vec<&Symbol>> = BTreeMap::new();
    for symbol in selected {
        files
            .entry(file_key(snapshot, &symbol.document_id, &symbol.path))
            .or_default()
            .push(symbol);
    }
    let mut groups: Vec<_> = files.into_iter().collect();
    for (_, symbols) in &mut groups {
        symbols.sort_by(|a, b| {
            incoming
                .get(b.id.as_str())
                .unwrap_or(&0)
                .cmp(incoming.get(a.id.as_str()).unwrap_or(&0))
                .then_with(|| {
                    entrypoints
                        .contains(b.id.as_str())
                        .cmp(&entrypoints.contains(a.id.as_str()))
                })
                .then_with(|| {
                    (&a.qualified_name, a.start_line, &a.id).cmp(&(
                        &b.qualified_name,
                        b.start_line,
                        &b.id,
                    ))
                })
        });
    }
    groups.sort_by(|a, b| {
        let score = |path| positions.get(&path).map_or(0.0, |i| scores[*i]);
        score(b.0)
            .total_cmp(&score(a.0))
            .then_with(|| a.0.cmp(&b.0))
    });
    let budget = q.budget_chars.unwrap_or(4000).clamp(128, 20000);
    let token_budget = scope.budget_tokens.map(|n| n.clamp(1, 8192));
    let mut buffers = vec![String::new(); groups.len()];
    let mut used = 0;
    let mut emitted = 0;
    let mut skipped = 0;
    let rounds = groups.iter().map(|(_, s)| s.len()).max().unwrap_or(0);
    'rounds: for round in 0..rounds {
        for (i, ((project, path), symbols)) in groups.iter().enumerate() {
            if emitted == limit {
                break 'rounds;
            }
            let Some(symbol) = symbols.get(round) else {
                continue;
            };
            let header = if buffers[i].is_empty() {
                match project {
                    Some(project) => format!(
                        "[project {}] {}\n",
                        serde_json::to_string(project).unwrap(),
                        serde_json::to_string(path).unwrap()
                    ),
                    None => format!("{}\n", serde_json::to_string(path).unwrap()),
                }
            } else {
                String::new()
            };
            let name = symbol
                .qualified_name
                .replace('\n', "\\n")
                .replace('\r', "\\r");
            let entry = format!("  {} {} {}\n", symbol.start_line, symbol.kind, name);
            let size = header.chars().count() + entry.chars().count();
            if used + size > budget {
                skipped += 1;
                continue;
            }
            if let Some(token_budget) = token_budget {
                let candidate: String = buffers
                    .iter()
                    .enumerate()
                    .map(|(j, buffer)| {
                        if i == j {
                            format!("{buffer}{header}{entry}")
                        } else {
                            buffer.clone()
                        }
                    })
                    .collect();
                if crate::search_pack::estimate_tokens(&candidate) > token_budget {
                    skipped += 1;
                    continue;
                }
            }
            buffers[i].push_str(&header);
            buffers[i].push_str(&entry);
            used += size;
            emitted += 1;
        }
    }
    let emitted_files = buffers.iter().filter(|s| !s.is_empty()).count();
    let map = buffers.concat();
    let coverage = |ids: &[String]| {
        ids.iter()
            .filter(|id| {
                snapshot.document_scopes.get(*id).map_or(
                    scope.project.is_none() && scope.path_prefix.is_none(),
                    |s| scope.includes(snapshot, id, &s.path),
                )
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    let estimated_tokens = crate::search_pack::estimate_tokens(&map);
    serde_json::json!({"map":map,"project":scope.project,"path_prefix":scope.path_prefix,"budget_tokens":token_budget,"estimated_tokens":estimated_tokens,"token_budget_estimated":true,"token_budget_scope":"map_text_only_configured_tokenizer_or_script_heuristic","budget_chars":budget,"used_chars":used,"budget_scope":"map_text_unicode_scalars_only","metadata_in_budget":false,
        "total_symbols":selected.len(),"emitted_symbols":emitted,"omitted_symbols":selected.len()-emitted,"emitted_files":emitted_files,"total_files":groups.len(),
        "truncated":emitted<selected.len(),"budget_skipped_symbols":skipped,"ranking":"bounded_file_pagerank","ranking_iterations":24,"ranking_scope":"project_and_path_scoped_fresh_file_graph","ranking_nodes":n,"ranking_edges":adjacency.iter().map(BTreeSet::len).sum::<usize>(),"selection":"round_robin_ranked_files","runtime_importance_verified":false,
        "stale_documents":coverage(&snapshot.stale_documents),"unindexed_documents":coverage(&snapshot.unindexed_documents),"parse_errors":coverage(&snapshot.parse_errors)})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Document;
    fn put(store: &Store, id: &str, path: &str, text: &str) {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("file://{path}"),
                source_file: Some(path.into()),
                content: text.into(),
                ..Default::default()
            })
            .unwrap();
    }
    #[test]
    fn file_graph_ranks_central_api_and_rounds_do_not_starve_other_files() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("map.duckdb")).unwrap();
        put(
            &store,
            "a",
            "/p/api.py",
            "def work(): pass\ndef extra(): pass\n",
        );
        put(
            &store,
            "b",
            "/p/main.py",
            "from api import work\ndef main(): work()\n",
        );
        put(
            &store,
            "c",
            "/p/callback.py",
            "from api import work\nregistered = work\n",
        );
        store.index_code().unwrap();
        let snapshot = store.code_snapshot().unwrap();
        let q = CodeQuery {
            limit: Some(2),
            ..Default::default()
        };
        let map = snapshot.query("repo_map", &q);
        assert!(
            map["map"].as_str().unwrap().starts_with("\"/p/api.py\""),
            "{map}"
        );
        assert_eq!(map["emitted_files"], 2);
        assert_eq!(map["emitted_symbols"], 2);
        assert_eq!(map["ranking_edges"], 2);
        assert_eq!(map, snapshot.query("repo_map", &q));
        put(&store, "c", "/p/callback.py", "registered = None\n");
        let stale = store.code_snapshot().unwrap().query("repo_map", &q);
        assert_eq!(stale["ranking_edges"], 1);
        assert!(!stale["stale_documents"].as_array().unwrap().is_empty());
    }
    #[test]
    fn oversized_early_entries_do_not_hide_later_fitting_symbols() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("budget.duckdb")).unwrap();
        put(
            &store,
            "a",
            "/p/a.py",
            &format!("def {}(): pass\n", "x".repeat(200)),
        );
        put(&store, "b", "/p/б.py", "def коротко(): pass\n");
        store.index_code().unwrap();
        let map = store.code_snapshot().unwrap().query(
            "repo_map",
            &CodeQuery {
                budget_chars: Some(128),
                ..Default::default()
            },
        );
        let text = map["map"].as_str().unwrap();
        assert!(text.contains("коротко"), "{map}");
        assert_eq!(
            text.chars().count(),
            map["used_chars"].as_u64().unwrap() as usize
        );
        assert!(text.chars().count() <= 128);
        assert_eq!(map["budget_skipped_symbols"], 1);
        assert_eq!(map["omitted_symbols"], 1);
    }
    fn scoped_put(store: &Store, id: &str, project: &str, path: &str, text: &str) {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("doc://{id}"),
                wing: Some(project.into()),
                source_file: Some(path.into()),
                content: text.into(),
                ..Default::default()
            })
            .unwrap();
    }
    #[test]
    fn projects_with_identical_paths_keep_separate_graphs_and_coverage() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("scopes.duckdb")).unwrap();
        for (project, name) in [("a", "local"), ("b", "foreign")] {
            scoped_put(
                &store,
                &format!("{project}-api"),
                project,
                "/src/lib/api.py",
                &format!("def {name}(): pass\n"),
            );
            scoped_put(
                &store,
                &format!("{project}-client"),
                project,
                "/src/app/main.py",
                &format!("from lib.api import {name}\ndef run(): {name}()\n"),
            );
        }
        store.index_code().unwrap();
        let q = RepoMapQuery {
            project: Some("a".into()),
            ..Default::default()
        };
        let first = store.code_snapshot().unwrap().repo_map(&q);
        assert!(first["map"].as_str().unwrap().contains("local"));
        assert!(!first["map"].as_str().unwrap().contains("foreign"));
        assert_eq!(first["ranking_nodes"], 2);
        assert_eq!(first["ranking_edges"], 1);
        assert_eq!(first["total_files"], 2);
        let all = store
            .code_snapshot()
            .unwrap()
            .repo_map(&RepoMapQuery::default());
        assert_eq!(all["ranking_nodes"], 4);
        assert_eq!(all["total_files"], 4);
        let text = all["map"].as_str().unwrap();
        assert!(text.contains("[project \"a\"]") && text.contains("[project \"b\"]"));
        scoped_put(
            &store,
            "b-client",
            "b",
            "/src/app/main.py",
            "def changed(): pass\n",
        );
        scoped_put(
            &store,
            "b-new",
            "b",
            "/src/new.py",
            "def unindexed(): pass\n",
        );
        let scoped = store.code_snapshot().unwrap().repo_map(&q);
        assert_eq!(
            first, scoped,
            "another project's stale/unindexed sources must not change this map"
        );
        let all = store
            .code_snapshot()
            .unwrap()
            .repo_map(&RepoMapQuery::default());
        assert_eq!(all["stale_documents"], serde_json::json!(["b-client"]));
        assert_eq!(all["unindexed_documents"], serde_json::json!(["b-new"]));
    }
    #[test]
    fn directory_scope_uses_path_components_and_token_budget_can_return_empty() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("prefix.duckdb")).unwrap();
        scoped_put(&store, "one", "a", "/src/lib/a.py", "def привет(): pass\n");
        scoped_put(
            &store,
            "two",
            "a",
            "/src/lib-copy/b.py",
            "def excluded(): pass\n",
        );
        store.index_code().unwrap();
        let snapshot = store.code_snapshot().unwrap();
        let mut q = RepoMapQuery {
            project: Some("a".into()),
            path_prefix: Some("/src/lib".into()),
            ..Default::default()
        };
        let map = snapshot.repo_map(&q);
        assert_eq!(map["total_symbols"], 1);
        assert!(map["map"].as_str().unwrap().contains("привет"));
        assert!(!map["map"].as_str().unwrap().contains("excluded"));
        for budget in [1, 5, 16, 32, 64] {
            q.budget_tokens = Some(budget);
            let map = snapshot.repo_map(&q);
            let text = map["map"].as_str().unwrap();
            assert_eq!(
                map["estimated_tokens"],
                crate::search_pack::estimate_tokens(text)
            );
            assert!(map["estimated_tokens"].as_u64().unwrap() <= budget as u64);
            assert_eq!(map["used_chars"], text.chars().count());
            if budget == 1 {
                assert_eq!(text, "");
                assert_eq!(map["emitted_symbols"], 0);
            }
        }
        q.path_prefix = Some("/src/li".into());
        assert_eq!(snapshot.repo_map(&q)["total_symbols"], 0);
    }
    #[test]
    fn scoped_snapshot_does_not_decode_foreign_corrupt_payloads() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("corrupt-foreign.duckdb")).unwrap();
        scoped_put(&store, "a", "a", "/p/a.py", "def local(): pass\n");
        scoped_put(&store, "b", "b", "/p/b.py", "def foreign(): pass\n");
        store.index_code().unwrap();
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE code_index_documents SET payload='not JSON' WHERE document_id='b'",
                [],
            )
            .unwrap();
        assert!(store.code_snapshot().is_err());
        let q = RepoMapQuery {
            project: Some("a".into()),
            ..Default::default()
        };
        let map = store
            .code_snapshot_in_project(q.project.as_deref())
            .unwrap()
            .repo_map(&q);
        assert_eq!(map["total_symbols"], 1);
        assert!(map["map"].as_str().unwrap().contains("local"));
        let injection = "a' OR 1=1 --";
        assert!(store
            .code_snapshot_in_project(Some(injection))
            .unwrap()
            .symbols
            .is_empty());
    }
}
