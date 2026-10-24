//! Stale-page detection against raw parents and LLM refresh.

use super::*;

/// Most graph neighbours read when collecting a page's review dependencies;
/// reaching it means the set may be incomplete, which is an error.
const REVIEW_NEIGHBOR_LIMIT: u32 = 10_000;

/// One wiki page whose recorded source changed, disappeared, or is newer than
/// the page when only legacy timestamp provenance is available.
#[derive(Debug, Clone, Serialize)]
pub struct StaleWikiItem {
    pub wiki_id: String,
    pub wiki_uri: String,
    pub wiki_title: String,
    pub wiki_kind: String,
    pub wiki_updated_at: String,
    pub raw_id: String,
    pub raw_uri: String,
    pub raw_title: String,
    /// None for a deleted/unavailable source; its recorded URI remains present.
    pub raw_updated_at: Option<String>,
    /// `source_version` | `source_missing` | `graph_related` | `metadata` |
    /// `citation` | `content_source`. Version evidence overrides timestamps.
    pub link_kind: String,
}

/// Unique raw source that should be recompiled, with linked stale wiki ids.
#[derive(Debug, Clone, Serialize)]
pub struct RawRefreshTarget {
    pub raw_id: String,
    pub raw_uri: String,
    pub raw_title: String,
    pub stale_wiki_ids: Vec<String>,
}

/// Per-source recompile failure (apply path).
#[derive(Debug, Clone, Serialize)]
pub struct RefreshError {
    pub raw_id: String,
    pub message: String,
}

