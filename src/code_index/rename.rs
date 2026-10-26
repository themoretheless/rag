//! Reviewed, source-bound Rust rename plans. No textual fallback for unbound names.
use super::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameQuery {
    pub symbol: String,
    pub new_name: String,
    /// Required project boundary. No cross-project source edits.
    pub project: String,
    pub path: Option<String>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameApply {
    pub query: RenameQuery,
    /// Exact token from a successful preview. Recomputed before any source write.
    pub plan_token: String,
}
#[derive(Debug, Clone, Serialize)]
struct Edit {
    start_byte: usize,
    end_byte: usize,
    old_text: String,
    new_text: String,
    evidence: String,
}
struct Input {
    id: String,
    path: PathBuf,
    source: String,
    hash: String,
    tree: Option<tree_sitter::Tree>,
    edits: BTreeMap<usize, Edit>,
}
struct Plan {
    report: Value,
    inputs: Vec<Input>,
}
fn rust_tree(source: &str) -> Result<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|e| AppError::Invalid(e.to_string()))?;
    parser
        .parse(source, None)
        .ok_or_else(|| AppError::Invalid("Rust parse unavailable".into()))
}
fn identifier(name: &str) -> Result<()> {
    let raw = name.starts_with("r#");
    let spelling = name.strip_prefix("r#").unwrap_or(name);
    let forbidden = matches!(spelling, "_" | "self" | "Self" | "super" | "crate")
        || (!raw
            && matches!(
                spelling,
                "as" | "async"
                    | "await"
                    | "break"
                    | "const"
                    | "continue"
                    | "dyn"
                    | "else"
                    | "enum"
                    | "extern"
                    | "false"
                    | "fn"
                    | "for"
                    | "if"
                    | "impl"
                    | "in"
                    | "let"
                    | "loop"
                    | "match"
                    | "mod"
                    | "move"
                    | "mut"
                    | "pub"
                    | "ref"
                    | "return"
                    | "static"
                    | "struct"
                    | "trait"
                    | "true"
                    | "type"
                    | "unsafe"
                    | "use"
                    | "where"
                    | "while"
                    | "abstract"
                    | "become"
                    | "box"
                    | "do"
                    | "final"
                    | "gen"
                    | "macro"
                    | "override"
                    | "priv"
                    | "try"
                    | "typeof"
                    | "unsized"
                    | "virtual"
                    | "yield"
            ));
    if forbidden {
        return Err(AppError::Invalid(
            "new_name is a reserved Rust identifier".into(),
        ));
    }
    let source = format!("fn {name}() {{}}");
    let tree = rust_tree(&source)?;
    let root = tree.root_node();
    let item = root.named_child(0);
    if root.has_error()
        || root.named_child_count() != 1
        || !item.is_some_and(|n| {
            n.kind() == "function_item"
                && n.child_by_field_name("name")
                    .is_some_and(|n| text(n, &source) == name)
        })
    {
        return Err(AppError::Invalid(
            "new_name must be one Rust identifier (raw identifiers allowed)".into(),
        ));
    }
    Ok(())
}
fn name_eq(left: &str, right: &str) -> bool {
    use unicode_normalization::UnicodeNormalization;
    left.trim_start_matches("r#")
        .nfc()
        .eq(right.trim_start_matches("r#").nfc())
}
fn metadata_input(path: &str) -> bool {
    let path = Path::new(path);
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    matches!(
        name,
        "Dockerfile" | "Containerfile" | "Cargo.lock" | ".dockerignore" | ".gitignore"
    ) || name.starts_with("Dockerfile.")
        || name.starts_with("Containerfile.")
        || matches!(
            path.extension()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some(
                "md" | "markdown"
                    | "yaml"
                    | "yml"
                    | "json"
                    | "toml"
                    | "dockerfile"
                    | "lock"
                    | "log"
            )
        )
}
fn leaves(node: Node, source: &str, old: &str, new: &str, matches: &mut Vec<(usize, usize, bool)>) {
    if matches!(
        node.kind(),
        "identifier" | "type_identifier" | "field_identifier"
    ) {
        let value = text(node, source);
        if name_eq(value, old) || name_eq(value, new) {
            matches.push((node.start_byte(), node.end_byte(), name_eq(value, new)));
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        leaves(child, source, old, new, matches);
    }
}
fn live(path: &Path, roots: &[PathBuf]) -> Result<(PathBuf, String)> {
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(AppError::Invalid(
            "rename refuses symlink source files".into(),
        ));
    }
    let canonical = path.canonicalize()?;
    crate::util::check_path_allowlist(&canonical, roots)?;
    let metadata = std::fs::metadata(&canonical)?;
    if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 {
        return Err(AppError::Invalid(
            "rename needs regular UTF-8 files of at most 4 MiB".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(AppError::Invalid(
                "rename refuses multiply linked files".into(),
            ));
        }
    }
    let bytes = std::fs::read(&canonical)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(AppError::Invalid("source grew beyond rename budget".into()));
    }
    let source = String::from_utf8(bytes).map_err(|_| {
        AppError::Invalid("rename requires UTF-8 without decoding transformations".into())
    })?;
    if source.starts_with('\u{feff}') {
        return Err(AppError::Invalid(
            "rename does not support BOM sources".into(),
        ));
    }
    Ok((canonical, source))
}
fn build(conn: &duckdb::Connection, q: &RenameQuery, roots: &[PathBuf]) -> Result<Plan> {
    identifier(&q.new_name)?;
    if q.project.trim().is_empty() {
        return Err(AppError::Invalid("project is required".into()));
    }
    let snapshot = snapshot_locked(conn, Some(&q.project))?;
    let tests = snapshot_locked_view(conn, Some(&q.project), true)?;
    let mut targets: Vec<_> = snapshot
        .symbols
        .iter()
        .filter(|s| {
            q.path.as_ref().is_none_or(|p| p == &s.path)
                && (s.id == q.symbol || s.name == q.symbol || s.qualified_name == q.symbol)
        })
        .collect();
    if targets.iter().any(|s| s.id == q.symbol) {
        targets.retain(|s| s.id == q.symbol);
    }
    let mut report = json!({"contract_version":1,"status":"blocked","index_revision":snapshot.index_revision,"project":q.project,"new_name":q.new_name,"candidates":targets.iter().take(50).collect::<Vec<_>>(),"candidates_total":targets.len(),"blockers":[],"changes":[],"plan_token":null,"applied":false,"compiler_verified":false,"coverage_complete":false,"scope":"captured Rust symbols and statically bound occurrences; all captured project inputs revalidated","limitations":["external consumers, generated sources and dynamic strings are outside captured coverage","unknown identifier occurrences block instead of textual replacement","macros, trait method families, module/file moves and non-Rust sources are unsupported","multi-file application has a recovery journal and rollback; it is not a crash-atomic filesystem transaction"]});
    let mut blockers = Vec::<Value>::new();
    if targets.len() != 1 {
        report["blockers"] = json!([{"code":"ambiguous_or_missing_symbol"}]);
        return Ok(Plan {
            report,
            inputs: Vec::new(),
        });
    }
    let target = targets[0];
    let mut target_ids = HashSet::from([target.id.as_str()]);
    for symbol in &tests.symbols {
        if symbol.document_id == target.document_id
            && symbol.start_byte == target.start_byte
            && symbol.name == target.name
        {
            target_ids.insert(symbol.id.as_str());
        }
    }
    if name_eq(&target.name, &q.new_name) {
        blockers.push(json!({"code":"same_name"}));
    }
    if !target.path.ends_with(".rs")
        || !matches!(
            target.kind.as_str(),
            "function_item"
                | "struct_item"
                | "enum_item"
                | "type_item"
                | "const_item"
                | "static_item"
        )
    {
        blockers.push(json!({"code":"unsupported_definition_kind","kind":target.kind}));
    }
    if !snapshot.stale_documents.is_empty()
        || !snapshot.unindexed_documents.is_empty()
        || !snapshot.parse_errors.is_empty()
        || !snapshot.binding_diagnostics.is_empty()
    {
        blockers.push(json!({"code":"incomplete_or_stale_project_graph","stale":snapshot.stale_documents,"unindexed":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors,"diagnostics":snapshot.binding_diagnostics}));
    }
    let revision = snapshot
        .index_revision
        .ok_or_else(|| AppError::Invalid("index required".into()))?;
    let files = history::files(conn, revision, Some(&q.project))?;
    if files.len() > 1000 || files.iter().map(|f| f.source.len()).sum::<usize>() > 32 * 1024 * 1024
    {
        return Err(AppError::Invalid(
            "rename project exceeds 1000-file/32-MiB budget; no partial plan".into(),
        ));
    }
    let mut inputs = Vec::new();
    let mut seen = HashSet::new();
    for f in files {
        let (path, source) = live(Path::new(&f.path), roots)?;
        let hash = content_hash(&source);
        if hash != f.hash {
            blockers.push(json!({"code":"filesystem_stale","path":f.path}));
        }
        if !seen.insert(path.clone()) {
            blockers.push(json!({"code":"duplicate_source_identity","path":f.path}));
        }
        let tree = if f.path.ends_with(".rs") {
            Some(rust_tree(&source)?)
        } else if rust_cargo::manifest(&f.path) || metadata_input(&f.path) {
            None
        } else {
            blockers.push(json!({"code":"unsupported_source_language","path":f.path}));
            None
        };
        if tree.as_ref().is_some_and(|t| t.root_node().has_error()) {
            blockers.push(json!({"code":"source_parse_error","path":f.path}));
        }
        inputs.push(Input {
            id: f.id,
            path,
            source,
            hash,
            tree,
            edits: BTreeMap::new(),
        });
    }
    let mut declaration = None;
    for input in &mut inputs {
        if input.id != target.document_id {
            continue;
        }
        if let Some(tree) = &input.tree {
            let mut node = tree
                .root_node()
                .descendant_for_byte_range(target.start_byte, target.end_byte);
            while let Some(n) = node {
                if n.start_byte() == target.start_byte
                    && n.end_byte() == target.end_byte
                    && n.kind() == target.kind
                {
                    let mut ancestor = n.parent();
                    while let Some(a) = ancestor {
                        if a.kind() == "trait_item"
                            || (a.kind() == "impl_item" && a.child_by_field_name("trait").is_some())
                        {
                            blockers.push(json!({"code":"trait_family_unsupported"}));
                        }
                        ancestor = a.parent();
                    }
                    declaration = n
                        .child_by_field_name("name")
                        .filter(|n| text(*n, &input.source) == target.name)
                        .map(|n| (n.start_byte(), n.end_byte()));
                    break;
                }
                node = n.parent();
            }
        }
        if let Some((start, end)) = declaration {
            input.edits.insert(
                start,
                Edit {
                    start_byte: start,
                    end_byte: end,
                    old_text: input.source[start..end].into(),
                    new_text: q.new_name.clone(),
                    evidence: "definition".into(),
                },
            );
        }
    }
    if declaration.is_none() {
        blockers.push(json!({"code":"definition_span_unavailable"}));
    }
    let mut other_bound = HashSet::new();
    for edge in snapshot
        .bound_references
        .iter()
        .chain(&tests.bound_references)
    {
        let r = &edge.reference;
        let Some(input) = inputs.iter_mut().find(|i| i.id == r.document_id) else {
            continue;
        };
        // line/column name anchors refer to the terminal identifier, not call start.
        let offset = input
            .source
            .split_inclusive('\n')
            .take(r.line.saturating_sub(1))
            .map(str::len)
            .sum::<usize>()
            + r.column.saturating_sub(1);
        let end = offset + r.name.len();
        if edge
            .candidates
            .iter()
            .any(|id| target_ids.contains(id.as_str()))
        {
            if edge.candidates.len() != 1
                || !name_eq(&r.name, &target.name)
                || input.source.get(offset..end) != Some(r.name.as_str())
            {
                blockers.push(json!({"code":"ambiguous_or_alias_occurrence","path":r.path,"line":r.line,"expression":r.expression}));
                continue;
            }
            input.edits.insert(
                offset,
                Edit {
                    start_byte: offset,
                    end_byte: end,
                    old_text: r.name.clone(),
                    new_text: q.new_name.clone(),
                    evidence: edge.resolution_basis.clone(),
                },
            );
        } else if !edge.candidates.is_empty() {
            other_bound.insert((input.id.clone(), offset));
        }
    }
    for input in &inputs {
        let Some(tree) = &input.tree else {
            continue;
        };
        let mut names = Vec::new();
        leaves(
            tree.root_node(),
            &input.source,
            &target.name,
            &q.new_name,
            &mut names,
        );
        for (start, end, is_new) in names {
            if is_new {
                blockers.push(
                    json!({"code":"possible_name_collision","path":input.path,"start_byte":start}),
                );
            } else if !input.edits.contains_key(&start)
                && !other_bound.contains(&(input.id.clone(), start))
            {
                // Same-name unrelated declarations can be proved by exact definition anchors.
                let unrelated = snapshot.symbols.iter().any(|s| {
                    s.id != target.id
                        && s.document_id == input.id
                        && s.name == target.name
                        && s.start_byte <= start
                        && end <= s.end_byte
                        && input
                            .tree
                            .as_ref()
                            .and_then(|t| t.root_node().descendant_for_byte_range(start, end))
                            .is_some_and(|n| {
                                n.parent().is_some_and(|p| {
                                    p.child_by_field_name("name")
                                        .is_some_and(|name| name.id() == n.id())
                                        && p.start_byte() == s.start_byte
                                })
                            })
                });
                if !unrelated {
                    blockers.push(json!({"code":"unaccounted_identifier","path":input.path,"start_byte":start,"text":&input.source[start..end]}));
                }
            }
        }
    }
    let changes:Vec<_>=inputs.iter().filter(|i|!i.edits.is_empty()).map(|i|json!({"document_id":i.id,"path":i.path,"source_hash":i.hash,"edits":i.edits.values().collect::<Vec<_>>()})).collect();
    let guards: Vec<_> = inputs
        .iter()
        .map(|i| json!({"document_id":i.id,"path":i.path,"source_hash":i.hash}))
        .collect();
    report["symbol"] = json!(target);
    report["changes"] = json!(changes);
    report["guards"] = json!(guards);
    report["blockers_total"] = json!(blockers.len());
    report["blockers"] = json!(blockers.iter().take(100).collect::<Vec<_>>());
    report["blockers_truncated"] = json!(blockers.len() > 100);
    let edits_total: usize = inputs.iter().map(|i| i.edits.len()).sum();
    if edits_total > 10000 || serde_json::to_vec(&report)?.len() > 4 * 1024 * 1024 {
        return Err(AppError::Invalid(
            "rename preview exceeds 10000-edit/4-MiB response budget; no partial plan".into(),
        ));
    }
    report["edits_total"] = json!(edits_total);
    if blockers.is_empty() {
        report["status"] = json!("preview");
        report["plan_token"] = json!(content_hash(&serde_json::to_string(&report)?));
    }
    Ok(Plan { report, inputs })
}

