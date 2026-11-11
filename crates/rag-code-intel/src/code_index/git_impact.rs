//! Git-state source snapshots, structural change mapping and two-sided impact.
use super::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
mod capture;

#[derive(Debug, Default, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitScope {
    /// Index -> working tree, including untracked supported source files.
    #[default]
    WorkingTree,
    /// HEAD -> index. Unstaged working files do not contribute.
    Staged,
    /// HEAD -> working tree, including staged and untracked source changes.
    All,
    /// Explicit before and after commit/tree-ish references, pinned to commit OIDs.
    Commits,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitImpactQuery {
    /// Exact allowlisted Git working-tree root; no checkout or source mutation.
    pub repository: String,
    pub scope: Option<GitScope>,
    /// Required with scope=commits; otherwise forbidden.
    pub before: Option<String>,
    pub after: Option<String>,
    /// Project identity for both transient graphs; does not select stored documents.
    pub project: Option<String>,
    /// Shared source captures across both sides, 1..10000 (default 2000).
    pub max_files: Option<usize>,
    /// Shared captured source bytes, 1 KiB..128 MiB (default 8 MiB); 4 MiB/file.
    pub max_bytes: Option<usize>,
    /// Changed file/symbol groups returned, 1..500 (default 100).
    pub limit: Option<usize>,
    /// Incoming graph depth, 1..10 (default 3).
    pub depth: Option<usize>,
    /// Affected symbols returned per side, 1..500 (default 100).
    pub max_affected: Option<usize>,
    /// Changed-symbol seeds per side, 1..1000 (default 500); truncation is explicit.
    pub max_seeds: Option<usize>,
    /// calls (default), references, or all (including inheritance candidates).
    pub relations: Option<PathRelations>,
    /// Attach bounded test recommendations from a cfg(test) graph overlay.
    pub include_tests: Option<bool>,
    pub max_tests: Option<usize>,
    pub test_max_work: Option<usize>,
}
struct Graph {
    snapshot: CodeSnapshot,
    bodies: HashMap<String, String>,
    contexts: HashMap<String, Option<String>>,
    unavailable_paths: HashSet<String>,
    keys: HashMap<String, String>,
    groups: BTreeMap<String, Vec<String>>,
}
fn graph(capture: &capture::Capture, root: &Path, project: &str, test_view: bool) -> Result<Graph> {
    let mut docs = HashMap::new();
    let mut snapshot = CodeSnapshot::default();
    let mut bodies = HashMap::new();
    let mut contexts = HashMap::new();
    let mut unavailable_paths: HashSet<_> = capture
        .unavailable
        .iter()
        .filter_map(|v| v["path"].as_str().map(str::to_string))
        .collect();
    for (relative, source) in &capture.sources {
        let path = root.join(relative).to_string_lossy().into_owned();
        let id = format!(
            "git:{}",
            content_hash(&serde_json::to_string(&(project, &path))?)
        );
        let mut doc = parse(&id, &path, &source.text, 0)?;
        doc.project = Some(project.into());
        snapshot.document_scopes.insert(
            id.clone(),
            DocumentScope {
                path,
                project: Some(project.into()),
            },
        );
        if doc.parse_errors {
            snapshot.parse_errors.push(id.clone());
            unavailable_paths.insert(relative.clone());
        }
        docs.insert(id, doc);
    }
    // Preserve unverified manifest boundaries instead of falling back through a
    // missing symlinked Cargo/go manifest to conventional-layout lookup.
    for unavailable in &capture.unavailable {
        let Some(relative) = unavailable["path"].as_str() else {
            continue;
        };
        if rust_cargo::manifest(relative) || go::manifest(relative) {
            let path = root.join(relative).to_string_lossy().into_owned();
            let id = format!(
                "git:{}",
                content_hash(&serde_json::to_string(&(project, &path))?)
            );
            let mut doc = parse(&id, &path, "", 0)?;
            doc.project = Some(project.into());
            snapshot.unindexed_documents.push(id.clone());
            docs.insert(id, doc);
        }
    }
    if test_view {
        rust_cfg::prepare_test_view(&mut docs);
    }
    let configured = rust_cfg::apply(&mut docs);
    snapshot.rust_build_contexts = configured.contexts;
    snapshot.binding_diagnostics = configured.diagnostics;
    for (id, doc) in &docs {
        for limitation in &doc.binding_limitations {
            snapshot
                .binding_diagnostics
                .push(json!({"document_id":id,"path":doc.path,"code":limitation}));
        }
        if doc.parse_errors {
            continue;
        }
        snapshot.symbols.extend(doc.symbols.clone());
        snapshot.references.extend(doc.references.clone());
    }
    snapshot.symbols.sort_by(|a, b| a.id.cmp(&b.id));
    // Broken source files may still have partial ASTs. Do not resolve through them.
    for (id, doc) in docs.iter_mut().filter(|(_, d)| d.parse_errors) {
        let mut blocked = parse(id, &doc.path, "", 0)?;
        blocked.project = doc.project.clone();
        blocked
            .binding_limitations
            .push("GIT_SOURCE_PARSE_ERROR".into());
        *doc = blocked;
    }
    snapshot.bound_references = resolve::references(&snapshot.symbols, &docs);
    snapshot.calls = snapshot
        .bound_references
        .iter()
        .filter(|e| e.reference.is_call)
        .cloned()
        .collect();
    snapshot.inheritance = inheritance::edges(&snapshot.symbols, &docs);
    snapshot.overrides = inheritance::overrides(&snapshot.symbols, &docs);
    snapshot.entrypoints = analysis::entrypoints(&snapshot.symbols, &docs);
    if test_view {
        snapshot.test_commands = rust_cargo::test_commands(&snapshot.symbols, &docs);
        snapshot.cargo_targets = rust_cargo::target_evidence(&docs);
    }
    let mut keys = HashMap::new();
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for symbol in &snapshot.symbols {
        let relative = Path::new(&symbol.path)
            .strip_prefix(root)
            .map_err(|_| AppError::config("Git symbol escapes repository"))?
            .to_string_lossy();
        let key =
            serde_json::to_string(&(relative.as_ref(), &symbol.qualified_name, &symbol.kind))?;
        let source = &capture.sources[relative.as_ref()].text;
        let body = source
            .get(symbol.start_byte..symbol.end_byte)
            .ok_or_else(|| AppError::config("invalid Git symbol span"))?;
        bodies.insert(symbol.id.clone(), content_hash(body));
        contexts.insert(symbol.id.clone(), symbol.build_context.clone());
        keys.insert(symbol.id.clone(), key.clone());
        groups.entry(key).or_default().push(symbol.id.clone());
    }
    for ids in groups.values_mut() {
        ids.sort();
    }
    Ok(Graph {
        snapshot,
        bodies,
        contexts,
        unavailable_paths,
        keys,
        groups,
    })
}
fn bindings(graph: &Graph) -> BTreeMap<&str, Vec<String>> {
    let symbols: HashMap<_, _> = graph
        .snapshot
        .symbols
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();
    let mut out: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for edge in &graph.snapshot.bound_references {
        let Some(caller) = edge
            .reference
            .caller
            .as_deref()
            .filter(|id| symbols.contains_key(id))
        else {
            continue;
        };
        let mut targets: Vec<_> = edge
            .candidates
            .iter()
            .filter_map(|id| graph.keys.get(id))
            .collect();
        targets.sort();
        let value = serde_json::to_string(&(
            edge.reference
                .start_byte
                .saturating_sub(symbols[caller].start_byte),
            &edge.reference.expression,
            edge.reference.is_call,
            &edge.resolution,
            &edge.resolution_basis,
            targets,
        ))
        .unwrap();
        out.entry(caller).or_default().push(value);
    }
    for values in out.values_mut() {
        values.sort();
    }
    out
}
fn identity(capture: &capture::Capture) -> Value {
    let evidence: Vec<_> = capture
        .sources
        .iter()
        .map(|(path, source)| (path, content_hash(&source.text), &source.evidence))
        .collect();
    json!({"git":capture.identity,"capture_id":content_hash(&serde_json::to_string(&(&capture.identity,&capture.entries,evidence)).unwrap()),"source_files":capture.sources.len(),"tracked_entries":capture.entries.len(),
        "unavailable_inputs":capture.unavailable.iter().take(50).collect::<Vec<_>>(),"unavailable_inputs_total":capture.unavailable.len(),"unsupported_paths_sample":capture.unsupported.iter().take(20).collect::<Vec<_>>(),"unsupported_paths_total":capture.unsupported.len()})
}
fn graph_evidence(graph: &Graph) -> Value {
    json!({"symbols":graph.snapshot.symbols.len(),"calls":graph.snapshot.calls.len(),"bound_references":graph.snapshot.bound_references.len(),
        "parse_error_documents":graph.snapshot.parse_errors.iter().take(50).collect::<Vec<_>>(),"parse_error_documents_total":graph.snapshot.parse_errors.len(),
        "binding_diagnostics":graph.snapshot.binding_diagnostics.iter().take(50).collect::<Vec<_>>(),"binding_diagnostics_total":graph.snapshot.binding_diagnostics.len(),
        "rust_build_contexts":graph.snapshot.rust_build_contexts.iter().take(50).collect::<Vec<_>>(),"rust_build_contexts_total":graph.snapshot.rust_build_contexts.len(),
        "source_mapping_complete_within_supported_inputs":graph.unavailable_paths.is_empty()&&graph.snapshot.binding_diagnostics.is_empty(),"runtime_semantics_complete":false})
}
fn impacts(graph: &Graph, seeds: &[&Symbol], q: &GitImpactQuery) -> Value {
    let relations = match q.relations.unwrap_or_default() {
        PathRelations::Calls => "calls",
        PathRelations::References => "references",
        PathRelations::All => "all",
    };
    let mut result = impact::query(
        &graph.snapshot,
        &CodeQuery {
            depth: q.depth,
            impact_relations: Some(relations.into()),
            ..Default::default()
        },
        seeds,
        q.max_affected.unwrap_or(100).clamp(1, 500),
    );
    let by_id: HashMap<_, _> = graph
        .snapshot
        .symbols
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();
    let items = result["items"].as_array().cloned().unwrap_or_default();
    let by_source: HashMap<_, _> = items
        .iter()
        .filter_map(|v| v["symbol_id"].as_str().map(|id| (id, v)))
        .collect();
    let seed_ids: HashSet<_> = seeds.iter().map(|s| s.id.as_str()).collect();
    let witnessed:Vec<_>=items.iter().map(|item| {
        let mut current=item["symbol_id"].as_str().unwrap();let mut steps=Vec::new();let mut nodes=vec![by_id[current]];
        while !seed_ids.contains(current) {
            let Some(edge)=by_source.get(current) else {break;};
            let target=edge["via_target"].as_str().unwrap();
            steps.push(json!({"source_symbol_id":current,"target_symbol_id":target,"relation":edge["via_kind"],"evidence":edge["via"]}));
            nodes.push(by_id[target]);current=target;
            if steps.len()>10 {break;}
        }
        json!({"symbol":by_id[item["symbol_id"].as_str().unwrap()],"depth":item["depth"],"seed_symbol_id":current,"seed_change_key":graph.keys.get(current),"path":{"symbols":nodes,"steps":steps,"complete_to_seed":seed_ids.contains(current)}})
    }).collect();
    result["items"] = witnessed.into();
    result["direct_changed_symbols"] = seeds
        .iter()
        .map(|s| json!({"symbol":s,"change_key":graph.keys.get(&s.id)}))
        .collect::<Vec<_>>()
        .into();
    if q.include_tests.unwrap_or(false) {
        result["affected_tests"] = affected_tests::selection(
            &graph.snapshot,
            seeds,
            &AffectedTestsQuery {
                symbols: vec![],
                path: None,
                project: None,
                depth: q.depth,
                limit: q.max_tests,
                max_work: q.test_max_work,
                relations: q.relations,
            },
        );
    }
    result
}

