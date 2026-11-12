//! Content-addressed bindings over captured dependency scopes, including absence
//! guards. Previous successful edges are never used as the dependency inventory.
use super::*;
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IndexOptions {
    /// Recompute all bindings, including unchanged revision inputs, for qualification.
    #[serde(default)]
    pub force_full_resolution: bool,
}
#[derive(Serialize, Deserialize)]
struct Entry {
    key: String,
    edges: Vec<Value>,
    checksum: String,
}
#[derive(Serialize, Deserialize)]
struct Cache {
    contract_version: u32,
    format_version: u32,
    entries: BTreeMap<String, Entry>,
}
#[derive(Deserialize)]
struct EdgeWire {
    #[serde(default)]
    context_evidence: Option<BindingContexts>,
    reference: Reference,
    candidates: Vec<String>,
    resolution: String,
    provenance: String,
    resolution_basis: String,
}
fn valid_contexts(
    edge: &EdgeWire,
    body_site: bool,
    allowed: &super::php_names::ContextIds<'_>,
) -> bool {
    let trait_target = edge
        .candidates
        .iter()
        .any(|id| allowed.trait_methods.contains(id.as_str()));
    let required = body_site || trait_target;
    let Some(evidence) = &edge.context_evidence else {
        return !required;
    };
    let body = evidence.kind == "php_captured_trait_consumers" && body_site;
    let entry = evidence.kind == "php_captured_trait_entry" && !body_site && trait_target;
    if !required
        || !(body || entry)
        || evidence.runtime_contexts_complete
        || evidence.context_correlation_proven
        || evidence.contexts.len() > 128
    {
        return false;
    }
    if entry
        && (evidence.contexts.len() != 1 || !evidence.enumeration_complete || evidence.truncated)
    {
        return false;
    }
    let mut owners = HashSet::new();
    let mut union = std::collections::BTreeSet::new();
    for context in &evidence.contexts {
        if !allowed.owners.contains(context.owner.as_str())
            || !owners.insert(&context.owner)
            || context
                .candidates
                .iter()
                .any(|id| !allowed.methods.contains(id.as_str()))
        {
            return false;
        }
        let trait_targets = context
            .candidates
            .iter()
            .filter(|id| allowed.trait_methods.contains(id.as_str()))
            .count();
        if let Some(target) = &context.target_owner {
            if !allowed.owners.contains(target.as_str())
                || trait_targets == 0
                || trait_targets != context.candidates.len()
                || entry && target != &context.owner
            {
                return false;
            }
        } else if trait_targets > 0 || entry {
            return false;
        }
        if context.resolution
            != match context.candidates.len() {
                0 => "unresolved",
                1 => "resolved_static",
                _ => "ambiguous",
            }
        {
            return false;
        }
        union.extend(context.candidates.iter().cloned());
    }
    let expected = if union.is_empty() {
        "unresolved"
    } else if union.len() > 1 || evidence.contexts.len() != 1 || !evidence.enumeration_complete {
        "ambiguous"
    } else {
        "resolved_static"
    };
    edge.resolution == expected && union.into_iter().collect::<Vec<_>>() == edge.candidates
}
pub(super) struct Resolved {
    pub edges: Vec<CallEdge>,
    pub cache: Value,
    pub stats: rag_mcp_core::IndexResolutionStats,
}
fn document_value(doc: &IndexedDocument) -> Result<Value> {
    let mut value = serde_json::to_value(doc)?;
    for key in [
        "blocked_members",
        "test_definitions",
        "ignored_test_definitions",
    ] {
        if let Some(values) = value.get_mut(key).and_then(Value::as_array_mut) {
            values.sort_by_key(Value::to_string);
        }
    }
    Ok(value)
}
fn interface(doc: &IndexedDocument) -> Result<String> {
    let mut value = document_value(doc)?;
    if doc.path.ends_with(".rs") {
        let object = value.as_object_mut().unwrap();
        for key in [
            "hash",
            "revision",
            "references",
            "policies",
            "sql",
            "styles",
            "routes",
            "endpoints",
            "file_imports",
            "file_resources",
            "resource_base_unresolved",
            "test_definitions",
            "ignored_test_definitions",
        ] {
            object.remove(key);
        }
        for symbol in object
            .get_mut("symbols")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            let symbol = symbol.as_object_mut().unwrap();
            for key in [
                "revision",
                "content_hash",
                "documentation",
                "documentation_examined",
            ] {
                symbol.remove(key);
            }
        }
    }
    Ok(content_hash(&serde_json::to_string(&value)?))
}
pub(super) fn resolve(
    symbols: &[Symbol],
    docs: &HashMap<String, IndexedDocument>,
    previous: Option<&Value>,
    force: bool,
) -> Result<Resolved> {
    let started = std::time::Instant::now();
    let cache = previous
        .cloned()
        .and_then(|v| serde_json::from_value::<Cache>(v).ok())
        .filter(|c| c.contract_version == 1 && c.format_version == INDEX_FORMAT_VERSION);
    let mut interfaces = BTreeMap::new();
    for (id, doc) in docs {
        interfaces.insert(id.clone(), interface(doc)?);
    }
    let known: HashSet<_> = symbols.iter().map(|s| s.id.as_str()).collect();
    let mut entries = BTreeMap::new();
    let mut misses = HashSet::new();
    let mut edges = Vec::new();
    let mut scoped = 0;
    let mut project_fallback = 0;
    let mut invalid = 0;
    let mut dependency_inputs = 0;
    for (id, doc) in docs {
        let mixed = docs.values().any(|d| {
            d.project == doc.project && !d.path.ends_with(".rs") && !rust_cargo::manifest(&d.path)
        });
        let scoped_ids = if doc.path.ends_with(".rs") && !mixed {
            rust_cargo::resolution_documents(docs, doc)
        } else {
            None
        };
        let scope = if doc.references.is_empty() {
            std::collections::BTreeSet::new()
        } else if let Some(ids) = scoped_ids {
            scoped += 1;
            ids
        } else {
            project_fallback += 1;
            docs.iter()
                .filter(|(_, d)| d.project == doc.project)
                .map(|(id, _)| id.clone())
                .collect()
        };
        dependency_inputs += scope.len();
        let key = content_hash(&serde_json::to_string(&(
            INDEX_FORMAT_VERSION,
            document_value(doc)?,
            scope
                .iter()
                .map(|id| (id, &interfaces[id]))
                .collect::<Vec<_>>(),
        ))?);
        let old = if force {
            None
        } else {
            cache
                .as_ref()
                .and_then(|c| c.entries.get(id))
                .filter(|e| e.key == key)
        };
        let reused = old.and_then(|e| {
            if e.edges.len() != doc.references.len()
                || content_hash(&serde_json::to_string(&e.edges).ok()?) != e.checksum
            {
                return None;
            }
            let mut result = Vec::new();
            let context_sites = super::php_names::context_sites(doc);
            let allowed = if doc.php_names.is_some() {
                super::php_names::context_ids(docs, doc)
            } else {
                super::php_names::ContextIds::default()
            };
            for (raw, reference) in e.edges.iter().zip(&doc.references) {
                let edge: EdgeWire = serde_json::from_value(raw.clone()).ok()?;
                if !valid_contexts(
                    &edge,
                    context_sites.contains(&(
                        reference.start_byte,
                        reference.is_call,
                        reference.expression.clone(),
                    )),
                    &allowed,
                ) || edge.provenance != "derived"
                    || serde_json::to_value(&edge.reference).ok()?
                        != serde_json::to_value(reference).ok()?
                    || edge
                        .candidates
                        .iter()
                        .any(|id| !known.contains(id.as_str()))
                {
                    return None;
                }
                result.push(CallEdge {
                    context_evidence: edge.context_evidence,
                    reference: edge.reference,
                    candidates: edge.candidates,
                    resolution: edge.resolution,
                    provenance: "derived",
                    resolution_basis: edge.resolution_basis,
                });
            }
            Some(result)
        });
        let values = if let Some(reused) = reused {
            let values = old.unwrap().edges.clone();
            edges.extend(reused);
            values
        } else {
            invalid += usize::from(old.is_some());
            misses.insert(id.clone());
            Vec::new()
        };
        entries.insert(
            id.clone(),
            Entry {
                key,
                edges: values,
                checksum: String::new(),
            },
        );
    }
    let reference_sites_reused = edges.len();
    let fresh = resolve::references_selected(symbols, docs, Some(&misses));
    let reference_sites_resolved = fresh.len();
    for edge in fresh {
        if let Some(entry) = entries.get_mut(&edge.reference.document_id) {
            entry.edges.push(serde_json::to_value(&edge)?);
        }
        edges.push(edge);
    }
    edges.sort_by(|a, b| {
        (
            &a.reference.document_id,
            a.reference.start_byte,
            &a.reference.expression,
            a.reference.is_call,
        )
            .cmp(&(
                &b.reference.document_id,
                b.reference.start_byte,
                &b.reference.expression,
                b.reference.is_call,
            ))
    });
    for entry in entries.values_mut() {
        entry.checksum = content_hash(&serde_json::to_string(&entry.edges)?);
    }
    let stats = rag_mcp_core::IndexResolutionStats {
        contract_version: 1,
        documents_total: docs.len(),
        documents_resolved: misses.len(),
        documents_reused: docs.len() - misses.len(),
        reference_sites_resolved,
        reference_sites_reused,
        rust_package_scopes: scoped,
        project_fallback_scopes: project_fallback,
        empty_reference_documents: docs.values().filter(|d| d.references.is_empty()).count(),
        dependency_inputs_hashed: dependency_inputs,
        invalid_cache_entries: invalid,
        forced_full: force,
        cache_available: cache.is_some(),
        no_resolution_required: false,
        resolution_ms: started.elapsed().as_secs_f64() * 1000.0,
        equivalent_to_existing: None,
    };
    Ok(Resolved {
        edges,
        cache: serde_json::to_value(Cache {
            contract_version: 1,
            format_version: INDEX_FORMAT_VERSION,
            entries,
        })?,
        stats,
    })
}
pub(super) fn previous(
    r: &impl crate::db::lin_engine::LinReader,
    revision: Option<i64>,
) -> Result<Option<Value>> {
    let Some(revision) = revision else {
        return Ok(None);
    };
    let raw = r
        .get_by_id("code_index_revisions", &revision.to_string())?
        .and_then(|row| crate::db::lin_engine::cell_str(&row, "graph_json").map(str::to_string));
    Ok(raw
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.get("resolution_cache").cloned()))
}
pub(super) fn noop_stats() -> rag_mcp_core::IndexResolutionStats {
    rag_mcp_core::IndexResolutionStats {
        contract_version: 1,
        no_resolution_required: true,
        ..Default::default()
    }
}
pub(super) fn peak_rss_bytes() -> Option<u64> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
            return None;
        }
        let usage = unsafe { usage.assume_init() };
        let value = u64::try_from(usage.ru_maxrss).ok()?;
        #[cfg(target_os = "linux")]
        {
            value.checked_mul(1024)
        }
        #[cfg(target_os = "macos")]
        {
            Some(value)
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sources() -> BTreeMap<String, (String, String)> {
        [
        ("workspace","/w/Cargo.toml","[workspace]\nmembers=['core','app','noise']\n"),
        ("core-man","/w/core/Cargo.toml","[package]\nname='p-core'\nversion='0.1.0'\n"),
        ("app-man","/w/app/Cargo.toml","[package]\nname='p-app'\nversion='0.1.0'\n[dependencies]\ncore_alias={package='p-core',path='../core'}\n"),
        ("noise-man","/w/noise/Cargo.toml","[package]\nname='p-noise'\nversion='0.1.0'\n"),
        ("core","/w/core/src/lib.rs","pub struct A;pub struct B;impl A{pub fn work(&self){}}impl B{pub fn work(&self){}}pub fn make()->A{A}pub fn unused()->i32{1}"),
        ("app","/w/app/src/lib.rs","pub fn entry(){let value=core_alias::make();value.work();}"),
        ("noise","/w/noise/src/lib.rs","pub struct A;impl A{pub fn work(&self){}}pub fn make()->A{A}")
    ].into_iter().map(|(id,path,text)|(id.into(),(path.into(),text.into()))).collect()
    }
    fn docs(
        sources: &BTreeMap<String, (String, String)>,
        revision: i64,
    ) -> (HashMap<String, IndexedDocument>, Vec<Symbol>) {
        let mut docs: HashMap<_, _> = sources
            .iter()
            .map(|(id, (path, text))| {
                let mut doc =
                    parse(id, path, text, if id == "core" { revision } else { 1 }).unwrap();
                doc.project = Some("fixture".into());
                (id.clone(), doc)
            })
            .collect();
        rust_cfg::apply(&mut docs);
        let symbols = docs.values().flat_map(|d| d.symbols.clone()).collect();
        (docs, symbols)
    }
    fn same(a: &[CallEdge], b: &[CallEdge]) {
        assert_eq!(
            serde_json::to_value(a).unwrap(),
            serde_json::to_value(b).unwrap()
        );
    }
    #[test]
    fn package_body_reuses_clients_and_signature_invalidates_dependents_only() {
        let mut source = sources();
        let (initial, symbols) = docs(&source, 1);
        let cold = resolve(&symbols, &initial, None, false).unwrap();
        assert_eq!(cold.stats.documents_resolved, 7);
        let (reloaded, symbols) = docs(&source, 1);
        let reused = resolve(&symbols, &reloaded, Some(&cold.cache), false).unwrap();
        assert_eq!(reused.stats.documents_reused, 7);
        assert!(reused.stats.reference_sites_reused > 0);
        same(&cold.edges, &reused.edges);
        source.get_mut("core").unwrap().1 = source["core"].1.replace("{1}", "{2}");
        let (body, symbols) = docs(&source, 2);
        let changed = resolve(&symbols, &body, Some(&cold.cache), false).unwrap();
        assert_eq!(changed.stats.documents_resolved, 1, "{:?}", changed.stats);
        assert!(changed.stats.reference_sites_reused > 0);
        same(
            &changed.edges,
            &resolve(&symbols, &body, None, true).unwrap().edges,
        );
        source.get_mut("core").unwrap().1 =
            source["core"].1.replace("make()->A{A}", "make()->B{B}");
        let (signature, symbols) = docs(&source, 3);
        let changed = resolve(&symbols, &signature, Some(&changed.cache), false).unwrap();
        assert!(changed.stats.documents_reused >= 1, "{:?}", changed.stats);
        assert!(changed.stats.documents_resolved >= 2);
        same(
            &changed.edges,
            &resolve(&symbols, &signature, None, true).unwrap().edges,
        );
        let work = changed
            .edges
            .iter()
            .find(|e| e.reference.document_id == "app" && e.reference.name == "work")
            .unwrap();
        assert_eq!(work.candidates.len(), 1);
        assert!(
            symbols
                .iter()
                .any(|s| s.id == work.candidates[0] && s.qualified_name == "B::work")
        );
    }
    #[test]
    fn new_missing_and_manifest_providers_invalidate_without_prior_edges() {
        let mut source = sources();
        source.get_mut("app").unwrap().1 = "pub fn entry(){core_alias::missing();}".into();
        let (initial, symbols) = docs(&source, 1);
        let cold = resolve(&symbols, &initial, None, false).unwrap();
        assert!(
            cold.edges
                .iter()
                .find(|e| e.reference.document_id == "app" && e.reference.is_call)
                .unwrap()
                .candidates
                .is_empty()
        );
        source
            .get_mut("core")
            .unwrap()
            .1
            .push_str("pub fn missing(){}");
        let (added, symbols) = docs(&source, 2);
        let added = resolve(&symbols, &added, Some(&cold.cache), false).unwrap();
        same(
            &added.edges,
            &resolve(&symbols, &docs(&source, 2).0, None, true)
                .unwrap()
                .edges,
        );
        assert_eq!(
            added
                .edges
                .iter()
                .find(|e| e.reference.document_id == "app" && e.reference.is_call)
                .unwrap()
                .candidates
                .len(),
            1
        );
        source.remove("core");
        let (removed, symbols) = docs(&source, 3);
        let removed = resolve(&symbols, &removed, Some(&added.cache), false).unwrap();
        same(
            &removed.edges,
            &resolve(&symbols, &docs(&source, 3).0, None, true)
                .unwrap()
                .edges,
        );
        assert!(
            removed
                .edges
                .iter()
                .find(|e| e.reference.document_id == "app" && e.reference.is_call)
                .unwrap()
                .candidates
                .is_empty()
        );
        source.get_mut("app-man").unwrap().1 = source["app-man"]
            .1
            .replace("p-core", "p-noise")
            .replace("../core", "../noise");
        source.get_mut("app").unwrap().1 =
            "pub fn entry(){let value=core_alias::make();value.work();}".into();
        let (manifest, symbols) = docs(&source, 4);
        let changed = resolve(&symbols, &manifest, Some(&removed.cache), false).unwrap();
        same(
            &changed.edges,
            &resolve(&symbols, &manifest, None, true).unwrap().edges,
        );
        let edge = changed
            .edges
            .iter()
            .find(|e| e.reference.name == "work")
            .unwrap();
        assert_eq!(edge.candidates.len(), 1);
        assert!(edge.candidates.iter().all(|id| id.starts_with("noise:")));
    }
    #[test]
    fn corrupt_cache_and_mixed_language_inputs_fall_back_to_full_facts() {
        let mut source = sources();
        let (initial, symbols) = docs(&source, 1);
        let cold = resolve(&symbols, &initial, None, false).unwrap();
        let mut cache = cold.cache.clone();
        cache["entries"]["app"]["checksum"] = "bad".into();
        let repaired = resolve(&symbols, &initial, Some(&cache), false).unwrap();
        assert_eq!(repaired.stats.invalid_cache_entries, 1);
        same(&cold.edges, &repaired.edges);
        let mut cache = cold.cache;
        cache["format_version"] = 0.into();
        assert!(
            !resolve(&symbols, &initial, Some(&cache), false)
                .unwrap()
                .stats
                .cache_available
        );
        source.insert(
            "python".into(),
            ("/w/helper.py".into(), "def helper(): return 1\n".into()),
        );
        let (mixed, symbols) = docs(&source, 1);
        let first = resolve(&symbols, &mixed, None, false).unwrap();
        assert_eq!(
            first.stats.project_fallback_scopes,
            mixed.values().filter(|d| !d.references.is_empty()).count()
        );
        source.get_mut("python").unwrap().1 = "def helper(): return 2\n".into();
        let (mixed, symbols) = docs(&source, 1);
        let next = resolve(&symbols, &mixed, Some(&first.cache), false).unwrap();
        assert_eq!(next.stats.reference_sites_reused, 0);
        assert_eq!(
            next.stats.documents_resolved,
            mixed
                .iter()
                .filter(|(id, d)| !d.references.is_empty() || *id == "python")
                .count()
        );
        same(
            &next.edges,
            &resolve(&symbols, &mixed, None, true).unwrap().edges,
        );
    }
    #[test]
    fn committed_cache_roundtrip_forced_equivalence_and_completion_are_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("cache.duckdb")).unwrap();
        let mut source = sources();
        for (id, (path, text)) in &source {
            store
                .upsert_document(&crate::models::Document {
                    id: id.clone(),
                    uri: format!("file://{path}"),
                    source_file: Some(path.clone()),
                    content: text.clone(),
                    wing: Some("fixture".into()),
                    revision: 1,
                    ..Default::default()
                })
                .unwrap();
        }
        let first = store.index_code().unwrap();
        assert_eq!(first.resolution.as_ref().unwrap().documents_resolved, 7);
        assert!(first.completion.as_ref().unwrap().commit_included);
        assert!(
            first.completion.as_ref().unwrap().native_total_ms
                >= first
                    .stage_timings
                    .as_ref()
                    .unwrap()
                    .before_metrics_write_ms
        );
        let noop = store.index_code().unwrap();
        assert!(noop.resolution.as_ref().unwrap().no_resolution_required);
        assert!(!noop.revision_created);
        source.get_mut("core").unwrap().1 = source["core"].1.replace("{1}", "{2}");
        let (path, text) = &source["core"];
        store
            .upsert_document(&crate::models::Document {
                id: "core".into(),
                uri: format!("file://{path}"),
                source_file: Some(path.clone()),
                content: text.clone(),
                wing: Some("fixture".into()),
                revision: 2,
                ..Default::default()
            })
            .unwrap();
        let incremental = store.index_code().unwrap();
        assert_eq!(
            incremental.resolution.as_ref().unwrap().documents_resolved,
            1
        );
        let full = store
            .index_code_with_options(&IndexOptions {
                force_full_resolution: true,
            })
            .unwrap();
        assert_eq!(full.index_revision, incremental.index_revision);
        assert!(!full.revision_created);
        assert_eq!(
            full.resolution.as_ref().unwrap().equivalent_to_existing,
            Some(true)
        );
        assert_eq!(full.resolution.as_ref().unwrap().documents_resolved, 7);
    }
}
