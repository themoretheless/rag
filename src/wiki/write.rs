//! Wiki page writes (CAS, metadata merge), reads, listing and raw ingest.

use super::*;

/// Options for wiki page write (log op / extra metadata).
#[derive(Debug, Clone, Default)]
pub struct WriteWikiOpts {
    /// Ops log `op` (default `wiki_write`).
    pub op: Option<String>,
    /// Ops log prefix (default `WIKI`).
    pub prefix: Option<String>,
    /// Ops log message override.
    pub message: Option<String>,
    /// Extra keys merged into document `metadata_json`.
    pub extra_metadata: Option<serde_json::Value>,
    /// Replace the metadata snapshot before overlays (internal replication/restore).
    pub metadata_json_override: Option<String>,
    /// Extra keys merged into ops_log `payload_json`.
    pub extra_payload: Option<serde_json::Value>,
    /// Optimistic concurrency: must match existing document revision when set.
    pub if_match_revision: Option<i64>,
}

/// Transport-neutral command for creating or replacing a wiki page.
#[derive(Debug, Clone)]
pub struct WikiWriteCommand {
    pub slug: String,
    pub title: String,
    pub content: String,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub kind: String,
    pub category: Option<String>,
    pub summary: Option<String>,
    pub agent: Option<String>,
    pub options: WriteWikiOpts,
}

/// Borrowed fields for [`write_wiki_page`].
#[derive(Debug, Clone, Copy)]
pub struct WikiPageWrite<'a> {
    /// Kebab slug; the page lives at `wiki://{slug}`.
    pub slug: &'a str,
    pub title: &'a str,
    pub content: &'a str,
    pub kind: &'a str,
    pub category: Option<&'a str>,
    pub summary: Option<&'a str>,
    pub agent: Option<&'a str>,
}

/// Write/update a wiki page by slug (`wiki://{slug}`), chunk+embed+graph.
pub async fn write_wiki_page(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    page: WikiPageWrite<'_>,
) -> Result<WikiWriteResult> {
    let WikiPageWrite {
        slug,
        title,
        content,
        kind,
        category,
        summary,
        agent,
    } = page;
    write_wiki_page_command(
        store,
        embedder,
        config,
        WikiWriteCommand {
            slug: slug.to_string(),
            title: title.to_string(),
            content: content.to_string(),
            wing: None,
            room: None,
            kind: kind.to_string(),
            category: category.map(str::to_string),
            summary: summary.map(str::to_string),
            agent: agent.map(str::to_string),
            options: WriteWikiOpts::default(),
        },
    )
    .await
}

/// Borrowed fields for [`update_wiki_page_cas`]. `None` title/kind keep the
/// page's current values.
#[derive(Debug, Clone, Copy)]
pub struct WikiPageUpdate<'a> {
    /// Document id, `wiki://slug`, or bare slug of an existing wiki page.
    pub id_or_slug: &'a str,
    pub title: Option<&'a str>,
    pub content: &'a str,
    pub kind: Option<&'a str>,
    pub category: Option<&'a str>,
    pub summary: Option<&'a str>,
    pub agent: Option<&'a str>,
    /// Optimistic concurrency: must match the current revision when set.
    pub if_match_revision: Option<i64>,
}

/// Command-object entrypoint used by transports and application services.
pub async fn write_wiki_page_command(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    command: WikiWriteCommand,
) -> Result<WikiWriteResult> {
    let identity = crate::db::sync::SyncIdentity::current()?;
    let (write, _) = write_wiki_page_synced_command(
        store,
        embedder,
        config,
        command,
        crate::db::sync::WikiSyncWrite::Local(identity),
        false,
    )
    .await?;
    write.ok_or_else(|| AppError::db("local wiki write returned no committed document"))
}

