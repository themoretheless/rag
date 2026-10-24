//! Bounded deterministic file-graph ranking and a fair, compact source map.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn query(
    snapshot: &CodeSnapshot,
    selected: &[&Symbol],
    q: &CodeQuery,
    limit: usize,
) -> serde_json::Value {
    let paths: BTreeSet<_> = snapshot
        .symbols
        .iter()
        .map(|s| s.path.as_str())
        .chain(
            snapshot
                .bound_references
                .iter()
                .map(|r| r.reference.path.as_str()),
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
            *incoming.entry(symbol.id.as_str()).or_default() += 1;
            if symbol.path != edge.reference.path {
                if let (Some(&from), Some(&to)) = (
                    positions.get(edge.reference.path.as_str()),
                    positions.get(symbol.path.as_str()),
                ) {
                    adjacency[from].insert(to);
                }
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
    let mut files: BTreeMap<&str, Vec<&Symbol>> = BTreeMap::new();
    for symbol in selected {
        files.entry(symbol.path.as_str()).or_default().push(symbol);
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
        let score = |path: &str| positions.get(path).map_or(0.0, |i| scores[*i]);
        score(b.0).total_cmp(&score(a.0)).then_with(|| a.0.cmp(b.0))
    });
    let budget = q.budget_chars.unwrap_or(4000).clamp(128, 20000);
    let mut buffers = vec![String::new(); groups.len()];
    let mut used = 0;
    let mut emitted = 0;
    let mut skipped = 0;
    let rounds = groups.iter().map(|(_, s)| s.len()).max().unwrap_or(0);
    'rounds: for round in 0..rounds {
        for (i, (path, symbols)) in groups.iter().enumerate() {
            if emitted == limit {
                break 'rounds;
            }
            let Some(symbol) = symbols.get(round) else {
                continue;
            };
            let header = if buffers[i].is_empty() {
                format!("{}\n", serde_json::to_string(path).unwrap())
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
            buffers[i].push_str(&header);
            buffers[i].push_str(&entry);
            used += size;
            emitted += 1;
        }
    }
    let emitted_files = buffers.iter().filter(|s| !s.is_empty()).count();
    let map = buffers.concat();
    serde_json::json!({"map":map,"budget_chars":budget,"used_chars":used,"budget_scope":"map_text_unicode_scalars_only","metadata_in_budget":false,
        "total_symbols":selected.len(),"emitted_symbols":emitted,"omitted_symbols":selected.len()-emitted,"emitted_files":emitted_files,"total_files":groups.len(),
        "truncated":emitted<selected.len(),"budget_skipped_symbols":skipped,"ranking":"bounded_file_pagerank","ranking_iterations":24,"ranking_scope":"fresh_symbol_and_reference_file_graph","ranking_nodes":n,"ranking_edges":adjacency.iter().map(BTreeSet::len).sum::<usize>(),"selection":"round_robin_ranked_files","runtime_importance_verified":false,
        "stale_documents":snapshot.stale_documents,"unindexed_documents":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors})
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
}
