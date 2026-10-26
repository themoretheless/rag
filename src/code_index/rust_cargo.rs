//! Cargo boundaries derived only from captured source documents. Never runs Cargo,
//! build scripts, proc macros, or reads the live filesystem during graph queries.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
mod reexports;
pub(super) use reexports::Facts as ReexportFacts;

pub(super) fn collect_reexports(root: Node, source: &str, doc: &mut IndexedDocument) {
    reexports::collect(root, source, doc);
}
pub(super) fn configure_reexports(
    doc: &mut IndexedDocument,
    decision: impl Fn(usize) -> Option<bool>,
) {
    reexports::configure(doc, decision);
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct Manifest {
    valid: bool,
    workspace_root: bool,
    package: Option<String>,
    targets: Vec<Target>,
    dependencies: BTreeMap<String, Dependency>,
    workspace_dependencies: BTreeMap<String, Dependency>,
    autolib: bool,
    autobins: bool,
    autotests: bool,
    #[serde(default)]
    build: Option<std::result::Result<rust_cfg::Input, String>>,
    #[serde(default)]
    features: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    selected_target: Option<Target>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Target {
    name: String,
    #[serde(default)]
    cargo_name: String,
    #[serde(default = "yes")]
    test_enabled: bool,
    #[serde(default = "yes")]
    harness: bool,
    kind: String,
    path: String,
    required_features: bool,
    #[serde(default)]
    required_feature_names: Vec<String>,
}
fn yes() -> bool {
    true
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Dependency {
    path: Option<String>,
    package: Option<String>,
    workspace: bool,
    optional: bool,
    dev: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Module {
    scope: Vec<String>,
    name: String,
    path: Option<String>,
    inline: bool,
    conditional: bool,
    #[serde(default)]
    start: usize,
    #[serde(default)]
    end: usize,
    #[serde(default)]
    unmodeled: bool,
}
pub(super) fn manifest(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .is_some_and(|n| n == "Cargo.toml")
}
fn normalized(path: &Path) -> String {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => (),
            Component::ParentDir if result.file_name().is_some() => {
                result.pop();
            }
            other => result.push(other.as_os_str()),
        }
    }
    result.to_string_lossy().replace('\\', "/")
}
fn dependencies(table: Option<&toml_edit::Item>, dev: bool) -> BTreeMap<String, Dependency> {
    let Some(table) = table.and_then(toml_edit::Item::as_table_like) else {
        return BTreeMap::new();
    };
    table
        .iter()
        .map(|(name, item)| {
            let get = |key: &str| item.get(key);
            (
                name.replace('-', "_"),
                Dependency {
                    path: get("path")
                        .and_then(toml_edit::Item::as_str)
                        .map(str::to_string),
                    package: get("package")
                        .and_then(toml_edit::Item::as_str)
                        .map(str::to_string),
                    workspace: get("workspace").and_then(toml_edit::Item::as_bool) == Some(true),
                    optional: get("optional").and_then(toml_edit::Item::as_bool) == Some(true),
                    dev,
                },
            )
        })
        .collect()
}
pub(super) fn parse_manifest(source: &str, out: &mut IndexedDocument) {
    let Ok(parsed) = source.parse::<toml_edit::DocumentMut>() else {
        out.cargo = Some(Manifest::default());
        out.parse_errors = true;
        out.binding_limitations.push("RUST_MANIFEST_INVALID".into());
        return;
    };
    let package = parsed.get("package");
    let name = package
        .and_then(|p| p.get("name"))
        .and_then(toml_edit::Item::as_str);
    let flag = |key: &str| {
        package
            .and_then(|p| p.get(key))
            .and_then(toml_edit::Item::as_bool)
            != Some(false)
    };
    let mut result = Manifest {
        valid: package.is_none() || name.is_some(),
        workspace_root: parsed.get("workspace").is_some(),
        package: name.map(str::to_string),
        autolib: flag("autolib"),
        autobins: flag("autobins"),
        autotests: flag("autotests"),
        dependencies: dependencies(parsed.get("dependencies"), false),
        workspace_dependencies: dependencies(
            parsed.get("workspace").and_then(|w| w.get("dependencies")),
            false,
        ),
        build: package
            .and_then(|p| p.get("metadata"))
            .and_then(|p| p.get("rag"))
            .and_then(|p| p.get("graph"))
            .map(rust_cfg::parse_input),
        features: parsed
            .get("features")
            .and_then(toml_edit::Item::as_table_like)
            .map(|t| {
                t.iter()
                    .map(|(name, value)| {
                        (
                            name.into(),
                            value
                                .as_array()
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|v| v.as_str().map(str::to_string))
                                        .collect()
                                })
                                .unwrap_or_default(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
        ..Manifest::default()
    };
    // A regular dependency wins over a dev-only declaration of the same alias.
    for (alias, dep) in dependencies(parsed.get("dev-dependencies"), true) {
        result.dependencies.entry(alias).or_insert(dep);
    }
    for kind in ["lib", "bin", "test", "bench", "example"] {
        let tables: Vec<&dyn toml_edit::TableLike> = if kind == "lib" {
            parsed
                .get(kind)
                .and_then(toml_edit::Item::as_table_like)
                .into_iter()
                .collect()
        } else {
            parsed
                .get(kind)
                .and_then(toml_edit::Item::as_array_of_tables)
                .map(|a| a.iter().map(|t| t as &dyn toml_edit::TableLike).collect())
                .unwrap_or_default()
        };
        for table in tables {
            let target_name = table
                .get("name")
                .and_then(toml_edit::Item::as_str)
                .or(name)
                .unwrap_or("");
            let default_path = match kind {
                "lib" => "src/lib.rs".into(),
                "bin" if name == Some(target_name) => "src/main.rs".into(),
                "bin" => format!("src/bin/{target_name}.rs"),
                other => format!("{other}s/{target_name}.rs"),
            };
            result.targets.push(Target {
                name: target_name.replace('-', "_"),
                cargo_name: target_name.into(),
                test_enabled: table
                    .get("test")
                    .and_then(toml_edit::Item::as_bool)
                    .unwrap_or(matches!(kind, "lib" | "bin" | "test")),
                harness: table
                    .get("harness")
                    .and_then(toml_edit::Item::as_bool)
                    .unwrap_or(true),
                kind: kind.into(),
                path: table
                    .get("path")
                    .and_then(toml_edit::Item::as_str)
                    .unwrap_or(&default_path)
                    .into(),
                required_features: table
                    .get("required-features")
                    .and_then(toml_edit::Item::as_array)
                    .is_some_and(|a| !a.is_empty()),
                required_feature_names: table
                    .get("required-features")
                    .and_then(toml_edit::Item::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
            });
        }
    }
    if !result.valid {
        out.binding_limitations.push("RUST_MANIFEST_INVALID".into());
    }
    out.cargo = Some(result);
}

pub(super) fn collect_modules(root: Node, source: &str, out: &mut IndexedDocument) {
    fn walk_modules(
        node: Node,
        scope: &[String],
        source: &str,
        out: &mut IndexedDocument,
        inherited: bool,
        depth: usize,
    ) {
        if depth > 64 {
            out.binding_limitations
                .push("RUST_MODULE_DEPTH_LIMIT".into());
            return;
        }
        let mut attrs = String::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "attribute_item" {
                attrs.push_str(text(child, source));
                continue;
            }
            if child.kind() == "line_comment" || child.kind() == "block_comment" {
                continue;
            }
            if child.kind() == "mod_item" {
                if let Some(name) = child.child_by_field_name("name") {
                    let body = child.child_by_field_name("body");
                    // cfg_attr can change a path as well as presence; never guess its branch.
                    let mut conditional =
                        inherited || attrs.contains("cfg") || attrs.contains("macro");
                    let path = attrs
                        .split("path")
                        .nth(1)
                        .and_then(|v| v.split_once('='))
                        .and_then(|(_, v)| {
                            let raw = v.trim().split(']').next()?.trim();
                            // Standard string literals are JSON-compatible; unknown escapes/raw strings fail closed.
                            serde_json::from_str::<String>(raw).ok()
                        });
                    let name = text(name, source).to_string();
                    conditional |= attrs.contains("path") && path.is_none();
                    out.rust_modules.push(Module {
                        start: child.start_byte(),
                        end: child.end_byte(),
                        unmodeled: attrs.contains("macro")
                            || (attrs.contains("path") && path.is_none()),
                        scope: scope.to_vec(),
                        name: name.clone(),
                        path,
                        inline: body.is_some(),
                        conditional,
                    });
                    if let Some(body) = body {
                        let mut nested = scope.to_vec();
                        nested.push(name);
                        walk_modules(body, &nested, source, out, conditional, depth + 1);
                    }
                }
            }
            attrs.clear();
        }
    }
    walk_modules(root, &[], source, out, false, 0);
}

fn nearest<'a>(
    docs: &'a HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
) -> Option<&'a IndexedDocument> {
    docs.values()
        .filter(|d| {
            d.project == doc.project
                && manifest(&d.path)
                && Path::new(&doc.path)
                    .starts_with(Path::new(&d.path).parent().unwrap_or(Path::new("")))
        })
        .max_by_key(|d| d.path.len())
}
pub(super) fn standard_crate_unshadowed(
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
    position: usize,
    name: &str,
) -> bool {
    if !matches!(name, "std" | "alloc") {
        return true;
    }
    if nearest(docs, doc).is_some_and(|m| {
        m.cargo
            .as_ref()
            .is_none_or(|c| !c.valid || c.dependencies.contains_key(name))
    }) {
        return false;
    }
    if let Some(manifest) = nearest(docs, doc) {
        for target in targets(manifest, docs) {
            let map = modules(manifest, &target, docs);
            if map.values().any(|loc| loc.file == doc.path) {
                if let Some(root) = map
                    .get(&Vec::new())
                    .and_then(|loc| at(docs, &doc.project, &loc.file))
                {
                    if resolve::unverified_standard_extern(root, name) {
                        return false;
                    }
                }
            }
        }
    }
    let scope = inline_namespace(doc, position);
    let qualified = scope
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(name))
        .collect::<Vec<_>>()
        .join("::");
    if doc.symbols.iter().any(|s| {
        s.qualified_name == qualified
            && matches!(
                s.kind.as_str(),
                "struct_item" | "enum_item" | "trait_item" | "type_item" | "union_item"
            )
    }) {
        return false;
    }
    !doc.rust_modules
        .iter()
        .any(|m| m.scope == scope && m.name == name)
}
pub(super) fn build_input(
    doc: &IndexedDocument,
) -> Option<&std::result::Result<rust_cfg::Input, String>> {
    doc.cargo.as_ref()?.build.as_ref()
}
pub(super) fn boundary_verified(doc: &IndexedDocument) -> bool {
    doc.cargo
        .as_ref()
        .is_some_and(|m| m.valid && m.package.is_some())
}
pub(super) fn feature_definitions(doc: &IndexedDocument) -> BTreeMap<String, Vec<String>> {
    doc.cargo
        .as_ref()
        .map(|c| c.features.clone())
        .unwrap_or_default()
}
pub(super) fn optional_names(doc: &IndexedDocument) -> HashSet<String> {
    doc.cargo
        .as_ref()
        .map(|c| {
            c.dependencies
                .iter()
                .filter(|(_, d)| d.optional)
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default()
}
pub(super) fn restrict(
    doc: &mut IndexedDocument,
    input: &rust_cfg::Input,
    features: &BTreeSet<String>,
    enabled_deps: &BTreeSet<String>,
    all: &HashMap<String, IndexedDocument>,
) -> std::result::Result<(), String> {
    let matches: Vec<_> = targets(doc, all)
        .into_iter()
        .filter(|t| {
            t.kind == input.target_kind && input.target_name.as_ref().is_none_or(|n| *n == t.name)
        })
        .collect();
    if matches.len() != 1 {
        return Err("target_missing_or_ambiguous".into());
    }
    let mut selected = matches[0].clone();
    if !selected
        .required_feature_names
        .iter()
        .all(|n| features.contains(n))
    {
        return Err("target_required_features_unselected".into());
    }
    selected.required_features = false;
    let manifest = doc.cargo.as_mut().ok_or("manifest_unavailable")?;
    manifest.selected_target = Some(selected);
    for (name, dependency) in &mut manifest.dependencies {
        if enabled_deps.contains(name) {
            dependency.optional = false;
        }
    }
    Ok(())
}
pub(super) fn configured_paths(
    man: &IndexedDocument,
    all: &HashMap<String, IndexedDocument>,
) -> BTreeSet<String> {
    targets(man, all)
        .iter()
        .flat_map(|t| modules(man, t, all).into_values().map(|l| l.file))
        .collect()
}
pub(super) fn filter_modules(doc: &mut IndexedDocument, decision: impl Fn(usize) -> Option<bool>) {
    doc.rust_modules.retain_mut(|m| match decision(m.start) {
        Some(false) => false,
        Some(true) => {
            m.conditional = m.unmodeled;
            true
        }
        None => {
            m.conditional = true;
            true
        }
    });
}
fn at<'a>(
    docs: &'a HashMap<String, IndexedDocument>,
    project: &Option<String>,
    path: &str,
) -> Option<&'a IndexedDocument> {
    let mut matches = docs
        .values()
        .filter(|d| d.project == *project && normalized(Path::new(&d.path)) == path);
    let first = matches.next()?;
    if matches.next().is_some() {
        None
    } else {
        Some(first)
    }
}
fn targets(man: &IndexedDocument, docs: &HashMap<String, IndexedDocument>) -> Vec<Target> {
    let Some(m) = man
        .cargo
        .as_ref()
        .filter(|m| m.valid && m.package.is_some())
    else {
        return vec![];
    };
    if let Some(selected) = &m.selected_target {
        return vec![selected.clone()];
    }
    let base = Path::new(&man.path).parent().unwrap_or(Path::new(""));
    let name = m.package.as_ref().unwrap().replace('-', "_");
    let mut out = m.targets.clone();
    for (kind, path, enabled) in [
        ("lib", "src/lib.rs", m.autolib),
        ("bin", "src/main.rs", m.autobins),
    ] {
        if enabled
            && !out.iter().any(|t| t.kind == kind)
            && at(docs, &man.project, &normalized(&base.join(path))).is_some()
        {
            out.push(Target {
                name: name.clone(),
                cargo_name: m.package.clone().unwrap(),
                test_enabled: true,
                harness: true,
                kind: kind.into(),
                path: path.into(),
                required_features: false,
                required_feature_names: vec![],
            });
        }
    }
    for d in docs
        .values()
        .filter(|d| d.project == man.project && d.path.ends_with(".rs"))
    {
        // A nested package owns its own auto targets.
        if nearest(docs, d).is_none_or(|owner| owner.path != man.path) {
            continue;
        }
        let Ok(relative) = Path::new(&d.path).strip_prefix(base) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        for (kind, prefix, enabled) in [
            ("bin", "src/bin/", m.autobins),
            ("test", "tests/", m.autotests),
        ] {
            if !enabled {
                continue;
            }
            let Some(tail) = relative.strip_prefix(prefix) else {
                continue;
            };
            let target_name = if !tail.contains('/') {
                tail.strip_suffix(".rs")
            } else {
                tail.strip_suffix("/main.rs").filter(|n| !n.contains('/'))
            };
            if let Some(target_name) = target_name {
                if !out.iter().any(|t| {
                    t.path == relative
                        || (t.kind == kind && t.name == target_name.replace('-', "_"))
                }) {
                    out.push(Target {
                        name: target_name.replace('-', "_"),
                        cargo_name: target_name.into(),
                        test_enabled: true,
                        harness: true,
                        kind: kind.into(),
                        path: relative.clone(),
                        required_features: false,
                        required_feature_names: vec![],
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| (&a.kind, &a.name, &a.path).cmp(&(&b.kind, &b.name, &b.path)));
    out
}

/// Captured inline namespace relative to this file's external module.
pub(super) fn inline_namespace(doc: &IndexedDocument, at: usize) -> Vec<String> {
    doc.rust_modules
        .iter()
        .filter(|m| m.inline && m.start <= at && at < m.end)
        .max_by_key(|m| m.scope.len())
        .map(|m| {
            let mut scope = m.scope.clone();
            scope.push(m.name.clone());
            scope
        })
        .unwrap_or_default()
}
pub(super) fn inline_relative_path(doc: &IndexedDocument, at: usize, path: &str) -> String {
    let parts: Vec<_> = path.split("::").collect();
    if !matches!(parts.first().copied(), Some("self" | "super")) {
        return path.into();
    }
    let mut scope = inline_namespace(doc, at);
    if scope.is_empty() {
        return path.into();
    }
    let mut parents = 0;
    let mut tail = 0;
    for part in &parts {
        match *part {
            "self" => (),
            "super" => {
                if scope.pop().is_none() {
                    parents += 1;
                }
            }
            _ => break,
        }
        tail += 1;
    }
    let mut normalized = if parents == 0 {
        vec!["self".to_string()]
    } else {
        vec!["super".to_string(); parents]
    };
    normalized.extend(scope);
    normalized.extend(parts[tail..].iter().map(|s| s.to_string()));
    normalized.join("::")
}

pub(super) fn enable_test_flag(doc: &mut IndexedDocument) {
    if let Some(Ok(input)) = doc.cargo.as_mut().and_then(|m| m.build.as_mut()) {
        input.enable_tests();
    }
}
pub(super) fn test_commands(
    symbols: &[Symbol],
    docs: &HashMap<String, IndexedDocument>,
) -> HashMap<String, Vec<serde_json::Value>> {
    let mut out: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
    for man in docs.values().filter(|m| {
        boundary_verified(m)
            && m.cargo
                .as_ref()
                .is_some_and(|c| c.valid && c.package.is_some())
    }) {
        for target in targets(man, docs).into_iter().filter(|t| {
            t.test_enabled
                && !t.required_features
                && matches!(
                    t.kind.as_str(),
                    "lib" | "bin" | "test" | "example" | "bench"
                )
        }) {
            let members = modules(man, &target, docs);
            let member_paths: HashSet<_> = members.values().map(|l| l.file.as_str()).collect();
            for symbol in symbols
                .iter()
                .filter(|s| member_paths.contains(s.path.as_str()))
            {
                let Some(doc) = docs.get(&symbol.document_id).filter(|d| {
                    d.project == man.project && d.test_definitions.contains(&symbol.start_byte)
                }) else {
                    continue;
                };
                let mut argv = vec![
                    "cargo".into(),
                    "test".into(),
                    "--manifest-path".into(),
                    man.path.clone(),
                ];
                argv.push(format!("--{}", target.kind));
                if target.kind != "lib" {
                    argv.push(if target.cargo_name.is_empty() {
                        target.name.clone()
                    } else {
                        target.cargo_name.clone()
                    });
                }
                if let Some(Ok(input)) = build_input(man) {
                    argv.extend(input.command_options());
                }
                if target.harness {
                    argv.push(symbol.name.trim_start_matches("r#").into());
                    if doc.ignored_test_definitions.contains(&symbol.start_byte) {
                        argv.extend(["--".into(), "--ignored".into()]);
                    }
                }
                let value = serde_json::json!({"argv":argv,"cwd":Path::new(&man.path).parent().unwrap_or(Path::new("")),"kind":"cargo_test","filter_mode":if target.harness{"substring"}else{"unfiltered_custom_harness"},"filter_applied":target.harness,
                    "manifest":{"path":man.path,"revision":man.revision,"content_hash":man.hash,"project":man.project},"target_kind":target.kind,"target_name":target.cargo_name,"test_source_hash":doc.hash,"configuration_review_required":true,"executed":false});
                let entries = out.entry(symbol.id.clone()).or_default();
                if !entries.contains(&value) {
                    entries.push(value);
                }
            }
        }
    }
    for values in out.values_mut() {
        values.sort_by_key(serde_json::Value::to_string);
    }
    out
}
#[derive(Clone)]
struct Location {
    file: String,
    prefix: Vec<String>,
}
fn modules(
    man: &IndexedDocument,
    target: &Target,
    docs: &HashMap<String, IndexedDocument>,
) -> BTreeMap<Vec<String>, Location> {
    let base = Path::new(&man.path).parent().unwrap_or(Path::new(""));
    let root = normalized(&base.join(&target.path));
    let mut result = BTreeMap::new();
    let mut queue = vec![(
        Vec::<String>::new(),
        root.clone(),
        Vec::<String>::new(),
        PathBuf::from(&root)
            .parent()
            .unwrap_or(Path::new(""))
            .to_path_buf(),
    )];
    let mut seen = BTreeSet::new();
    while let Some((scope, file, prefix, module_dir)) = queue.pop() {
        if seen.len() >= 4096 || scope.len() > 64 {
            break;
        }
        if !seen.insert((file.clone(), prefix.clone())) {
            continue;
        }
        let Some(doc) = at(docs, &man.project, &file).filter(|d| !d.parse_errors) else {
            continue;
        };
        // Cross-package #[path] modules are not silently admitted.
        if nearest(docs, doc).is_some_and(|owner| owner.path != man.path) {
            continue;
        }
        result.insert(
            scope.clone(),
            Location {
                file: file.clone(),
                prefix: prefix.clone(),
            },
        );
        for item in doc
            .rust_modules
            .iter()
            .filter(|m| m.scope == prefix && !m.conditional)
        {
            let mut nested = scope.clone();
            nested.push(item.name.clone());
            if item.inline {
                let mut p = prefix.clone();
                p.push(item.name.clone());
                queue.push((nested, file.clone(), p, module_dir.join(&item.name)));
            } else {
                let paths = if let Some(path) = &item.path {
                    vec![normalized(&module_dir.join(path))]
                } else {
                    vec![
                        normalized(&module_dir.join(format!("{}.rs", item.name))),
                        normalized(&module_dir.join(&item.name).join("mod.rs")),
                    ]
                };
                let paths: Vec<_> = paths
                    .into_iter()
                    .filter(|p| at(docs, &man.project, p).is_some())
                    .collect();
                // Rust rejects a simultaneous foo.rs and foo/mod.rs; preserve that ambiguity.
                if paths.len() != 1 {
                    continue;
                }
                let next = &paths[0];
                let next_path = Path::new(next);
                let next_dir = if next_path.file_name().is_some_and(|n| n == "mod.rs") {
                    next_path.parent().unwrap().to_path_buf()
                } else {
                    next_path.with_extension("")
                };
                queue.push((nested, next.clone(), vec![], next_dir));
            }
        }
    }
    result
}

pub(super) fn target_evidence(docs: &HashMap<String, IndexedDocument>) -> Vec<serde_json::Value> {
    let mut result = Vec::new();
    for man in docs.values().filter(|d| {
        d.cargo
            .as_ref()
            .is_some_and(|c| c.valid && c.package.is_some())
    }) {
        for target in targets(man, docs) {
            let members = if target.required_features {
                BTreeMap::new()
            } else {
                modules(man, &target, docs)
            };
            let paths: BTreeSet<_> = members.values().map(|l| &l.file).collect();
            let identity = content_hash(
                &serde_json::to_string(&(&man.project, &man.path, &man.hash, &target)).unwrap(),
            );
            result.push(serde_json::json!({"target_id":identity,"project":man.project,
                "manifest_path":man.path,"manifest_revision":man.revision,"manifest_hash":man.hash,
                "package":man.cargo.as_ref().unwrap().package,"name":target.name,"kind":target.kind,
                "root_path":normalized(&Path::new(&man.path).parent().unwrap_or(Path::new("")).join(&target.path)),
                "member_paths":paths.iter().take(128).collect::<Vec<_>>(),"observed_member_paths_total":paths.len(),"member_paths_truncated":paths.len()>128,
                "scope":"captured_source_syntax","complete":false,
                "required_features_unselected":target.required_features,"features_evaluated":false,
                "cfg_evaluated":false,"macro_expansion":false,"compiler_verified":false}));
        }
    }
    result.sort_by_key(|v| v["target_id"].as_str().unwrap().to_string());
    result
}

/// Whether two captured files coexist in a known target. No manifest means the
/// legacy source-layout model, not a claim about the filesystem or compiler.
pub(super) fn share_target(
    docs: &HashMap<String, IndexedDocument>,
    left: &IndexedDocument,
    right: &IndexedDocument,
) -> bool {
    if left.project != right.project {
        return false;
    }
    match (nearest(docs, left), nearest(docs, right)) {
        (None, None) => true,
        (Some(a), Some(b)) if a.path == b.path => targets(a, docs)
            .iter()
            .filter(|t| !t.required_features)
            .any(|t| {
                let map = modules(a, t, docs);
                map.values().any(|l| l.file == left.path)
                    && map.values().any(|l| l.file == right.path)
            }),
        _ => false,
    }
}

/// Conservative captured package closure. All project manifests remain guards.
pub(super) fn resolution_documents(
    docs: &HashMap<String, IndexedDocument>,
    source: &IndexedDocument,
) -> Option<BTreeSet<String>> {
    let manifests: Vec<_> = docs
        .iter()
        .filter(|(_, d)| d.project == source.project && manifest(&d.path))
        .collect();
    if manifests
        .iter()
        .any(|(_, d)| d.parse_errors || !d.cargo.as_ref().is_some_and(|m| m.valid))
    {
        return None;
    }
    let owner = nearest(docs, source)?;
    if !owner
        .cargo
        .as_ref()
        .is_some_and(|m| m.valid && m.package.is_some())
    {
        return None;
    }
    let mut packages = BTreeSet::new();
    let mut queue = vec![owner.path.clone()];
    while let Some(path) = queue.pop() {
        if !packages.insert(path.clone()) {
            continue;
        }
        let package = at(docs, &source.project, &path)?;
        let facts = package.cargo.as_ref().filter(|m| m.valid)?;
        for (alias, dependency) in &facts.dependencies {
            let mut dep = dependency.clone();
            let mut base = Path::new(&package.path).parent()?.to_path_buf();
            if dep.workspace {
                let workspace = manifests
                    .iter()
                    .filter(|(_, m)| {
                        m.cargo.as_ref().is_some_and(|c| c.workspace_root)
                            && Path::new(&package.path)
                                .starts_with(Path::new(&m.path).parent().unwrap())
                    })
                    .max_by_key(|(_, m)| m.path.len())?
                    .1;
                dep = workspace
                    .cargo
                    .as_ref()?
                    .workspace_dependencies
                    .get(alias)?
                    .clone();
                base = Path::new(&workspace.path).parent()?.to_path_buf();
            }
            if let Some(relative) = dep.path {
                let path = normalized(&base.join(relative).join("Cargo.toml"));
                if at(docs, &source.project, &path).is_some() {
                    queue.push(path);
                }
            }
        }
    }
    Some(
        docs.iter()
            .filter(|(_, d)| {
                d.project == source.project
                    && (manifest(&d.path)
                        || nearest(docs, d).is_none_or(|m| packages.contains(&m.path)))
            })
            .map(|(id, _)| id.clone())
            .collect(),
    )
}
/// Some(empty) means a known Cargo boundary forbids the legacy src-layout fallback.
pub(super) fn imported<'a>(
    symbols: &'a [Symbol],
    docs: &HashMap<String, IndexedDocument>,
    current: &str,
    module: &str,
    name: &str,
    allow_methods: bool,
) -> Option<Vec<&'a Symbol>> {
    let mut handled = false;
    let mut found = Vec::new();
    for doc in docs.values().filter(|d| d.path == current) {
        if let Some(matches) = imported_for_source(symbols, docs, doc, module, name, allow_methods)
        {
            handled = true;
            found.extend(matches);
        }
    }
    handled.then_some(found)
}
pub(super) fn external_member<'a>(
    symbols: &'a [Symbol],
    docs: &HashMap<String, IndexedDocument>,
    current: &str,
    name: &str,
    member: &str,
    allow_methods: bool,
) -> Vec<&'a Symbol> {
    let mut found = vec![];
    for source in docs.values().filter(|d| d.path == current) {
        if nearest(docs, source).is_some_and(|m| {
            m.cargo
                .as_ref()
                .is_some_and(|c| c.valid && c.dependencies.contains_key(name))
        }) {
            found.extend(
                imported_for_source(symbols, docs, source, name, member, allow_methods)
                    .unwrap_or_default(),
            );
        }
    }
    found
}
fn imported_for_source<'a>(
    symbols: &'a [Symbol],
    docs: &HashMap<String, IndexedDocument>,
    doc: &IndexedDocument,
    module: &str,
    name: &str,
    allow_methods: bool,
) -> Option<Vec<&'a Symbol>> {
    let man = nearest(docs, doc)?;
    let m = man.cargo.as_ref()?;
    if !m.valid || m.package.is_none() {
        return Some(vec![]);
    }
    let parts: Vec<_> = module
        .split("::")
        .chain(name.split("::"))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let mut found = Vec::new();
    for target in targets(man, docs)
        .into_iter()
        .filter(|t| !t.required_features)
    {
        let map = modules(man, &target, docs);
        let scopes: Vec<_> = map
            .iter()
            .filter(|(_, loc)| loc.file == doc.path && loc.prefix.is_empty())
            .map(|(s, _)| s.clone())
            .collect();
        for scope in scopes {
            let Some(head) = parts.first() else { continue };
            let mut target_map = map.clone();
            let mut absolute = if head == "self" || head == "super" {
                scope
            } else {
                vec![]
            };
            let mut tail = &parts[..];
            if !matches!(head.as_str(), "crate" | "self" | "super") {
                let own_lib = targets(man, docs)
                    .iter()
                    .any(|t| t.kind == "lib" && &t.name == head);
                if own_lib {
                    let libs: Vec<_> = targets(man, docs)
                        .into_iter()
                        .filter(|t| t.kind == "lib" && !t.required_features)
                        .collect();
                    if libs.len() != 1 {
                        continue;
                    }
                    target_map = modules(man, &libs[0], docs);
                    tail = &parts[1..];
                } else if let Some(dep) = m.dependencies.get(head) {
                    if dep.optional
                        || (dep.dev
                            && !matches!(target.kind.as_str(), "test" | "bench" | "example"))
                    {
                        continue;
                    }
                    let mut dep = dep.clone();
                    let mut base = Path::new(&man.path).parent().unwrap().to_path_buf();
                    if dep.workspace {
                        let workspace = docs
                            .values()
                            .filter(|d| {
                                d.project == man.project
                                    && d.cargo.as_ref().is_some_and(|c| c.workspace_root)
                                    && Path::new(&man.path)
                                        .starts_with(Path::new(&d.path).parent().unwrap())
                            })
                            .max_by_key(|d| d.path.len());
                        let Some(workspace) = workspace else { continue };
                        let Some(inherited) = workspace
                            .cargo
                            .as_ref()
                            .filter(|c| c.valid)
                            .and_then(|c| c.workspace_dependencies.get(head))
                        else {
                            continue;
                        };
                        dep = inherited.clone();
                        base = Path::new(&workspace.path).parent().unwrap().to_path_buf();
                    }
                    let Some(path) = dep.path.as_ref().filter(|_| !dep.optional) else {
                        continue;
                    };
                    let dep_path = normalized(&base.join(path).join("Cargo.toml"));
                    let Some(dep_man) = at(docs, &man.project, &dep_path) else {
                        continue;
                    };
                    if dep.package.as_ref().is_some_and(|p| {
                        dep_man.cargo.as_ref().and_then(|c| c.package.as_ref()) != Some(p)
                    }) {
                        continue;
                    }
                    let libs: Vec<_> = targets(dep_man, docs)
                        .into_iter()
                        .filter(|t| t.kind == "lib" && !t.required_features)
                        .collect();
                    if libs.len() != 1 {
                        continue;
                    }
                    target_map = modules(dep_man, &libs[0], docs);
                    tail = &parts[1..];
                }
            }
            let mut valid = true;
            for part in tail {
                match part.as_str() {
                    "crate" => absolute.clear(),
                    "self" => (),
                    "super" => {
                        if absolute.pop().is_none() {
                            valid = false;
                            break;
                        }
                    }
                    _ => absolute.push(part.clone()),
                }
            }
            if !valid {
                continue;
            }
            match reexports::lookup(
                symbols,
                docs,
                &target_map,
                &map,
                doc,
                &absolute,
                allow_methods,
            ) {
                Ok(Some(candidates)) => {
                    found.extend(candidates);
                    continue;
                }
                Err(()) => return Some(vec![]),
                Ok(None) => (),
            }
            for split in (0..absolute.len()).rev() {
                let Some(location) = target_map.get(&absolute[..split]) else {
                    continue;
                };
                let mut qualified = location.prefix.clone();
                qualified.extend_from_slice(&absolute[split..]);
                let qualified = qualified.join("::");
                let candidates = if allow_methods {
                    super::inheritance::lookup(symbols, docs, &location.file, &qualified)
                } else {
                    super::resolve::matching(symbols, &location.file, &qualified)
                };
                found.extend(candidates.into_iter().filter(|s| {
                    docs.get(&s.document_id)
                        .is_some_and(|d| d.project == doc.project)
                }));
            }
        }
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found.dedup_by(|a, b| a.id == b.id);
    Some(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn graph(files: &[(&str, &str)]) -> CodeSnapshot {
        let mut docs = HashMap::new();
        let mut out = CodeSnapshot::default();
        for (i, (path, source)) in files.iter().enumerate() {
            let mut d = parse(&i.to_string(), path, source, 1).unwrap();
            d.project = Some("fixture".into());
            out.symbols.extend(d.symbols.clone());
            docs.insert(i.to_string(), d);
        }
        out.bound_references = resolve::references(&out.symbols, &docs);
        out.calls = out
            .bound_references
            .iter()
            .filter(|e| e.reference.is_call)
            .cloned()
            .collect();
        out
    }
    fn paths(g: &CodeSnapshot, caller_path: &str, expression: &str) -> Vec<String> {
        let call = g
            .calls
            .iter()
            .find(|e| e.reference.path == caller_path && e.reference.expression == expression)
            .unwrap_or_else(|| panic!("missing {caller_path} {expression}: {:?}", g.calls));
        let mut found: Vec<_> = call
            .candidates
            .iter()
            .map(|id| g.symbols.iter().find(|s| &s.id == id).unwrap().path.clone())
            .collect();
        found.sort();
        found
    }
    #[test]
    fn cargo_separates_lib_binary_and_integration_test_roots() {
        let g = graph(&[
            ("/w/Cargo.toml", "[package]\nname='app'\nversion='0.1.0'\n"),
            (
                "/w/src/lib.rs",
                "pub fn work() {} fn run() { crate::work(); }",
            ),
            (
                "/w/src/main.rs",
                "fn work() {} fn main() { crate::work(); app::work(); }",
            ),
            (
                "/w/tests/check.rs",
                "fn work() {} fn check() { crate::work(); app::work(); }",
            ),
        ]);
        assert_eq!(paths(&g, "/w/src/lib.rs", "crate::work"), ["/w/src/lib.rs"]);
        assert_eq!(
            paths(&g, "/w/src/main.rs", "crate::work"),
            ["/w/src/main.rs"]
        );
        assert_eq!(paths(&g, "/w/src/main.rs", "app::work"), ["/w/src/lib.rs"]);
        assert_eq!(
            paths(&g, "/w/tests/check.rs", "crate::work"),
            ["/w/tests/check.rs"]
        );
        assert_eq!(
            paths(&g, "/w/tests/check.rs", "app::work"),
            ["/w/src/lib.rs"]
        );
    }
    #[test]
    fn cargo_resolves_renamed_path_dependency_and_custom_lib_root() {
        let g = graph(&[
            ("/w/Cargo.toml", "[workspace]\nmembers=['client','engine']\n"),
            ("/w/client/Cargo.toml", "[package]\nname='client'\n[dependencies]\nalias={package='engine',path='../engine'}\n"),
            ("/w/engine/Cargo.toml", "[package]\nname='engine'\n[lib]\npath='code/root.rs'\n"),
            ("/w/engine/code/root.rs", "pub mod api; pub fn work() {}"),
            ("/w/engine/code/api.rs", "pub fn work() {}"),
            ("/w/client/src/main.rs", "fn main(){ alias::work(); alias::api::work(); engine::work(); }"),
            ("/w/client/src/alias.rs", "pub fn work() {}"),
        ]);
        assert_eq!(
            paths(&g, "/w/client/src/main.rs", "alias::work"),
            ["/w/engine/code/root.rs"]
        );
        assert_eq!(
            paths(&g, "/w/client/src/main.rs", "alias::api::work"),
            ["/w/engine/code/api.rs"]
        );
        assert!(paths(&g, "/w/client/src/main.rs", "engine::work").is_empty());
    }
    #[test]
    fn cargo_inherited_dependency_is_relative_to_workspace() {
        let g = graph(&[
            ("/w/Cargo.toml", "[workspace]\nmembers=['client','engine']\n[workspace.dependencies]\nrenamed={path='engine',package='engine'}\n"),
            ("/w/client/Cargo.toml", "[package]\nname='client'\n[dependencies]\nrenamed.workspace=true\n"),
            ("/w/engine/Cargo.toml", "[package]\nname='engine'\n"),
            ("/w/engine/src/lib.rs", "pub fn work() {}"),
            ("/w/client/src/main.rs", "fn main(){ renamed::work(); }"),
        ]);
        assert_eq!(
            paths(&g, "/w/client/src/main.rs", "renamed::work"),
            ["/w/engine/src/lib.rs"]
        );
    }
    #[test]
    fn cargo_follows_declared_modules_and_path_attributes_only() {
        let g = graph(&[
            ("/w/Cargo.toml", "[package]\nname='app'\n"),
            ("/w/src/lib.rs", "#[path = \"custom.rs\"] mod api; mod inline { pub fn work() {} } fn run(){ crate::api::work(); crate::inline::work(); crate::orphan::work(); }"),
            ("/w/src/custom.rs", "pub fn work() {}"),
            ("/w/src/orphan.rs", "pub fn work() {}"),
        ]);
        assert_eq!(
            paths(&g, "/w/src/lib.rs", "crate::api::work"),
            ["/w/src/custom.rs"]
        );
        assert_eq!(
            paths(&g, "/w/src/lib.rs", "crate::inline::work"),
            ["/w/src/lib.rs"]
        );
        assert!(paths(&g, "/w/src/lib.rs", "crate::orphan::work").is_empty());
    }
    #[test]
    fn cargo_missing_invalid_and_conditional_boundaries_do_not_fall_back() {
        for manifest in ["[package]\nname='app'", "[package", ""] {
            let g = graph(&[
                ("/w/Cargo.toml", manifest),
                (
                    "/w/src/lib.rs",
                    "#[cfg(feature=\"x\")] mod api; fn run(){crate::api::work();}",
                ),
                ("/w/src/api.rs", "pub fn work() {}"),
                ("/w/src/main.rs", "fn main(){crate::work();}"),
            ]);
            assert!(paths(&g, "/w/src/lib.rs", "crate::api::work").is_empty());
            assert!(paths(&g, "/w/src/main.rs", "crate::work").is_empty());
        }
    }
    #[test]
    fn cargo_module_ambiguity_and_required_features_are_not_resolved() {
        let g = graph(&[
            ("/w/Cargo.toml", "[package]\nname='app'\n[[bin]]\nname='gated'\npath='gated.rs'\nrequired-features=['x']"),
            ("/w/src/lib.rs", "mod api; fn run(){crate::api::work();}"),
            ("/w/src/api.rs", "pub fn work() {}"),
            ("/w/src/api/mod.rs", "pub fn work() {}"),
            ("/w/gated.rs", "fn work() {} fn main(){crate::work();}"),
        ]);
        assert!(paths(&g, "/w/src/lib.rs", "crate::api::work").is_empty());
        assert!(paths(&g, "/w/gated.rs", "crate::work").is_empty());
    }
    #[test]
    fn cargo_custom_crate_name_and_nested_workspace_boundary() {
        let g = graph(&[
            ("/w/Cargo.toml", "[workspace]\n[workspace.dependencies]\nengine={path='engine'}"),
            ("/w/engine/Cargo.toml", "[package]\nname='engine'"),
            ("/w/engine/src/lib.rs", "pub fn work() {}"),
            ("/w/nested/Cargo.toml", "[workspace]\nmembers=['app']"),
            ("/w/nested/app/Cargo.toml", "[package]\nname='app'\n[lib]\nname='custom'\n[dependencies]\nengine.workspace=true"),
            ("/w/nested/app/src/main.rs", "fn main(){custom::work(); app::work(); engine::work();}"),
            ("/w/nested/app/src/lib.rs", "pub fn work() {}"),
        ]);
        assert_eq!(
            paths(&g, "/w/nested/app/src/main.rs", "custom::work"),
            ["/w/nested/app/src/lib.rs"]
        );
        assert!(paths(&g, "/w/nested/app/src/main.rs", "app::work").is_empty());
        assert!(paths(&g, "/w/nested/app/src/main.rs", "engine::work").is_empty());
    }
    #[test]
    fn cargo_manifest_only_updates_rebind_and_stale_boundary_blocks_links() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("cargo.duckdb")).unwrap();
        let document =
            |id: &str, path: &str, source: &str, revision: i64| crate::models::Document {
                id: id.into(),
                uri: format!("file://{path}"),
                source_file: Some(path.into()),
                content: source.into(),
                wing: Some("fixture".into()),
                revision,
                ..Default::default()
            };
        let original = "[package]\nname='app'\n[dependencies]\nengine={path='../one'}";
        for d in [
            document("manifest", "/w/app/Cargo.toml", original, 1),
            document(
                "main",
                "/w/app/src/main.rs",
                "fn main(){engine::work();}",
                1,
            ),
            document(
                "one-man",
                "/w/one/Cargo.toml",
                "[package]\nname='engine'",
                1,
            ),
            document("one-lib", "/w/one/src/lib.rs", "pub fn work(){}", 1),
            document(
                "two-man",
                "/w/two/Cargo.toml",
                "[package]\nname='engine'",
                1,
            ),
            document("two-lib", "/w/two/src/lib.rs", "pub fn work(){}", 1),
        ] {
            store.upsert_document(&d).unwrap();
        }
        let first = store.index_code().unwrap();
        assert_eq!(
            paths(
                &store.code_snapshot().unwrap(),
                "/w/app/src/main.rs",
                "engine::work"
            ),
            ["/w/one/src/lib.rs"]
        );
        let noop = store.index_code().unwrap();
        assert_eq!(noop.unchanged, 6);
        assert!(!noop.revision_created);
        store
            .upsert_document(&document(
                "manifest",
                "/w/app/Cargo.toml",
                &original.replace("../one", "../two"),
                2,
            ))
            .unwrap();
        let stale = store.code_snapshot().unwrap();
        assert!(paths(&stale, "/w/app/src/main.rs", "engine::work").is_empty());
        assert_eq!(stale.stale_documents, ["manifest"]);
        assert!(stale
            .binding_diagnostics
            .iter()
            .any(|v| v["code"] == "RUST_MANIFEST_BOUNDARY_UNVERIFIED"));
        let changed = store.index_code().unwrap();
        assert_eq!(changed.indexed, 1);
        assert_eq!(changed.unchanged, 5);
        assert_ne!(changed.index_revision, first.index_revision);
        assert_eq!(
            paths(
                &store.code_snapshot().unwrap(),
                "/w/app/src/main.rs",
                "engine::work"
            ),
            ["/w/two/src/lib.rs"]
        );
        let diff = store
            .code_index_diff(&history::DiffQuery {
                before: first.index_revision.unwrap(),
                after: changed.index_revision,
                project: Some("fixture".into()),
                limit: Some(100),
            })
            .unwrap();
        assert_eq!(
            diff["edges_added"],
            serde_json::json!(["main::main::function_item -> two-lib::work::function_item"])
        );
        assert_eq!(
            diff["edges_removed"],
            serde_json::json!(["main::main::function_item -> one-lib::work::function_item"])
        );
        assert_eq!(diff["symbol_changes_total"], 0);
    }
}