/// Create a wiki page only if its URI is absent, including archived documents.
/// The final absence check shares the document/chunks/catalog/journal transaction.
pub(crate) async fn create_wiki_page_command(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    command: WikiWriteCommand,
) -> Result<WikiWriteResult> {
    let identity = crate::db::sync::SyncIdentity::current()?;
    let (write, _) = write_wiki_page_synced_command(
        store,
        embedder,
        config,
        command,
        crate::db::sync::WikiSyncWrite::Local(identity),
        true,
    )
    .await?;
    write.ok_or_else(|| AppError::db("wiki create returned no committed document"))
}

/// Internal replication entrypoint. Only this typed mode can suppress re-journalling;
/// an agent name or any other user-supplied wiki option cannot impersonate sync.
pub(crate) async fn write_wiki_page_synced_command(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    command: WikiWriteCommand,
    sync: crate::db::sync::WikiSyncWrite<'_>,
    create_only: bool,
) -> Result<(Option<WikiWriteResult>, i64)> {
    let incoming = !matches!(sync, crate::db::sync::WikiSyncWrite::Local(_));
    // Every transport and nested wiki workflow shares this boundary. Keep the
    // lease through embedding and commit so reembed_all cannot change identity
    // after a successful preflight but before these vectors are persisted.
    let _embedding_guard = store.try_embedding_write_guard(config, "wiki write")?;
    let WikiWriteCommand {
        slug,
        title,
        content,
        wing,
        room,
        kind,
        category,
        summary,
        agent,
        options: opts,
    } = command;
    let slug = slugify(&slug);
    if slug.is_empty() {
        return Err(AppError::config("wiki slug must not be empty"));
    }
    let uri = format!("wiki://{slug}");
    let now = Utc::now();
    let kind = if kind.trim().is_empty() {
        LAYER_WIKI
    } else {
        kind.trim()
    };

    let if_match = opts.if_match_revision;
    // Never delete chunks before CAS succeeds: a stale if_match (or a concurrent
    // loser) must leave the previous body+chunks intact for retrieval.
    // Point reads, but they wait on the store mutex, which a long rebuild may
    // hold: read on the blocking pool, never on an async worker.
    let (existing, existing_index) = {
        let (store, uri, slug) = (store.clone(), uri.clone(), slug.clone());
        crate::util::run_blocking("wiki write read", move || {
            let existing = store.find_by_uri(&uri)?;
            let existing_index = if existing.is_some() {
                store.get_wiki_index_by_slug(&slug)?
            } else {
                None
            };
            Ok((existing, existing_index))
        })
        .await?
    };
    if create_only && existing.is_some() {
        return Err(AppError::conflict("wiki page already exists"));
    }
    let (document_id, created_at) = if let Some(existing) = existing.as_ref() {
        // Multi-LLM CAS: when RAG_WIKI_REQUIRE_IF_MATCH=true, updates must pass if_match.
        if config.wiki_require_if_match && if_match.is_none() && !incoming {
            return Err(AppError::config(format!(
                "if_match_revision (or if_match_etag) is required to update wiki page '{uri}' \
                 when RAG_WIKI_REQUIRE_IF_MATCH=true; call get_wiki_page and pass revision"
            )));
        }
        // Enforce raw immutability: never rewrite raw content via wiki write.
        assert_content_mutable(existing)?;
        // Fail-fast CAS check (store upsert still enforces under the write lock).
        if let Some(expected) = if_match {
            if existing.revision != expected {
                return Err(AppError::conflict(format!(
                    "etag mismatch: if_match_revision={expected} but document {} has revision {}",
                    existing.id, existing.revision
                )));
            }
        }
        (existing.id.clone(), existing.created_at)
    } else {
        if if_match.is_some() {
            return Err(AppError::conflict(format!(
                "etag mismatch: if_match_revision set but wiki page '{uri}' does not exist"
            )));
        }
        (Uuid::new_v4().to_string(), now)
    };

    let title = if title.trim().is_empty() {
        slug.clone()
    } else {
        title.trim().to_string()
    };

    // Updating a page must not silently turn a project-scoped/pinned document
    // into an unscoped default row. Start from the persisted document and
    // replace only fields owned by this command. Optional metadata overlays are
    // merged into the existing object so unrelated application keys survive.
    let metadata_overlay_requested =
        category.is_some() || summary.is_some() || opts.extra_metadata.is_some();
    if let Some(metadata) = &opts.metadata_json_override {
        let value: serde_json::Value = serde_json::from_str(metadata)
            .map_err(|error| AppError::config(format!("invalid wiki metadata JSON: {error}")))?;
        if !value.is_object() {
            return Err(AppError::config("wiki metadata must be a JSON object"));
        }
    }
    let metadata_json =
        if let Some(metadata) = opts.metadata_json_override.as_ref().filter(|_| incoming) {
            metadata.clone()
        } else {
            merge_wiki_metadata(
                opts.metadata_json_override.as_deref().or_else(|| {
                    existing
                        .as_ref()
                        .map(|document| document.metadata_json.as_str())
                }),
                category.as_deref(),
                summary.as_deref(),
                opts.extra_metadata,
                metadata_overlay_requested,
            )?
        };
    let mut doc = existing.clone().unwrap_or_default();
    doc.id = document_id.clone();
    doc.uri = uri.clone();
    doc.title = title.clone();
    doc.content = content.clone();
    if let Some(wing) = wing
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        doc.wing = Some(wing.to_string());
    }
    if let Some(room) = room
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        doc.room = Some(room.to_string());
    }
    if incoming {
        doc.wing = wing.clone();
        doc.room = room.clone();
    }
    doc.metadata_json = if incoming && opts.metadata_json_override.is_none() {
        let mut metadata: serde_json::Value = serde_json::from_str(&metadata_json)?;
        if let Some(object) = metadata.as_object_mut() {
            object.remove("category");
            object.remove("summary");
            if let Some(category) = &category {
                object.insert("category".into(), category.clone().into());
            }
            if let Some(summary) = &summary {
                object.insert("summary".into(), summary.clone().into());
            }
        }
        serde_json::to_string(&metadata)?
    } else {
        metadata_json
    };
    doc.created_at = created_at;
    doc.updated_at = now;
    doc.layer = LAYER_WIKI.into();
    doc.kind = kind.into();
    doc.content_hash = Some(content_hash(&content));
    let chunk_embedder =
        crate::document_indexer::reusing_document_vectors(store, embedder, &document_id)?;
    let chunks = DocumentIndexer::new(chunk_embedder.as_ref(), config)
        .build_plain_chunks(&doc)
        .await?;
    for chunk in &chunks {
        if chunk.embedding.len() != config.embedding_dims {
            return Err(AppError::embeddings(format!(
                "wiki embedder returned dims={}, expected {} from corpus configuration",
                chunk.embedding.len(),
                config.embedding_dims
            )));
        }
    }
    let chunk_count = chunks.len();

    let metadata = doc.metadata_lenient();
    let category = category.or_else(|| {
        if incoming {
            return None;
        }
        existing_index
            .as_ref()
            .and_then(|entry| entry.category.clone())
            .or_else(|| metadata_string_field(metadata.as_ref(), "category").map(str::to_string))
    });
    let summary = summary
        .or_else(|| {
            if incoming {
                return None;
            }
            existing_index
                .as_ref()
                .and_then(|entry| entry.summary.clone())
        })
        .or_else(|| metadata_string_field(metadata.as_ref(), "summary").map(str::to_string))
        .unwrap_or_else(|| first_line(&content, 240));
    let index_id = existing_index
        .as_ref()
        .map(|entry| entry.id.clone())
        .unwrap_or_else(|| format!("idx-{document_id}"));
    let index_entry = WikiIndexEntry {
        id: index_id.clone(),
        slug: slug.clone(),
        title: title.clone(),
        kind: kind.to_string(),
        category,
        summary: Some(summary),
        page_id: Some(document_id.clone()),
        updated_at: now,
    };

    let op = opts.op.unwrap_or_else(|| "wiki_write".into());
    let prefix = opts.prefix.unwrap_or_else(|| "WIKI".into());
    let message = opts
        .message
        .unwrap_or_else(|| format!("wrote wiki://{slug}"));
    let mut payload = serde_json::json!({ "slug": slug, "kind": kind });
    if let Some(extra) = opts.extra_payload {
        if let (Some(obj), serde_json::Value::Object(map)) = (payload.as_object_mut(), extra) {
            for (k, v) in map {
                obj.insert(k, v);
            }
        }
    }

    let audit_entry = OpsLogEntry {
        id: Uuid::new_v4().to_string(),
        seq: 0,
        ts: now,
        op,
        prefix: Some(prefix),
        message,
        entity_id: Some(document_id.clone()),
        entity_kind: Some(LAYER_WIKI.into()),
        payload_json: payload.to_string(),
        agent_name: agent.clone(),
    };
    // The commit writes chunks and rebuilds the page's graph slice. Local
    // writes (every agent and HTTP write) commit on the blocking pool; the
    // replication paths borrow their sync event, so they hand their worker to
    // blocking mode instead.
    let (write, sequence) = match sync {
        crate::db::sync::WikiSyncWrite::Local(identity) => {
            let store = store.clone();
            crate::util::run_blocking("wiki write commit", move || {
                store.write_wiki_document_atomic(crate::db::store::WikiDocumentWrite {
                    doc: &doc,
                    if_match_revision: if_match,
                    chunks: &chunks,
                    index_entry: &index_entry,
                    audit_entry: &audit_entry,
                    sync: &crate::db::sync::WikiSyncWrite::Local(identity),
                    create_only,
                })
            })
            .await?
        }
        replicated => crate::util::blocking_section(|| {
            store.write_wiki_document_atomic(crate::db::store::WikiDocumentWrite {
                doc: &doc,
                if_match_revision: if_match,
                chunks: &chunks,
                index_entry: &index_entry,
                audit_entry: &audit_entry,
                sync: &replicated,
                create_only,
            })
        })?,
    };
    let Some(write) = write else {
        return Ok((None, sequence));
    };
    let revision = write.revision;
    let node_id = write.node_id.unwrap_or_default();
    let edge_count = write.edge_count;

    Ok((
        Some(WikiWriteResult {
            document_id,
            uri,
            slug,
            chunk_count,
            node_id,
            edge_count,
            index_id,
            revision,
            etag: crate::models::format_document_etag(revision),
        }),
        sequence,
    ))
}