/// Result of [`find_stale_wiki`] / [`refresh_stale_wiki`].
#[derive(Debug, Clone, Serialize)]
pub struct RefreshStaleWikiResult {
    pub stale_count: usize,
    pub stale: Vec<StaleWikiItem>,
    /// Unique raw parents (agent mark-list / recompile queue), order stable by raw_uri.
    pub raw_sources: Vec<RawRefreshTarget>,
    /// Compile results when apply ran with an LLM (empty on list-only / dry_run).
    pub recompiled: Vec<CompileApplyResult>,
    pub errors: Vec<RefreshError>,
    pub dry_run: bool,
    /// True when at least one `compile_source` write was attempted.
    pub applied: bool,
    /// True when more unique raws exist than `max_docs` budget.
    pub capped: bool,
    pub max_docs: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Find wiki pages whose sources changed or disappeared.
///
/// A valid complete `source_versions` array defines the authoritative parent
/// set and uses body hashes instead of timestamps. This catches edits during
/// generation and ignores obsolete links to sources no longer used by the page.
///
/// Without a valid snapshot, legacy links are discovered from:
/// 1. graph `related` edges between raw and wiki document nodes (`compile_source`)
/// 2. wiki metadata `source_id` / `source_uri`, or arrays `source_ids` /
///    `source_uris` / `consolidated_from`
/// 3. wiki metadata `citations[].document_id` / `citations[].uri`
/// 4. body lines `source: <uri-or-id>`
pub fn find_stale_wiki(store: &Store) -> Result<Vec<StaleWikiItem>> {
    // The raw corpus is read without bodies: staleness needs ids, uris and
    // timestamps; only sources cited by a `source_versions` snapshot are hashed,
    // and their bodies are fetched in one batch below.
    let raws = store.list_documents_by_layer_without_body(LAYER_RAW)?;
    let (raw_by_id, raw_by_uri) = raw_lookups(&raws);

    // (wiki_id, raw_id) -> item (prefer more specific link_kind if already present)
    let mut pairs: std::collections::BTreeMap<(String, String), StaleWikiItem> =
        std::collections::BTreeMap::new();

    // 1) Graph `related` edges between a raw source and a wiki page, either
    // direction: one edge query instead of a neighborhood walk per raw.
    let wiki_docs = store.list_documents_by_layer(LAYER_WIKI)?;
    let wiki_by_id: HashMap<&str, &Document> = wiki_docs
        .iter()
        .map(|wiki| (wiki.id.as_str(), wiki))
        .collect();
    for (a, b) in store.document_pairs_by_relation("related")? {
        for (raw_id, wiki_id) in [(&a, &b), (&b, &a)] {
            let (Some(raw), Some(wiki)) = (
                raw_by_id.get(raw_id.as_str()),
                wiki_by_id.get(wiki_id.as_str()),
            ) else {
                continue;
            };
            if wiki.updated_at < raw.updated_at && wiki_source_versions(wiki).is_none() {
                insert_stale_pair(&mut pairs, wiki, raw, "graph_related");
            }
        }
    }

    // Snapshot parents are authoritative, including an explicitly empty set.
    // Legacy metadata/content references may survive a later compilation and
    // must not add parents outside that snapshot.
    let cited_raw_ids: Vec<&str> = {
        let mut ids: Vec<&str> = wiki_docs
            .iter()
            .filter_map(wiki_source_versions)
            .flatten()
            .filter_map(|version| {
                raw_by_id
                    .get_key_value(version.document_id.as_str())
                    .map(|(id, _)| *id)
            })
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    };
    let mut source_hashes: HashMap<String, String> = store
        .document_contents(&cited_raw_ids)?
        .into_iter()
        .map(|(id, content)| (id, content_hash(&content)))
        .collect();
    for wiki in &wiki_docs {
        if let Some(versions) = wiki_source_versions(wiki) {
            for version in versions {
                // Sources are almost always raw or wiki pages already in memory by
                // id; a miss falls back to the store (id, then uri), as before.
                let in_memory = raw_by_id
                    .get(version.document_id.as_str())
                    .or_else(|| wiki_by_id.get(version.document_id.as_str()))
                    .copied();
                let fetched;
                let source = match in_memory {
                    Some(source) => Some(source),
                    None => {
                        fetched = store.find_by_id_or_uri(&version.document_id, &version.uri)?;
                        fetched.as_ref()
                    }
                };
                if let Some(source) = source {
                    // A source cited by many pages is hashed once.
                    let hash = source_hashes
                        .entry(source.id.clone())
                        .or_insert_with(|| content_hash(&source.content));
                    if *hash != version.content_hash {
                        insert_stale_pair(&mut pairs, wiki, source, "source_version");
                    }
                } else {
                    pairs.insert(
                        (wiki.id.clone(), version.document_id.clone()),
                        StaleWikiItem {
                            wiki_id: wiki.id.clone(),
                            wiki_uri: wiki.uri.clone(),
                            wiki_title: wiki.title.clone(),
                            wiki_kind: wiki.kind.to_string(),
                            wiki_updated_at: wiki.updated_at.to_rfc3339(),
                            raw_id: version.document_id,
                            raw_uri: version.uri.clone(),
                            raw_title: version.uri,
                            raw_updated_at: None,
                            link_kind: "source_missing".into(),
                        },
                    );
                }
            }
        } else {
            // 2–4) Metadata + content markers for legacy pages.
            for (raw, link_kind) in linked_raws_for_wiki(store, wiki, &raw_by_id, &raw_by_uri)? {
                if wiki.updated_at < raw.updated_at {
                    insert_stale_pair(&mut pairs, wiki, &raw, link_kind);
                }
            }
        }
    }

    let mut out: Vec<StaleWikiItem> = pairs.into_values().collect();
    out.sort_by(|a, b| {
        b.raw_updated_at
            .cmp(&a.raw_updated_at)
            .then_with(|| a.wiki_uri.cmp(&b.wiki_uri))
            .then_with(|| a.raw_uri.cmp(&b.raw_uri))
    });
    Ok(out)
}

/// Resolve the entire recorded parent set for a review snapshot.
pub(crate) fn review_parent_ids(store: &Store, page: &Document) -> Result<Vec<(String, String)>> {
    if let Some(versions) = wiki_source_versions(page) {
        let mut refs = Vec::new();
        for v in versions {
            let source = store
                .get_document(&v.document_id)?
                .or(store.find_by_uri(&v.uri)?);
            refs.push(
                source
                    .map(|d| (d.id, d.uri))
                    .unwrap_or((v.document_id, v.uri)),
            );
        }
        refs.sort();
        refs.dedup();
        return Ok(refs);
    }
    // One page cites few sources: resolve each reference on demand
    // (`try_push_raw` falls back to id, then uri, lookups) instead of loading
    // every raw document with its body.
    let (ids, uris) = (RawLookup::new(), RawLookup::new());
    let mut result: Vec<(String, String)> = linked_raws_for_wiki(store, page, &ids, &uris)?
        .into_iter()
        .map(|(d, _)| (d.id.clone(), d.uri.clone()))
        .collect();
    if let Some(node) = store.find_node_by_document_id(&page.id)? {
        let view = store.neighbors(&node.id, 1, REVIEW_NEIGHBOR_LIMIT)?;
        if view.nodes.len() >= REVIEW_NEIGHBOR_LIMIT as usize {
            return Err(AppError::config(
                "review dependency set exceeds graph limit",
            ));
        }
        let node_documents: HashMap<&str, &str> = view
            .nodes
            .iter()
            .filter_map(|n| n.document_id.as_deref().map(|doc| (n.id.as_str(), doc)))
            .collect();
        for edge in &view.edges {
            if edge.rel_type != "related" {
                continue;
            }
            let other = if edge.source_id == node.id {
                &edge.target_id
            } else {
                &edge.source_id
            };
            if let Some(id) = node_documents.get(other.as_str()) {
                if let Some((id, uri)) = store.document_id_and_uri(id)? {
                    result.push((id, uri));
                }
            }
        }
    }
    result.sort();
    result.dedup();
    Ok(result)
}

/// List stale wiki↔raw pairs; optionally recompile unique raw parents via LLM.
///
/// - `dry_run=true` (default for callers): list only (agent mark-list).
/// - `dry_run=false` + `llm=Some`: re-run [`compile_source`] per unique raw (capped by
///   `max_docs` / `config.maint_max_docs`).
/// - `dry_run=false` + `llm=None`: same list + notes; no error (agent recompiles).
pub async fn refresh_stale_wiki(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    llm: Option<&dyn crate::llm::ChatCompleter>,
    dry_run: bool,
    max_docs: Option<usize>,
    agent: Option<&str>,
) -> Result<RefreshStaleWikiResult> {
    let max_docs = max_docs
        .unwrap_or(config.maint_max_docs)
        .max(1)
        .min(config.maint_max_docs.max(1));

    let stale = crate::util::blocking_section(|| find_stale_wiki(store))?;
    let mut raw_sources = group_raw_targets(&stale);
    let total_raws = raw_sources.len();
    let capped = total_raws > max_docs;
    if capped {
        raw_sources.truncate(max_docs);
    }

    let mut recompiled = Vec::new();
    let mut errors = Vec::new();
    let mut applied = false;

    let mut notes = if dry_run {
        Some(if stale.is_empty() {
            "no stale wiki pages".into()
        } else {
            format!(
                "dry_run: {} stale page(s) over {} unique raw source(s); recompile with dry_run=false + LLM, or call compile_source per raw_sources entry",
                stale.len(),
                total_raws
            )
        })
    } else if let Some(client) = llm {
        applied = !raw_sources.is_empty();
        for target in &raw_sources {
            match compile_source(
                store,
                embedder,
                config,
                client,
                &target.raw_id,
                false,
                agent,
            )
            .await
            {
                Ok(res) => recompiled.push(res),
                Err(e) => errors.push(RefreshError {
                    raw_id: target.raw_id.clone(),
                    message: e.to_string(),
                }),
            }
        }
        Some(format!(
            "applied recompile for {} raw source(s); {} ok, {} errors{}",
            raw_sources.len(),
            recompiled.len(),
            errors.len(),
            if capped {
                format!(" (capped at max_docs={max_docs}; {total_raws} total)")
            } else {
                String::new()
            }
        ))
    } else {
        Some(format!(
            "listed {} stale page(s) / {} unique raw(s); no ChatClient - agent should call compile_source on each raw_sources entry",
            stale.len(),
            total_raws
        ))
    };

    let missing_sources = stale
        .iter()
        .filter(|item| item.link_kind == "source_missing")
        .count();
    if missing_sources > 0 {
        let note = format!(
            "{missing_sources} source reference(s) are unavailable; restore or \
        relink them before recompiling. Missing sources are excluded from automatic recompile."
        );
        notes = Some(match notes {
            Some(existing) => format!("{existing}. {note}"),
            None => note,
        });
    }

    let _ = crate::util::blocking_section(|| {
        store.append_ops_log(&OpsLogEntry {
            id: Uuid::new_v4().to_string(),
            seq: 0,
            ts: Utc::now(),
            op: "refresh_stale_wiki".into(),
            prefix: Some("MAINT".into()),
            message: format!(
                "refresh_stale_wiki dry_run={dry_run} stale={} raws={} applied={applied}",
                stale.len(),
                total_raws
            ),
            entity_id: None,
            entity_kind: Some("wiki".into()),
            payload_json: serde_json::json!({
                "stale_count": stale.len(),
                "raw_count": total_raws,
                "dry_run": dry_run,
                "applied": applied,
                "capped": capped,
                "max_docs": max_docs,
                "recompiled": recompiled.len(),
                "errors": errors.len(),
            })
            .to_string(),
            agent_name: agent.map(|s| s.to_string()),
        })
    })?;

    Ok(RefreshStaleWikiResult {
        stale_count: stale.len(),
        stale,
        raw_sources,
        recompiled,
        errors,
        dry_run,
        applied,
        capped,
        max_docs,
        notes,
    })
}

pub(super) fn insert_stale_pair(
    pairs: &mut std::collections::BTreeMap<(String, String), StaleWikiItem>,
    wiki: &Document,
    raw: &Document,
    link_kind: &str,
) {
    let key = (wiki.id.clone(), raw.id.clone());
    // Prefer graph_related when already present; otherwise first write wins.
    if let Some(existing) = pairs.get(&key) {
        if existing.link_kind == "graph_related" {
            return;
        }
        if link_kind != "graph_related" && existing.link_kind == link_kind {
            return;
        }
        if link_kind != "graph_related" {
            return;
        }
    }
    pairs.insert(
        key,
        StaleWikiItem {
            wiki_id: wiki.id.clone(),
            wiki_uri: wiki.uri.clone(),
            wiki_title: wiki.title.clone(),
            wiki_kind: wiki.kind.to_string(),
            wiki_updated_at: wiki.updated_at.to_rfc3339(),
            raw_id: raw.id.clone(),
            raw_uri: raw.uri.clone(),
            raw_title: raw.title.clone(),
            raw_updated_at: Some(raw.updated_at.to_rfc3339()),
            link_kind: link_kind.to_string(),
        },
    );
}

pub(super) fn group_raw_targets(stale: &[StaleWikiItem]) -> Vec<RawRefreshTarget> {
    let mut map: std::collections::BTreeMap<String, RawRefreshTarget> =
        std::collections::BTreeMap::new();
    for s in stale {
        if s.link_kind == "source_missing" {
            continue;
        }
        let entry = map
            .entry(s.raw_id.clone())
            .or_insert_with(|| RawRefreshTarget {
                raw_id: s.raw_id.clone(),
                raw_uri: s.raw_uri.clone(),
                raw_title: s.raw_title.clone(),
                stale_wiki_ids: Vec::new(),
            });
        if !entry.stale_wiki_ids.contains(&s.wiki_id) {
            entry.stale_wiki_ids.push(s.wiki_id.clone());
        }
    }
    // Stable order by raw_uri then id.
    let mut out: Vec<RawRefreshTarget> = map.into_values().collect();
    out.sort_by(|a, b| {
        a.raw_uri
            .cmp(&b.raw_uri)
            .then_with(|| a.raw_id.cmp(&b.raw_id))
    });
    out
}

/// Resolve raw parents linked from a wiki page (metadata + content).
/// Raw documents keyed by id or uri, borrowing the loaded list (no clones).
pub(super) type RawLookup<'a> = HashMap<&'a str, &'a Document>;

/// Id and (non-empty) uri lookups over `raws`.
pub(super) fn raw_lookups(raws: &[Document]) -> (RawLookup<'_>, RawLookup<'_>) {
    let by_id = raws.iter().map(|raw| (raw.id.as_str(), raw)).collect();
    let by_uri = raws
        .iter()
        .filter(|raw| !raw.uri.is_empty())
        .map(|raw| (raw.uri.as_str(), raw))
        .collect();
    (by_id, by_uri)
}

pub(super) fn linked_raws_for_wiki(
    store: &Store,
    wiki: &Document,
    raw_by_id: &RawLookup<'_>,
    raw_by_uri: &RawLookup<'_>,
) -> Result<Vec<(Document, &'static str)>> {
    let mut out: Vec<(Document, &'static str)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    if let Some(meta) = wiki.metadata_lenient() {
        if let Some(sid) = meta.get("source_id").and_then(|v| v.as_str()) {
            try_push_raw(
                store, sid, "metadata", raw_by_id, raw_by_uri, &mut seen, &mut out,
            )?;
        }
        if let Some(suri) = meta.get("source_uri").and_then(|v| v.as_str()) {
            try_push_raw(
                store, suri, "metadata", raw_by_id, raw_by_uri, &mut seen, &mut out,
            )?;
        }
        // Consolidation persists every parent in arrays. These references must
        // remain sufficient even when graph relations are absent or rebuilt.
        for field in ["source_ids", "source_uris", "consolidated_from"] {
            if let Some(references) = meta.get(field).and_then(|value| value.as_array()) {
                for reference in references.iter().filter_map(|value| value.as_str()) {
                    try_push_raw(
                        store, reference, "metadata", raw_by_id, raw_by_uri, &mut seen, &mut out,
                    )?;
                }
            }
        }
        if let Some(arr) = meta.get("citations").and_then(|v| v.as_array()) {
            for c in arr {
                if let Some(did) = c.get("document_id").and_then(|v| v.as_str()) {
                    try_push_raw(
                        store, did, "citation", raw_by_id, raw_by_uri, &mut seen, &mut out,
                    )?;
                }
                if let Some(uri) = c.get("uri").and_then(|v| v.as_str()) {
                    try_push_raw(
                        store, uri, "citation", raw_by_id, raw_by_uri, &mut seen, &mut out,
                    )?;
                }
            }
        }
    }

    for line in wiki.content.lines() {
        let t = line.trim();
        let rest = t
            .strip_prefix("source:")
            .or_else(|| t.strip_prefix("source_uri:"))
            .or_else(|| t.strip_prefix("Source:"));
        if let Some(rest) = rest {
            let ref_s = rest.trim().trim_matches('`').trim();
            if !ref_s.is_empty() {
                try_push_raw(
                    store,
                    ref_s,
                    "content_source",
                    raw_by_id,
                    raw_by_uri,
                    &mut seen,
                    &mut out,
                )?;
            }
        }
    }

    Ok(out)
}

pub(super) fn try_push_raw(
    store: &Store,
    id_or_uri: &str,
    kind: &'static str,
    raw_by_id: &RawLookup<'_>,
    raw_by_uri: &RawLookup<'_>,
    seen: &mut std::collections::HashSet<String>,
    out: &mut Vec<(Document, &'static str)>,
) -> Result<()> {
    let id = id_or_uri.trim();
    if id.is_empty() {
        return Ok(());
    }
    let raw = if let Some(r) = raw_by_id.get(id) {
        (*r).clone()
    } else if let Some(r) = raw_by_uri.get(id) {
        (*r).clone()
    } else if let Some(d) = store.get_document(id)? {
        if is_raw_layer(&d.layer) {
            d
        } else {
            return Ok(());
        }
    } else if let Some(d) = store.find_by_uri(id)? {
        if is_raw_layer(&d.layer) {
            d
        } else {
            return Ok(());
        }
    } else {
        return Ok(());
    };
    if seen.insert(raw.id.clone()) {
        out.push((raw, kind));
    }
    Ok(())
}