impl Store {
    pub fn code_rename_preview(&self, q: &RenameQuery, roots: &[PathBuf]) -> Result<Value> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let plan = build(&tx, q, roots)?;
        tx.commit()?;
        Ok(plan.report)
    }
    pub fn code_rename_apply(&self, q: &RenameApply, roots: &[PathBuf]) -> Result<Value> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let plan = build(&tx, &q.query, roots)?;
        if plan.report["status"] != "preview"
            || plan.report["plan_token"].as_str() != Some(q.plan_token.as_str())
        {
            return Err(AppError::Invalid(
                "rename plan changed or blocked; request and review a fresh preview".into(),
            ));
        }
        // Finish the read transaction before writing files, retaining the Store lock
        // so ingestion cannot silently advance the captured corpus during apply.
        tx.commit()?;
        let result = apply(plan, roots)?;
        drop(conn);
        Ok(result)
    }
}

fn exclusive(path: &Path, bytes: &[u8], permissions: Option<std::fs::Permissions>) -> Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    if let Some(p) = permissions {
        f.set_permissions(p)?;
    }
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(())
}
fn journal(path: &Path, value: &Value) -> Result<()> {
    // The main journal is never truncated in place: each replacement is synced.
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    exclusive(&temporary, &serde_json::to_vec_pretty(value)?, None)?;
    std::fs::rename(&temporary, path)?;
    #[cfg(unix)]
    std::fs::File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
struct Staged {
    path: PathBuf,
    backup: PathBuf,
    temporary: PathBuf,
    old_hash: String,
    new_hash: String,
}
fn apply(plan: Plan, roots: &[PathBuf]) -> Result<Value> {
    apply_with(plan, roots, |_, _| Ok(()))
}
fn apply_with(
    mut plan: Plan,
    roots: &[PathBuf],
    mut before_replace: impl FnMut(usize, &Path) -> Result<()>,
) -> Result<Value> {
    // Check every provider/configuration guard again, including unedited files.
    for input in &plan.inputs {
        let (path, source) = live(&input.path, roots)?;
        if path != input.path || content_hash(&source) != input.hash {
            return Err(AppError::Invalid(
                "rename input changed during apply; no edits applied".into(),
            ));
        }
    }
    let operation = uuid::Uuid::new_v4().to_string();
    let mut staged = Vec::<Staged>::new();
    for input in &plan.inputs {
        if input.edits.is_empty() {
            continue;
        }
        let metadata = std::fs::metadata(&input.path)?;
        if metadata.permissions().readonly() {
            return Err(AppError::Invalid(
                "rename refuses read-only source files".into(),
            ));
        }
        let mut changed = input.source.clone();
        for edit in input.edits.values().rev() {
            if changed.get(edit.start_byte..edit.end_byte) != Some(edit.old_text.as_str()) {
                return Err(AppError::Invalid(
                    "overlapping or invalid rename spans".into(),
                ));
            }
            changed.replace_range(edit.start_byte..edit.end_byte, &edit.new_text);
        }
        if rust_tree(&changed)?.root_node().has_error() {
            return Err(AppError::Invalid(
                "rename produces invalid Rust syntax".into(),
            ));
        }
        let parent = input
            .path
            .parent()
            .ok_or_else(|| AppError::Invalid("source parent missing".into()))?;
        let stem = format!(".rag-rename-{operation}-{}", staged.len());
        let backup = parent.join(format!("{stem}.bak"));
        let temporary = parent.join(format!("{stem}.new"));
        exclusive(
            &backup,
            input.source.as_bytes(),
            Some(metadata.permissions()),
        )?;
        exclusive(&temporary, changed.as_bytes(), Some(metadata.permissions()))?;
        staged.push(Staged {
            path: input.path.clone(),
            backup,
            temporary,
            old_hash: input.hash.clone(),
            new_hash: content_hash(&changed),
        });
    }
    let first = staged
        .first()
        .ok_or_else(|| AppError::Invalid("rename has no edits".into()))?;
    let journal_path = first
        .path
        .parent()
        .unwrap()
        .join(format!(".rag-rename-{operation}.json"));
    let mut state = json!({"contract_version":1,"operation":operation,"status":"prepared","plan_token":plan.report["plan_token"],"files":staged.iter().map(|s|json!({"path":s.path,"backup":s.backup,"staged":s.temporary,"old_hash":s.old_hash,"new_hash":s.new_hash})).collect::<Vec<_>>(),"committed_files":[],"rollback_conflicts":[]});
    // A prepared journal and original backups survive interruption for recovery.
    #[cfg(unix)]
    for item in &staged {
        std::fs::File::open(item.path.parent().unwrap())?.sync_all()?;
    }
    exclusive(&journal_path, &serde_json::to_vec_pretty(&state)?, None)?;
    #[cfg(unix)]
    std::fs::File::open(journal_path.parent().unwrap())?.sync_all()?;
    let mut committed = Vec::<usize>::new();
    let outcome = (|| -> Result<()> {
        for input in &plan.inputs {
            let (_, source) = live(&input.path, roots)?;
            if content_hash(&source) != input.hash {
                return Err(AppError::Invalid(
                    "rename guard changed after staging".into(),
                ));
            }
        }
        for (index, item) in staged.iter().enumerate() {
            before_replace(index, &item.path)?;
            let (_, source) = live(&item.path, roots)?;
            if content_hash(&source) != item.old_hash {
                return Err(AppError::Invalid(
                    "rename file changed before replacement".into(),
                ));
            }
            std::fs::rename(&item.temporary, &item.path)?;
            committed.push(index);
            #[cfg(unix)]
            std::fs::File::open(item.path.parent().unwrap())?.sync_all()?;
            state["committed_files"] = json!(committed);
            state["status"] = json!("applying");
            journal(&journal_path, &state)?;
        }
        state["status"] = json!("applied");
        journal(&journal_path, &state)?;
        Ok(())
    })();
    if let Err(error) = outcome {
        let mut conflicts = Vec::new();
        for index in committed.iter().rev() {
            let item = &staged[*index];
            let restore = (|| -> Result<()> {
                let (_, current) = live(&item.path, roots)?;
                if content_hash(&current) != item.new_hash {
                    return Err(AppError::Invalid(
                        "concurrent edit preserved during rollback".into(),
                    ));
                }
                let original = std::fs::read(&item.backup)?;
                if content_hash(
                    std::str::from_utf8(&original).map_err(|e| AppError::Invalid(e.to_string()))?,
                ) != item.old_hash
                {
                    return Err(AppError::Invalid("backup checksum mismatch".into()));
                }
                let temporary = item.backup.with_extension("restore");
                exclusive(
                    &temporary,
                    &original,
                    Some(std::fs::metadata(&item.backup)?.permissions()),
                )?;
                std::fs::rename(temporary, &item.path)?;
                #[cfg(unix)]
                std::fs::File::open(item.path.parent().unwrap())?.sync_all()?;
                Ok(())
            })();
            if let Err(e) = restore {
                conflicts.push(json!({"path":item.path,"error":e.to_string()}));
            }
        }
        state["status"] = json!(if conflicts.is_empty() {
            "rolled_back"
        } else {
            "recovery_required"
        });
        state["error"] = json!(error.to_string());
        state["rollback_conflicts"] = json!(conflicts);
        // Even journal write failure must return the retained recovery locations.
        let journal_error = journal(&journal_path, &state).err().map(|e| e.to_string());
        return Ok(
            json!({"status":state["status"],"success":false,"applied":false,"error":error.to_string(),"journal":journal_path,"journal_state":state,"journal_error":journal_error,"backups_retained":true}),
        );
    }
    plan.report["status"] = json!("applied");
    plan.report["applied"] = json!(true);
    plan.report["journal"] = json!(journal_path);
    plan.report["backups_retained"] = json!(true);
    plan.report["index_refresh_required"] = json!(true);
    plan.report["compiler_check_required"] = json!(true);
    Ok(plan.report)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        store: Store,
        dir: tempfile::TempDir,
        root: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap().join("sources");
            std::fs::create_dir_all(root.join("src")).unwrap();
            let store = Store::open(&dir.path().join("rename.duckdb")).unwrap();
            let f = Self { store, dir, root };
            for (id,source) in [("Cargo.toml","[package]\nname='rename-fixture'\nversion='0.1.0'\nedition='2021'\n"),("src/lib.rs","pub mod api; pub mod client;\n// value stays in comments\npub const MESSAGE: &str = \"value\";\n"),("src/api.rs","pub fn value()->i32{1}\n"),("src/client.rs","pub fn use_value()->i32{crate::api::value()}\n")] {f.put(id,source,1);}
            f.store.index_code().unwrap();
            f
        }
        fn put(&self, id: &str, source: &str, revision: i64) {
            let path = self.root.join(id);
            std::fs::write(&path, source).unwrap();
            self.store
                .upsert_document(&crate::models::Document {
                    id: id.into(),
                    uri: format!("file://{}", path.display()),
                    source_file: Some(path.to_string_lossy().into_owned()),
                    content: source.into(),
                    revision,
                    wing: Some("rename".into()),
                    ..Default::default()
                })
                .unwrap();
        }
        fn query(&self) -> RenameQuery {
            RenameQuery {
                symbol: "value".into(),
                new_name: "compute".into(),
                project: "rename".into(),
                path: None,
            }
        }
        fn preview(&self) -> Value {
            self.store
                .code_rename_preview(&self.query(), std::slice::from_ref(&self.root))
                .unwrap()
        }
        fn plan(&self) -> Plan {
            let conn = self.store.lock().unwrap();
            build(&conn, &self.query(), std::slice::from_ref(&self.root)).unwrap()
        }
    }
    #[test]
    fn preview_is_exact_and_apply_preserves_backups_and_requires_refresh() {
        let f = Fixture::new();
        let p = f.preview();
        assert_eq!(p["status"], "preview", "{p}");
        assert_eq!(p, f.preview());
        assert_eq!(p["changes"].as_array().unwrap().len(), 2);
        let before = std::fs::read_to_string(f.root.join("src/api.rs")).unwrap();
        assert!(before.contains("fn value"));
        let r = f
            .store
            .code_rename_apply(
                &RenameApply {
                    query: f.query(),
                    plan_token: p["plan_token"].as_str().unwrap().into(),
                },
                std::slice::from_ref(&f.root),
            )
            .unwrap();
        assert_eq!(r["status"], "applied");
        assert_eq!(r["index_refresh_required"], true);
        assert_eq!(
            std::fs::read_to_string(f.root.join("src/api.rs")).unwrap(),
            "pub fn compute()->i32{1}\n"
        );
        assert!(std::fs::read_to_string(f.root.join("src/client.rs"))
            .unwrap()
            .contains("api::compute()"));
        assert!(std::fs::read_to_string(f.root.join("src/lib.rs"))
            .unwrap()
            .contains("\"value\""));
        let journal: Value =
            serde_json::from_slice(&std::fs::read(r["journal"].as_str().unwrap()).unwrap())
                .unwrap();
        assert_eq!(journal["status"], "applied");
        for file in journal["files"].as_array().unwrap() {
            let text = std::fs::read_to_string(file["backup"].as_str().unwrap()).unwrap();
            assert_eq!(content_hash(&text), file["old_hash"]);
        }
        assert_eq!(f.preview()["status"], "blocked");
    }
    #[test]
    fn stale_unedited_input_and_wrong_token_never_mutate_sources() {
        let f = Fixture::new();
        let p = f.preview();
        assert_eq!(p["status"], "preview", "{p}");
        let mut apply = RenameApply {
            query: f.query(),
            plan_token: "forged".into(),
        };
        assert!(f
            .store
            .code_rename_apply(&apply, std::slice::from_ref(&f.root))
            .is_err());
        apply.plan_token = p["plan_token"].as_str().unwrap().into();
        std::fs::write(
            f.root.join("src/lib.rs"),
            "pub mod api;pub mod client;// concurrent\n",
        )
        .unwrap();
        assert!(f
            .store
            .code_rename_apply(&apply, std::slice::from_ref(&f.root))
            .is_err());
        assert_eq!(
            std::fs::read_to_string(f.root.join("src/api.rs")).unwrap(),
            "pub fn value()->i32{1}\n"
        );
    }
    #[test]
    fn captured_docs_and_configuration_keep_guards_without_becoming_rust_rename_blockers() {
        let f = Fixture::new();
        f.put(
            "README.md",
            "value and compute are documentation, not a Rust binding.\n",
            1,
        );
        f.put(
            "compose.yaml",
            "services: {value: {image: demo/compute:v1}}\n",
            1,
        );
        f.put("Dockerfile", "FROM scratch\n# value compute\n", 1);
        f.store.index_code().unwrap();
        let preview = f.preview();
        assert_eq!(preview["status"], "preview", "{preview}");
        assert_eq!(preview["changes"].as_array().unwrap().len(), 2);
        assert_eq!(preview["guards"].as_array().unwrap().len(), 7);
        std::fs::write(
            f.root.join("compose.yaml"),
            "services: {changed: {image: other:v1}}\n",
        )
        .unwrap();
        assert!(f
            .store
            .code_rename_apply(
                &RenameApply {
                    query: f.query(),
                    plan_token: preview["plan_token"].as_str().unwrap().into()
                },
                std::slice::from_ref(&f.root)
            )
            .is_err());
        assert!(std::fs::read_to_string(f.root.join("src/api.rs"))
            .unwrap()
            .contains("fn value"));
    }
    #[test]
    fn collisions_unknown_occurrences_aliases_and_trait_families_block() {
        for source in [
            "pub fn use_value()->i32{let compute=1;crate::api::value()+compute}",
            "macro_rules! x {()=>{value()}} pub fn use_value()->i32{crate::api::value()}",
            "use crate::api::value as alias;pub fn use_value()->i32{alias()}",
        ] {
            let f = Fixture::new();
            f.put("src/client.rs", source, 2);
            f.store.index_code().unwrap();
            let p = f.preview();
            assert_eq!(p["status"], "blocked", "{p}");
            assert!(p["plan_token"].is_null());
        }
        let f = Fixture::new();
        f.put("src/api.rs", "pub trait T{fn value()->i32{1}}", 2);
        f.store.index_code().unwrap();
        let p = f.preview();
        assert_eq!(p["status"], "blocked", "{p}");
        for name in ["fn", "x(){} fn injected", "r#self", ""] {
            let mut q = f.query();
            q.new_name = name.into();
            assert!(f
                .store
                .code_rename_preview(&q, std::slice::from_ref(&f.root))
                .is_err());
        }
    }
    #[test]
    fn replacement_failure_rolls_back_and_conflicting_editor_change_is_preserved() {
        for concurrent in [false, true] {
            let f = Fixture::new();
            let plan = f.plan();
            assert_eq!(plan.report["status"], "preview", "{}", plan.report);
            let mut first = None::<PathBuf>;
            let result = apply_with(plan, std::slice::from_ref(&f.root), |index, path| {
                if index == 0 {
                    first = Some(path.into());
                    return Ok(());
                }
                if concurrent {
                    std::fs::write(first.as_ref().unwrap(), "// external editor change\n")?;
                }
                Err(AppError::Invalid(
                    "injected second replacement failure".into(),
                ))
            })
            .unwrap();
            assert_eq!(
                result["status"],
                if concurrent {
                    "recovery_required"
                } else {
                    "rolled_back"
                }
            );
            if concurrent {
                assert_eq!(
                    std::fs::read_to_string(first.unwrap()).unwrap(),
                    "// external editor change\n"
                );
            } else {
                assert_eq!(
                    std::fs::read_to_string(f.root.join("src/api.rs")).unwrap(),
                    "pub fn value()->i32{1}\n"
                );
            }
            assert!(Path::new(result["journal"].as_str().unwrap()).exists());
        }
    }
    #[test]
    fn paths_permissions_and_source_encoding_are_checked() {
        let f = Fixture::new();
        assert!(f.store.code_rename_preview(&f.query(), &[]).is_err());
        for bytes in [
            &b"\xffinvalid"[..],
            &b"\xef\xbb\xbfpub fn value()->i32{1}\n"[..],
        ] {
            let invalid = Fixture::new();
            std::fs::write(invalid.root.join("src/api.rs"), bytes).unwrap();
            assert!(invalid
                .store
                .code_rename_preview(&invalid.query(), std::slice::from_ref(&invalid.root))
                .is_err());
        }
        let readonly = Fixture::new();
        let preview = readonly.preview();
        let api = readonly.root.join("src/api.rs");
        let mut permissions = std::fs::metadata(&api).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&api, permissions).unwrap();
        let request = RenameApply {
            query: readonly.query(),
            plan_token: preview["plan_token"].as_str().unwrap().into(),
        };
        assert!(readonly
            .store
            .code_rename_apply(&request, std::slice::from_ref(&readonly.root))
            .is_err());
        assert!(std::fs::read_to_string(&api).unwrap().contains("fn value"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let api = f.root.join("src/api.rs");
            let original = f.dir.path().join("original.rs");
            std::fs::rename(&api, &original).unwrap();
            symlink(&original, &api).unwrap();
            assert!(f
                .store
                .code_rename_preview(&f.query(), std::slice::from_ref(&f.root))
                .is_err());
            std::fs::remove_file(&api).unwrap();
            std::fs::hard_link(&original, &api).unwrap();
            assert!(f
                .store
                .code_rename_preview(&f.query(), std::slice::from_ref(&f.root))
                .is_err());
        }
        assert_eq!(
            crate::access::required_tool_role("rename"),
            Some(crate::access::AccessRole::Read)
        );
        assert_eq!(
            crate::access::required_tool_role("apply_rename"),
            Some(crate::access::AccessRole::Admin)
        );
    }
}