pub(super) fn merge_wiki_metadata(
    existing: Option<&str>,
    category: Option<&str>,
    summary: Option<&str>,
    extra: Option<serde_json::Value>,
    overlay_requested: bool,
) -> Result<String> {
    let existing = existing.unwrap_or("{}");
    if !overlay_requested {
        return Ok(if existing.trim().is_empty() {
            "{}".into()
        } else {
            existing.to_string()
        });
    }

    let mut metadata = if existing.trim().is_empty() {
        serde_json::Map::new()
    } else {
        match serde_json::from_str::<serde_json::Value>(existing) {
            Ok(serde_json::Value::Object(map)) => map,
            Ok(_) => {
                return Err(AppError::config(
                    "existing wiki metadata_json is not an object; refusing to discard it while applying metadata fields",
                ))
            }
            Err(error) => {
                return Err(AppError::config(format!(
                    "existing wiki metadata_json is invalid; refusing to discard it while applying metadata fields: {error}"
                )))
            }
        }
    };
    if let Some(category) = category {
        metadata.insert(
            "category".into(),
            serde_json::Value::String(category.to_string()),
        );
    }
    if let Some(summary) = summary {
        metadata.insert(
            "summary".into(),
            serde_json::Value::String(summary.to_string()),
        );
    }
    if let Some(extra) = extra {
        if let serde_json::Value::Object(map) = extra {
            metadata.extend(map);
        } else {
            metadata.insert("extra".into(), extra);
        }
    }
    Ok(serde_json::Value::Object(metadata).to_string())
}