/// Capture Git states and build native ephemeral graphs; no database write or checkout.
pub fn git_diff_impact(q: &GitImpactQuery, roots: &[PathBuf]) -> Result<Value> {
    let scope = q.scope.unwrap_or_default();
    if matches!(scope, GitScope::Commits) != (q.before.is_some() && q.after.is_some())
        || (!matches!(scope, GitScope::Commits) && (q.before.is_some() || q.after.is_some()))
    {
        return Err(AppError::config(
            "before and after are both required only with scope=commits",
        ));
    }
    let mut git = capture::Git::open(q, roots)?;
    let mut index_guard = None;
    let mut untracked_guard = None;
    let (before, after) = match scope {
        GitScope::Commits => {
            let a = git.revision(q.before.as_deref().unwrap())?;
            let b = git.revision(q.after.as_deref().unwrap())?;
            (git.tree(&a)?, git.tree(&b)?)
        }
        GitScope::Staged => {
            let head = git.revision("HEAD")?;
            let (raw, entries) = git.index()?;
            index_guard = Some(raw.clone());
            (git.tree(&head)?,git.objects(entries,json!({"kind":"index","entry_fingerprint":blake3::hash(&raw).to_hex().to_string()}))?)
        }
        GitScope::WorkingTree | GitScope::All => {
            let head = if matches!(scope, GitScope::All) {
                Some(git.revision("HEAD")?)
            } else {
                None
            };
            let (raw, entries) = git.index()?;
            index_guard = Some(raw.clone());
            let untracked = git.untracked()?;
            untracked_guard = Some(untracked.clone());
            let before = if let Some(head) = head {
                git.tree(&head)?
            } else {
                git.objects(entries.clone(),json!({"kind":"index","entry_fingerprint":blake3::hash(&raw).to_hex().to_string()}))?
            };
            (before, git.working(entries, &untracked, roots)?)
        }
    };
    let project = q
        .project
        .clone()
        .unwrap_or_else(|| format!("git:{}", content_hash(&git.root.to_string_lossy())));
    let old = graph(
        &before,
        &git.root,
        &project,
        q.include_tests.unwrap_or(false),
    )?;
    let new = graph(
        &after,
        &git.root,
        &project,
        q.include_tests.unwrap_or(false),
    )?;
    let old_symbols: HashMap<_, _> = old
        .snapshot
        .symbols
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();
    let new_symbols: HashMap<_, _> = new
        .snapshot
        .symbols
        .iter()
        .map(|s| (s.id.as_str(), s))
        .collect();
    let old_bindings = bindings(&old);
    let new_bindings = bindings(&new);
    let keys: BTreeSet<_> = old.groups.keys().chain(new.groups.keys()).collect();
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let mut changes = Vec::new();
    let mut total = 0;
    let mut before_seeds = Vec::new();
    let mut after_seeds = Vec::new();
    for key in keys {
        let a = old.groups.get(key).map(Vec::as_slice).unwrap_or(&[]);
        let b = new.groups.get(key).map(Vec::as_slice).unwrap_or(&[]);
        let values = |ids: &[String], g: &Graph| {
            let mut values: Vec<_> = ids
                .iter()
                .map(|id| (g.bodies[id].clone(), g.contexts[id].clone()))
                .collect();
            values.sort();
            values
        };
        let changed = values(a, &old) != values(b, &new)
            || (a.len() == 1
                && b.len() == 1
                && old_bindings.get(a[0].as_str()) != new_bindings.get(b[0].as_str()));
        if !changed {
            continue;
        }
        total += 1;
        if changes.len() == limit {
            continue;
        }
        let relative: (String, String, String) = serde_json::from_str(key)?;
        let kind = if a.is_empty() && old.unavailable_paths.contains(&relative.0) {
            "unavailable_before"
        } else if b.is_empty() && new.unavailable_paths.contains(&relative.0) {
            "unavailable_after"
        } else if a.is_empty() {
            "added"
        } else if b.is_empty() {
            "removed"
        } else if a.len() != 1 || b.len() != 1 {
            "ambiguous_changed"
        } else if old.bodies[&a[0]] != new.bodies[&b[0]] {
            "modified"
        } else if old_symbols[a[0].as_str()].build_context
            != new_symbols[b[0].as_str()].build_context
        {
            "configuration_changed"
        } else {
            "binding_changed"
        };
        let aa: Vec<_> = a.iter().map(|id| old_symbols[id.as_str()]).collect();
        let bb: Vec<_> = b.iter().map(|id| new_symbols[id.as_str()]).collect();
        before_seeds.extend(aa.iter().copied());
        after_seeds.extend(bb.iter().copied());
        changes.push(json!({"change_key":key,"kind":kind,"mapping":"same_relative_path_qualified_name_kind; duplicate_groups_never_paired","before":aa.iter().take(20).collect::<Vec<_>>(),"before_total":aa.len(),"after":bb.iter().take(20).collect::<Vec<_>>(),"after_total":bb.len(),"candidates_truncated":aa.len()>20||bb.len()>20}));
    }
    let paths: BTreeSet<_> = before
        .entries
        .keys()
        .chain(after.entries.keys())
        .chain(before.sources.keys())
        .chain(after.sources.keys())
        .collect();
    let mut files = Vec::new();
    let mut files_total = 0;
    for path in paths {
        let a = before.sources.get(path);
        let b = after.sources.get(path);
        let raw_hash = |s: &capture::Source| {
            s.evidence["bytes_hash"]
                .as_str()
                .or_else(|| s.evidence["live"]["bytes_hash"].as_str())
                .unwrap_or("")
                .to_string()
        };
        let source_changed = a.map(|s| (raw_hash(s), &s.evidence["mode"]))
            != b.map(|s| (raw_hash(s), &s.evidence["mode"]));
        let object_changed =
            a.is_none() && b.is_none() && before.entries.get(path) != after.entries.get(path);
        if !source_changed && !object_changed {
            continue;
        }
        files_total += 1;
        if files.len() == limit {
            continue;
        }
        files.push(json!({"path":path,"kind":if !before.entries.contains_key(path){"added"}else if !after.entries.contains_key(path){"removed"}else{"modified"},"symbol_extractor_supported":indexable(path),
            "before":a.map(|s|json!({"source_hash":content_hash(&s.text),"evidence":s.evidence})),"after":b.map(|s|json!({"source_hash":content_hash(&s.text),"evidence":s.evidence})),"before_entry":before.entries.get(path),"after_entry":after.entries.get(path)}));
    }
    let before_seed_total = before_seeds.len();
    let after_seed_total = after_seeds.len();
    let seed_limit = q.max_seeds.unwrap_or(500).clamp(1, 1000);
    before_seeds.truncate(seed_limit);
    after_seeds.truncate(seed_limit);
    let mut before_impact = impacts(&old, &before_seeds, q);
    let mut after_impact = impacts(&new, &after_seeds, q);
    before_impact["seed_symbols_total_for_returned_groups"] = before_seed_total.into();
    before_impact["seeds_truncated"] = (before_seed_total > seed_limit).into();
    after_impact["seed_symbols_total_for_returned_groups"] = after_seed_total.into();
    after_impact["seeds_truncated"] = (after_seed_total > seed_limit).into();
    if let Some(raw) = index_guard {
        if git.index()?.0 != raw {
            return Err(AppError::Busy(
                "Git index changed during analysis; retry".into(),
            ));
        }
    }
    if let Some(raw) = untracked_guard {
        if git.untracked()? != raw {
            return Err(AppError::Busy(
                "untracked source set changed during analysis; retry".into(),
            ));
        }
        git.validate_working(&after, roots)?;
    }
    Ok(
        json!({"status":"captured","scope":match scope{GitScope::WorkingTree=>"working_tree",GitScope::Staged=>"staged",GitScope::All=>"all",GitScope::Commits=>"commits"},"repository":git.root,"project":project,
        "graph_origin":"ephemeral_git_source_snapshots","persistent_index_used":false,"index_format":INDEX_FORMAT_VERSION,"before":identity(&before),"after":identity(&after),
        "before_graph":graph_evidence(&old),"after_graph":graph_evidence(&new),
        "files":files,"files_changed_total":files_total,"files_truncated":files_total>limit,"changes":changes,"changed_symbol_groups_total":total,"changes_truncated":total>limit,
        "before_impact":before_impact,"after_impact":after_impact,"impact_seed_scope":"bounded_candidates_of_returned_changed_symbol_groups",
        "test_cfg_overlay":q.include_tests.unwrap_or(false),"test_command_source_requirement":"run only against the matching before/after captured source state",
        "capture":{"source_bytes":git.bytes,"git_commands":git.commands,"source_bytes_revalidation_additional":matches!(scope,GitScope::All|GitScope::WorkingTree)},
        "working_unsupported_files_checked":false,"rename_detection":false,"complete_cargo_semantics":false,"runtime_impact_proven":false,"absence_proven":false,"program_impact_complete":false,
        "limitations":["supported source and manifest formats only; working unsupported files are not read","no rename pairing across paths","source identity is captured Git/blob or revalidated filesystem evidence, not a stored-index revision","static resolver and depth/return limits apply; no complete runtime impact or absence proof"]}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    struct Repo {
        temp: tempfile::TempDir,
        root: PathBuf,
    }
    impl Repo {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let repo = Self { temp, root };
            repo.git(&["init", "-q"]);
            repo.write(
                "Cargo.toml",
                "[package]\nname='git-fixture'\nversion='0.1.0'\nedition='2021'\n",
            );
            repo.write(
                "src/lib.rs",
                "pub mod worker; pub fn api()->i32{worker::value()} pub fn entry()->i32{api()}\n",
            );
            repo.write(
                "src/worker.rs",
                "pub fn value()->i32{1}\npub fn removed(){}\n",
            );
            repo.git(&["add", "."]);
            repo.git(&[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-qm",
                "base",
            ]);
            repo
        }
        fn write(&self, path: &str, text: &str) {
            let path = self.root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        fn git(&self, args: &[&str]) -> String {
            let output = Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().into()
        }
        fn query(&self, scope: &str) -> GitImpactQuery {
            serde_json::from_value(
                json!({"repository":self.root,"scope":scope,"project":"fixture"}),
            )
            .unwrap()
        }
        fn run(&self, q: &GitImpactQuery) -> Value {
            git_diff_impact(q, std::slice::from_ref(&self.root)).unwrap()
        }
    }
    fn change<'a>(report: &'a Value, name: &str) -> &'a Value {
        report["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| {
                c["before"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .chain(c["after"].as_array().unwrap())
                    .any(|s| s["name"] == name)
            })
            .unwrap()
    }
    #[test]
    fn working_staged_and_commit_sources_are_distinct_and_pinned() {
        let repo = Repo::new();
        let before = repo.git(&["rev-parse", "HEAD"]);
        let staged = "pub fn value()->i32{2}\npub fn removed(){}\n";
        let working = "pub fn value()->i32{3}\npub fn removed(){}\n";
        repo.write("src/worker.rs", staged);
        repo.git(&["add", "src/worker.rs"]);
        repo.write("src/worker.rs", working);
        let stage = repo.run(&repo.query("staged"));
        let work = repo.run(&repo.query("working_tree"));
        let file = |v: &Value| {
            v["files"]
                .as_array()
                .unwrap()
                .iter()
                .find(|f| f["path"] == "src/worker.rs")
                .unwrap()
                .clone()
        };
        assert_eq!(file(&stage)["after"]["source_hash"], content_hash(staged));
        assert_eq!(file(&work)["before"]["source_hash"], content_hash(staged));
        assert_eq!(file(&work)["after"]["source_hash"], content_hash(working));
        assert_eq!(change(&stage, "value")["kind"], "modified");
        assert_eq!(
            stage["before_impact"]["items"].as_array().unwrap().len(),
            2,
            "{stage}"
        );
        for side in ["before_impact", "after_impact"] {
            let entry = stage[side]["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["symbol"]["name"] == "entry")
                .unwrap();
            assert_eq!(entry["path"]["steps"].as_array().unwrap().len(), 2);
            assert_eq!(entry["path"]["complete_to_seed"], true);
            assert_eq!(entry["path"]["symbols"][2]["name"], "value");
        }
        repo.git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "staged",
        ]);
        let after = repo.git(&["rev-parse", "HEAD"]);
        let mut q = repo.query("commits");
        q.before = Some(before.clone());
        q.after = Some(after.clone());
        let frozen = repo.run(&q);
        assert_eq!(frozen["before"]["git"]["commit"], before);
        assert_eq!(frozen["after"]["git"]["commit"], after);
        repo.write("src/worker.rs", "pub fn entirely_different(){}\n");
        assert_eq!(repo.run(&q), frozen);
        // A staged change reverted only in the worktree is absent from HEAD->worktree.
        repo.write("src/worker.rs", working);
        repo.git(&["add", "src/worker.rs"]);
        repo.write("src/worker.rs", staged);
        let no_changes = repo.run(&repo.query("all"));
        assert_eq!(no_changes["files_changed_total"], 0);
        assert_eq!(no_changes["changed_symbol_groups_total"], 0);
        assert!(repo.temp.path().exists());
    }
    #[test]
    fn added_removed_untracked_and_ambiguous_definitions_are_explicit() {
        let repo = Repo::new();
        repo.write(
            "src/worker.rs",
            "pub fn value()->i32{1}\npub fn added(){}\n",
        );
        repo.write("src/with space\nand newline.rs", "pub fn untracked(){}\n");
        let result = repo.run(&repo.query("all"));
        assert_eq!(change(&result, "removed")["kind"], "removed");
        assert_eq!(change(&result, "added")["kind"], "added");
        assert_eq!(change(&result, "untracked")["kind"], "added");
        assert!(result["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == "src/with space\nand newline.rs"));
        repo.write(
            "src/worker.rs",
            "#[cfg(a)]pub fn same(){one();}\n#[cfg(b)]pub fn same(){two();}\n",
        );
        repo.git(&["add", "src/worker.rs"]);
        repo.write(
            "src/worker.rs",
            "#[cfg(a)]pub fn same(){three();}\n#[cfg(b)]pub fn same(){two();}\n",
        );
        let result = repo.run(&repo.query("working_tree"));
        let same = change(&result, "same");
        assert_eq!(same["kind"], "ambiguous_changed");
        assert_eq!(same["before_total"], 2);
        assert_eq!(same["after_total"], 2);
        let mut q = repo.query("working_tree");
        q.max_seeds = Some(1);
        let limited = repo.run(&q);
        assert_eq!(limited["before_impact"]["seeds_truncated"], true);
        assert_eq!(limited["after_impact"]["seeds_truncated"], true);
    }
    #[test]
    fn capture_and_output_limits_are_never_silent_partial_graphs() {
        let repo = Repo::new();
        let mut q = repo.query("all");
        q.max_files = Some(1);
        assert!(git_diff_impact(&q, std::slice::from_ref(&repo.root))
            .unwrap_err()
            .to_string()
            .contains("budget"));
        q.max_files = None;
        repo.write(
            "src/worker.rs",
            &format!("pub fn changed(){{let _s=\"{}\";}}", "x".repeat(2000)),
        );
        q.max_bytes = Some(1024);
        assert!(git_diff_impact(&q, std::slice::from_ref(&repo.root)).is_err());
        q.max_bytes = None;
        q.limit = Some(1);
        q.max_seeds = Some(1);
        let result = repo.run(&q);
        assert_eq!(result["changes_truncated"], true);
        assert_eq!(result["changes"].as_array().unwrap().len(), 1);
        repo.write(
            "src/worker.rs",
            "pub fn value()->i32{2}\npub fn removed(){}\n",
        );
        q.limit = None;
        q.depth = Some(1);
        let result = repo.run(&q);
        assert_eq!(result["before_impact"]["depth_limited"], true);
        q.max_affected = Some(1);
        let result = repo.run(&q);
        assert_eq!(
            result["before_impact"]["items"].as_array().unwrap().len(),
            1
        );
    }
    #[test]
    fn allowlist_revisions_and_external_filters_are_guarded() {
        let repo = Repo::new();
        let q = repo.query("all");
        assert!(git_diff_impact(&q, &[]).is_err());
        let mut invalid = repo.query("commits");
        invalid.before = Some("--help".into());
        invalid.after = Some("HEAD".into());
        assert!(git_diff_impact(&invalid, std::slice::from_ref(&repo.root)).is_err());
        let mut invalid = repo.query("all");
        invalid.before = Some("HEAD".into());
        assert!(git_diff_impact(&invalid, std::slice::from_ref(&repo.root)).is_err());
        let marker = repo.root.join("FILTER_EXECUTED");
        repo.git(&[
            "config",
            "filter.evil.clean",
            &format!("touch '{}'; cat", marker.display()),
        ]);
        repo.write(".gitattributes", "*.rs filter=evil\n");
        repo.write(
            "src/worker.rs",
            "pub fn value()->i32{4}\npub fn removed(){}\n",
        );
        let result = repo.run(&q);
        assert_eq!(change(&result, "value")["kind"], "modified");
        assert!(!marker.exists());
        let mut git = capture::Git::open(&q, std::slice::from_ref(&repo.root)).unwrap();
        let (_, entries) = git.index().unwrap();
        let untracked = git.untracked().unwrap();
        let captured = git
            .working(entries, &untracked, std::slice::from_ref(&repo.root))
            .unwrap();
        repo.write("src/worker.rs", "pub fn value()->i32{5}\n");
        assert!(git
            .validate_working(&captured, std::slice::from_ref(&repo.root))
            .is_err());
    }
    #[test]
    fn manifest_only_feature_switch_changes_context_and_binding() {
        let repo = Repo::new();
        let manifest="[package]\nname='git-fixture'\nversion='0.1.0'\n[features]\nfast=[]\n[package.metadata.rag.graph]\nversion=1\ntarget_kind='lib'\nfeatures=['fast']\ndefault_features=false\n";
        repo.write("Cargo.toml", manifest);
        repo.write("src/lib.rs","#[cfg(feature=\"fast\")]pub fn chosen(){}\n#[cfg(not(feature=\"fast\"))]pub fn other(){}\npub fn stable(){}\n");
        repo.git(&["add", "."]);
        repo.write(
            "Cargo.toml",
            &manifest.replace("features=['fast']", "features=[]"),
        );
        let result = repo.run(&repo.query("working_tree"));
        assert_eq!(change(&result, "chosen")["kind"], "removed");
        assert_eq!(change(&result, "other")["kind"], "added");
        assert_eq!(change(&result, "stable")["kind"], "configuration_changed");
    }
    #[test]
    fn parse_errors_and_absent_sparse_sources_do_not_claim_symbol_deletion() {
        let repo = Repo::new();
        repo.write("src/worker.rs", "pub fn value( { broken");
        let result = repo.run(&repo.query("working_tree"));
        assert_eq!(change(&result, "value")["kind"], "unavailable_after");
        assert_eq!(
            result["after_graph"]["source_mapping_complete_within_supported_inputs"],
            false
        );
        repo.write(
            "src/worker.rs",
            "pub fn value()->i32{1}\npub fn removed(){}\n",
        );
        repo.git(&["update-index", "--skip-worktree", "src/worker.rs"]);
        std::fs::remove_file(repo.root.join("src/worker.rs")).unwrap();
        let result = repo.run(&repo.query("working_tree"));
        assert_eq!(result["changed_symbol_groups_total"], 0, "{result}");
        assert_eq!(result["files_changed_total"], 0);
        assert!(result["after"]["source_files"].as_u64().unwrap() > 0);
    }
}