pub(super) fn metadata_string_field<'a>(
    metadata: Option<&'a serde_json::Value>,
    field: &str,
) -> Option<&'a str> {
    metadata?.as_object()?.get(field)?.as_str()
}

/// Update an existing wiki page (by document id, `wiki://slug`, or slug).
///
/// Requires an existing `layer=wiki` document. Re-chunks, re-embeds, and
/// re-runs wikilink/tag graph extract. Rejects raw-layer documents. Honors
/// optional `if_match_revision` (multi-LLM CAS).
pub async fn update_wiki_page_cas(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    update: WikiPageUpdate<'_>,
) -> Result<WikiWriteResult> {
    let WikiPageUpdate {
        id_or_slug,
        title,
        content,
        kind,
        category,
        summary,
        agent,
        if_match_revision,
    } = update;
    let existing = get_wiki_page(store, id_or_slug)?;
    assert_content_mutable(&existing)?;

    let slug = slug_from_wiki_doc(&existing);
    let title = title
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(existing.title.as_str());
    let kind = kind
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(existing.kind.as_str());

    write_wiki_page_command(
        store,
        embedder,
        config,
        WikiWriteCommand {
            slug,
            title: title.to_string(),
            content: content.to_string(),
            wing: None,
            room: None,
            kind: kind.to_string(),
            category: category.map(str::to_string),
            summary: summary.map(str::to_string),
            agent: agent.map(str::to_string),
            options: WriteWikiOpts {
                if_match_revision,
                ..Default::default()
            },
        },
    )
    .await
}

/// Fetch a wiki page by document id, `wiki://slug`, or bare slug.
///
/// Returns [`AppError::NotFound`] when missing; [`AppError::Forbidden`] when
/// the document exists but is not `layer=wiki` (including raw sources).
///
/// Up to four point reads: they run off the async workers when called from
/// async code (a no-op when the caller already runs on a blocking thread).
pub fn get_wiki_page(store: &Store, id_or_uri_or_slug: &str) -> Result<Document> {
    crate::util::blocking_section(|| find_wiki_page(store, id_or_uri_or_slug))
}

fn find_wiki_page(store: &Store, id_or_uri_or_slug: &str) -> Result<Document> {
    let s = id_or_uri_or_slug.trim();
    if s.is_empty() {
        return Err(AppError::config(
            "get_wiki_page: id_or_slug must not be empty",
        ));
    }

    let doc = if let Some(d) = store.get_document(s)? {
        d
    } else if let Some(d) = store.find_by_uri(s)? {
        d
    } else {
        let slug = if let Some(rest) = s.strip_prefix("wiki://") {
            slugify(rest)
        } else {
            slugify(s)
        };
        if slug.is_empty() {
            return Err(AppError::not_found(format!(
                "wiki page not found: {id_or_uri_or_slug}"
            )));
        }
        let uri = format!("wiki://{slug}");
        if let Some(d) = store.find_by_uri(&uri)? {
            d
        } else if let Some(entry) = store.get_wiki_index_by_slug(&slug)? {
            if let Some(pid) = entry.page_id.as_deref() {
                store.get_document(pid)?.ok_or_else(|| {
                    AppError::not_found(format!(
                        "wiki page not found for index slug '{slug}' (stale page_id)"
                    ))
                })?
            } else {
                return Err(AppError::not_found(format!(
                    "wiki page not found: {id_or_uri_or_slug}"
                )));
            }
        } else {
            return Err(AppError::not_found(format!(
                "wiki page not found: {id_or_uri_or_slug}"
            )));
        }
    };

    if !is_wiki_layer(&doc.layer) {
        if is_raw_layer(&doc.layer) {
            return Err(AppError::forbidden(format!(
                "document {} is layer=raw (immutable source); use get_source / list_sources, not wiki page APIs",
                doc.id
            )));
        }
        return Err(AppError::forbidden(format!(
            "document {} has layer='{}'; expected layer=wiki",
            doc.id, doc.layer
        )));
    }
    Ok(doc)
}

/// List all documents with `layer=wiki`, ordered by `created_at` ascending.
pub fn list_wiki_pages(store: &Store) -> Result<Vec<Document>> {
    store.list_documents_by_layer(LAYER_WIKI)
}

/// Extract kebab slug from a wiki document uri (`wiki://slug`) or title.
pub(super) fn slug_from_wiki_doc(doc: &Document) -> String {
    if let Some(rest) = doc.uri.strip_prefix("wiki://") {
        let s = slugify(rest);
        if !s.is_empty() {
            return s;
        }
    }
    slugify(&doc.title)
}

/// Owned fields for [`ingest_raw`].
#[derive(Debug, Clone, Default)]
pub struct RawIngest {
    pub text: String,
    /// Defaults to the last uri segment.
    pub title: Option<String>,
    /// Defaults to a fresh `raw://{uuid}`.
    pub uri: Option<String>,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub source_file: Option<String>,
}

/// Ingest immutable raw text (layer=raw). Re-ingest same uri replaces chunks but keeps raw policy.
pub async fn ingest_raw(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    raw: RawIngest,
) -> Result<crate::models::IngestResult> {
    let RawIngest {
        text,
        title,
        uri,
        wing,
        room,
        source_file,
    } = raw;
    crate::util::blocking_section(|| {
        store.ensure_embedding_manifest(config)?;
        store.require_embedding_manifest_match(config)
    })?;
    let now = Utc::now();
    let uri = uri
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| format!("raw://{}", Uuid::new_v4()));
    let title = title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| {
        uri.rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("untitled")
            .to_string()
    });

    let existing = {
        let (store, uri) = (store.clone(), uri.clone());
        crate::util::run_blocking("ingest_raw read", move || store.find_by_uri(&uri)).await?
    };
    let (document_id, created_at) = if let Some(existing) = existing {
        (existing.id, existing.created_at)
    } else {
        (Uuid::new_v4().to_string(), now)
    };

    let doc = Document {
        id: document_id.clone(),
        uri: uri.clone(),
        title,
        content: text,
        metadata_json: "{}".into(),
        created_at,
        updated_at: now,
        wing,
        room,
        source_file,
        layer: LAYER_RAW.into(),
        kind: "document".into(),
        content_hash: None,
        ..Default::default()
    };
    let chunk_embedder =
        crate::document_indexer::reusing_document_vectors(store, embedder, &document_id)?;
    let chunks = DocumentIndexer::new(chunk_embedder.as_ref(), config)
        .build_plain_chunks(&doc)
        .await?;
    let chunk_count = chunks.len();
    let audit_entry = OpsLogEntry {
        id: Uuid::new_v4().to_string(),
        seq: 0,
        ts: now,
        op: "ingest_raw".into(),
        prefix: Some("INGEST".into()),
        message: format!("ingested raw {uri}"),
        entity_id: Some(document_id.clone()),
        entity_kind: Some(LAYER_RAW.into()),
        payload_json: "{}".into(),
        agent_name: None,
    };
    let content_digest = content_hash(&doc.content);
    // Commit (chunks + graph slice) and audit on the blocking pool.
    let write = {
        let store = store.clone();
        crate::util::run_blocking("ingest_raw commit", move || {
            // Re-ingest replace path: intentionally does not call assert_content_mutable.
            let write = store.write_document_atomic(
                &doc,
                None,
                DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
            )?;
            store.append_ops_log(&audit_entry)?;
            Ok(write)
        })
        .await?
    };
    let rev = write.revision;
    Ok(crate::models::IngestResult {
        document_id,
        chunk_count,
        node_id: write.node_id.unwrap_or_default(),
        edge_count: write.edge_count,
        content_hash: content_digest,
        op: "inserted".into(),
        revision: rev,
        etag: crate::models::format_document_etag(rev),
    })
}
