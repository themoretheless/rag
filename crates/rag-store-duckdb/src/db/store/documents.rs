//! Document lookups, listings, catalog metadata, taxonomy and placement.

use super::*;
use crate::db::lin_engine::{
    cell_i64, cell_nonempty_string, cell_opt_string, cell_str, cell_string, cell_ts,
};

/// Identity of a document without its body or metadata.
#[derive(Debug, Clone)]
pub struct DocumentRef {
    pub id: String,
    pub uri: String,
    pub layer: String,
}

impl Store {
    /// Fetch a document by id.
    pub fn get_document(&self, id: &str) -> Result<Option<Document>> {
        let r = self.read()?;
        get_document_locked(&r, id)
    }

    /// Fetch a document by stable URI (re-ingest lookup).
    pub fn find_by_uri(&self, uri: &str) -> Result<Option<Document>> {
        let r = self.read()?;
        let mut found = None;
        r.for_each_by_eq_projected("documents", "uri", uri, None, |row| {
            found = Some(rows::document(row)?);
            Ok(false)
        })?;
        Ok(found)
    }

    /// Document by id, else by uri. The uri query runs only when the id misses.
    pub fn find_by_id_or_uri(&self, id: &str, uri: &str) -> Result<Option<Document>> {
        if let Some(document) = self.get_document(id)? {
            return Ok(Some(document));
        }
        if uri.is_empty() {
            return Ok(None);
        }
        self.find_by_uri(uri)
    }

    /// Stored `content_hash` for each id that exists, in one read guard.
    pub fn content_hashes(
        &self,
        ids: &[&str],
    ) -> Result<std::collections::HashMap<String, Option<String>>> {
        let mut out = std::collections::HashMap::with_capacity(ids.len());
        if ids.is_empty() {
            return Ok(out);
        }
        let r = self.read()?;
        for &id in ids {
            let maybe_row = if let Some(eng) = r.disk_v2() {
                eng.get_projected_by_id("documents", id, &["content_hash"])?
            } else {
                r.get_by_id("documents", id)?
            };
            if let Some(row) = maybe_row {
                out.insert(id.to_string(), cell_opt_string(&row, "content_hash"));
            }
        }
        Ok(out)
    }

    /// Fetch the first document matching `content_hash` (dedupe probe).
    pub fn find_by_content_hash(&self, hash: &str) -> Result<Option<Document>> {
        Ok(self.list_by_content_hash(hash)?.into_iter().next())
    }

    /// All documents with the given `content_hash`, ordered by `created_at` ascending.
    pub fn list_by_content_hash(&self, hash: &str) -> Result<Vec<Document>> {
        let r = self.read()?;
        let rows = r.find_by_eq("documents", "content_hash", hash)?;
        let mut docs: Vec<Document> = rows
            .iter()
            .map(rows::document)
            .collect::<Result<Vec<_>>>()?;
        docs.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(docs)
    }

    /// Alias of [`Self::list_by_content_hash`].
    pub fn find_all_by_content_hash(&self, hash: &str) -> Result<Vec<Document>> {
        self.list_by_content_hash(hash)
    }

    /// Content-hash / URI idempotency probe for `check_duplicate`.
    pub fn check_duplicate(
        &self,
        content: Option<&str>,
        content_hash_in: Option<&str>,
        uri: Option<&str>,
    ) -> Result<DuplicateCheckResult> {
        let hash = match content_hash_in.map(str::trim).filter(|s| !s.is_empty()) {
            Some(h) => Some(h.to_string()),
            None => content
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(content_hash),
        };
        let uri = uri.map(str::trim).filter(|s| !s.is_empty());

        if hash.is_none() && uri.is_none() {
            return Err(AppError::config(
                "check_duplicate requires content, content_hash, or uri",
            ));
        }

        let mut by_id: std::collections::BTreeMap<String, (Document, Vec<&'static str>)> =
            std::collections::BTreeMap::new();

        if let Some(ref h) = hash {
            for doc in self.list_by_content_hash(h)? {
                by_id
                    .entry(doc.id.clone())
                    .or_insert_with(|| (doc, Vec::new()))
                    .1
                    .push("content_hash");
            }
        }

        if let Some(u) = uri {
            if let Some(doc) = self.find_by_uri(u)? {
                by_id
                    .entry(doc.id.clone())
                    .or_insert_with(|| (doc, Vec::new()))
                    .1
                    .push("uri");
            }
        }

        let matches: Vec<DuplicateMatch> = by_id
            .into_values()
            .map(|(doc, reasons)| {
                let match_reason = if reasons.contains(&"content_hash") && reasons.contains(&"uri")
                {
                    "content_hash+uri".to_string()
                } else if reasons.contains(&"uri") {
                    "uri".to_string()
                } else {
                    "content_hash".to_string()
                };
                DuplicateMatch {
                    id: doc.id,
                    uri: doc.uri,
                    title: doc.title,
                    content_hash: doc.content_hash,
                    wing: doc.wing,
                    room: doc.room,
                    source_file: doc.source_file,
                    match_reason,
                }
            })
            .collect();

        Ok(DuplicateCheckResult {
            is_duplicate: !matches.is_empty(),
            content_hash: hash,
            matches,
        })
    }

    /// List all documents ordered by `created_at` ascending.
    pub fn list_documents(&self) -> Result<Vec<Document>> {
        let r = self.read()?;
        let mut docs = r.filter_map_rows("documents", |row| Ok(Some(rows::document(row)?)))?;
        docs.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(docs)
    }

    /// Current `revision` of one document, without loading its body.
    pub fn document_revision(&self, id: &str) -> Result<Option<i64>> {
        let r = self.read()?;
        let maybe_row = if let Some(eng) = r.disk_v2() {
            eng.get_projected_by_id("documents", id, &["revision"])?
        } else {
            r.get_by_id("documents", id)?
        };
        Ok(maybe_row.map(|row| cell_i64(&row, "revision", 1).max(1)))
    }

    /// `(id, uri)` of one document, without loading its body.
    pub fn document_id_and_uri(&self, id: &str) -> Result<Option<(String, String)>> {
        let r = self.read()?;
        let maybe_row = if let Some(eng) = r.disk_v2() {
            eng.get_projected_by_id("documents", id, &["id", "uri"])?
        } else {
            r.get_by_id("documents", id)?
        };
        Ok(maybe_row.map(|row| (cell_string(&row, "id"), cell_string(&row, "uri"))))
    }

    /// `(id, uri, layer)` of every document ordered by `created_at`, without bodies.
    pub fn list_document_refs(&self) -> Result<Vec<DocumentRef>> {
        let r = self.read()?;
        let mut items: Vec<(DateTime<Utc>, DocumentRef)> = Vec::new();
        r.scan_each_projected("documents", &["id", "uri", "layer", "created_at"], |row| {
            let ts = cell_ts(row, "created_at")?;
            items.push((
                ts,
                DocumentRef {
                    id: cell_string(row, "id"),
                    uri: cell_string(row, "uri"),
                    layer: cell_opt_string(row, "layer").unwrap_or_default(),
                },
            ));
            Ok(true)
        })?;
        items.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
        Ok(items.into_iter().map(|(_, r)| r).collect())
    }

    /// Count documents without loading document bodies.
    pub fn count_documents(&self) -> Result<usize> {
        let r = self.read()?;
        r.count_rows("documents", None)
    }

    /// Count documents in one layer without loading their bodies.
    pub fn count_documents_by_layer(&self, layer: &str) -> Result<usize> {
        let r = self.read()?;
        let mut count = 0usize;
        r.for_each_by_eq_projected("documents", "layer", layer, Some(&["id"]), |_| {
            count += 1;
            Ok(true)
        })?;
        Ok(count)
    }

    /// Count documents eligible for a graph rebuild.
    pub fn count_graph_rebuild_candidates(&self, dirty_only: bool) -> Result<usize> {
        let r = self.read()?;
        if !dirty_only {
            return r.count_rows("documents", None);
        }
        let mut owned_docs = std::collections::HashSet::new();
        r.scan_each_projected("graph_nodes", &["document_id"], |row| {
            if let Some(doc_id) = cell_str(row, "document_id").filter(|s| !s.is_empty()) {
                owned_docs.insert(doc_id.to_string());
            }
            Ok(true)
        })?;
        let mut count = 0usize;
        r.scan_each_projected("documents", &["id"], |row| {
            if let Some(id) = cell_str(row, "id")
                && !owned_docs.contains(id)
            {
                count += 1;
            }
            Ok(true)
        })?;
        Ok(count)
    }

    /// Load one deterministic bounded page of graph rebuild candidates.
    pub fn list_graph_rebuild_candidates_after(
        &self,
        dirty_only: bool,
        after_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Document>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let r = self.read()?;
        let after = after_id.unwrap_or("");
        let mut owned_docs = std::collections::HashSet::new();
        if dirty_only {
            r.scan_each_projected("graph_nodes", &["document_id"], |row| {
                if let Some(doc_id) = cell_str(row, "document_id").filter(|s| !s.is_empty()) {
                    owned_docs.insert(doc_id.to_string());
                }
                Ok(true)
            })?;
        }
        let mut candidate_ids: Vec<String> = Vec::new();
        r.scan_each_projected("documents", &["id"], |row| {
            if let Some(id) = cell_str(row, "id")
                && id > after
                && (!dirty_only || !owned_docs.contains(id))
            {
                candidate_ids.push(id.to_string());
            }
            Ok(true)
        })?;
        candidate_ids.sort();
        candidate_ids.truncate(limit);
        let mut docs = Vec::with_capacity(candidate_ids.len());
        for id in candidate_ids {
            if let Some(row) = r.get_by_id("documents", &id)? {
                docs.push(rows::document(&row)?);
            }
        }
        Ok(docs)
    }

    /// Load one deterministic bounded page of document ids for full-corpus work.
    pub fn list_document_ids_after(
        &self,
        after_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let r = self.read()?;
        let after = after_id.unwrap_or("");
        let mut ids: Vec<String> = Vec::new();
        r.scan_each_projected("documents", &["id"], |row| {
            if let Some(id) = cell_str(row, "id")
                && id > after
            {
                ids.push(id.to_string());
            }
            Ok(true)
        })?;
        ids.sort();
        ids.truncate(limit);
        Ok(ids)
    }

    /// Count chunks attached to the first `limit` documents in keyset order.
    pub fn count_chunks_for_document_limit(&self, limit: usize) -> Result<usize> {
        if limit == 0 {
            return Ok(0);
        }
        let r = self.read()?;
        let mut ids: Vec<String> = Vec::new();
        r.scan_each_projected("documents", &["id"], |row| {
            if let Some(id) = cell_str(row, "id") {
                ids.push(id.to_string());
            }
            Ok(true)
        })?;
        ids.sort();
        ids.truncate(limit);
        let mut count = 0usize;
        for id in &ids {
            r.for_each_by_eq_projected(
                "chunks",
                "document_id",
                id.as_str(),
                Some(&["id"]),
                |_| {
                    count += 1;
                    Ok(true)
                },
            )?;
        }
        Ok(count)
    }

    /// Count and return a bounded deterministic prefix of persisted-hash matches.
    pub fn exact_duplicate_source_ids(
        &self,
        hash: &str,
        keep_id: &str,
        limit: usize,
    ) -> Result<(usize, Vec<String>)> {
        let r = self.read()?;
        let mut matches: Vec<(DateTime<Utc>, String)> = Vec::new();
        r.for_each_by_eq_projected(
            "documents",
            "content_hash",
            hash,
            Some(&["id", "created_at"]),
            |row| {
                if let Some(id) = cell_str(row, "id")
                    && id != keep_id
                {
                    matches.push((cell_ts(row, "created_at")?, id.to_string()));
                }
                Ok(true)
            },
        )?;
        let count = matches.len();
        if limit == 0 {
            return Ok((count, Vec::new()));
        }
        matches.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        matches.truncate(limit);
        Ok((count, matches.into_iter().map(|(_, id)| id).collect()))
    }

    /// Count and load only the bounded active, user-organizable unscoped set (without bodies).
    pub fn list_active_unscoped_documents(&self, limit: usize) -> Result<(usize, Vec<Document>)> {
        let r = self.read()?;
        let mut candidates: Vec<Document> = Vec::new();
        r.scan_each_projected(
            "documents",
            &[
                "id",
                "uri",
                "title",
                "metadata_json",
                "content_hash",
                "wing",
                "room",
                "source_file",
                "layer",
                "kind",
                "created_at",
                "updated_at",
                "status",
                "pinned",
                "boost",
                "revision",
            ],
            |row| {
                let status = cell_nonempty_string(row, "status").unwrap_or_else(|| "active".into());
                if status == "archived" || status == "tombstone" {
                    return Ok(true);
                }
                let layer = cell_str(row, "layer").unwrap_or("");
                if matches!(layer, "schema" | "index" | "log") {
                    return Ok(true);
                }
                let wing = cell_str(row, "wing").map(str::trim).unwrap_or("");
                if !wing.is_empty() {
                    return Ok(true);
                }
                candidates.push(rows::document_without_body(row)?);
                Ok(true)
            },
        )?;
        let count = candidates.len();
        candidates.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.title.cmp(&b.title))
                .then_with(|| a.id.cmp(&b.id))
        });
        candidates.truncate(limit.max(1));
        Ok((count, candidates))
    }

    /// Count and load a bounded batch of searchable documents whose derived chunks are missing.
    pub fn list_documents_missing_chunks(&self, limit: usize) -> Result<(usize, Vec<Document>)> {
        let r = self.read()?;
        let mut docs_with_chunks = std::collections::HashSet::new();
        r.scan_each_projected("chunks", &["document_id"], |row| {
            if let Some(doc_id) = cell_str(row, "document_id") {
                docs_with_chunks.insert(doc_id.to_string());
            }
            Ok(true)
        })?;

        let mut missing_keys: Vec<(DateTime<Utc>, String)> = Vec::new();
        r.scan_each_projected(
            "documents",
            &["id", "layer", "content", "updated_at"],
            |row| {
                let Some(id) = cell_str(row, "id") else {
                    return Ok(true);
                };
                if docs_with_chunks.contains(id) {
                    return Ok(true);
                }
                let layer = cell_str(row, "layer").map(str::trim).unwrap_or("");
                let eff_layer = if layer.is_empty() { "raw" } else { layer };
                if eff_layer.eq_ignore_ascii_case("schema") {
                    return Ok(true);
                }
                let content = cell_str(row, "content").map(str::trim).unwrap_or("");
                if content.is_empty() {
                    return Ok(true);
                }
                missing_keys.push((cell_ts(row, "updated_at")?, id.to_string()));
                Ok(true)
            },
        )?;

        let count = missing_keys.len();
        missing_keys.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        missing_keys.truncate(limit.max(1));
        let mut docs = Vec::with_capacity(missing_keys.len());
        for (_, id) in missing_keys {
            if let Some(row) = r.get_by_id("documents", &id)? {
                docs.push(rows::document(&row)?);
            }
        }
        Ok((count, docs))
    }

    /// List documents with the given `layer` (`raw`, `wiki`, …), ordered by `created_at`.
    pub fn list_documents_by_layer(&self, layer: &str) -> Result<Vec<Document>> {
        let r = self.read()?;
        let rows = r.find_by_eq("documents", "layer", layer)?;
        let mut out: Vec<Document> = rows
            .iter()
            .map(rows::document)
            .collect::<Result<Vec<_>>>()?;
        out.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    /// Like [`Self::list_documents_by_layer`], with an empty `content`.
    pub fn list_documents_by_layer_without_body(&self, layer: &str) -> Result<Vec<Document>> {
        let r = self.read()?;
        let rows = r.find_by_eq("documents", "layer", layer)?;
        let mut out: Vec<Document> = rows
            .iter()
            .map(rows::document_without_body)
            .collect::<Result<Vec<_>>>()?;
        out.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    /// `content` of each of `ids` that exists, in one read guard.
    pub fn document_contents(
        &self,
        ids: &[&str],
    ) -> Result<std::collections::HashMap<String, String>> {
        let mut out = std::collections::HashMap::with_capacity(ids.len());
        if ids.is_empty() {
            return Ok(out);
        }
        let r = self.read()?;
        for &id in ids {
            if let Some(row) = r.get_by_id("documents", id)? {
                out.insert(id.to_string(), cell_string(&row, "content"));
            }
        }
        Ok(out)
    }

    /// Wiki/catalog listing **without** loading `content` (sidebar / full catalog).
    pub fn list_wiki_page_metas(&self) -> Result<Vec<crate::models::WikiPageListItem>> {
        self.list_wiki_page_metas_filtered(&WikiPageMetaFilter::default())
            .map(|(items, _total)| items)
    }

    /// Wiki catalog with optional text query, placement/kind filters, and pagination.
    pub fn list_wiki_page_metas_filtered(
        &self,
        filter: &WikiPageMetaFilter,
    ) -> Result<(Vec<crate::models::WikiPageListItem>, usize)> {
        let r = self.read()?;
        fetch_wiki_page_meta_items(&r, filter)
    }

    /// Incoming wikilink sources for a document (label, document_id or node id).
    pub fn wiki_backlinks_for_document(&self, document_id: &str) -> Result<Vec<(String, String)>> {
        Ok(self
            .wiki_backlink_view(document_id)?
            .map(|(node_id, view)| wiki_backlink_rows(&node_id, &view))
            .unwrap_or_default())
    }

    /// The document's graph node id and its incoming-edge view, if it has a node.
    pub fn wiki_backlink_view(
        &self,
        document_id: &str,
    ) -> Result<Option<(String, crate::models::GraphView)>> {
        let Some(node) = self.find_node_by_document_id(document_id)? else {
            return Ok(None);
        };
        let view = self.backlinks(&node.id)?;
        Ok(Some((node.id, view)))
    }

    /// Incoming wikilink sources constrained to the same project wing.
    pub fn wiki_backlinks_for_document_in_wing(
        &self,
        document_id: &str,
        wing: &str,
    ) -> Result<Vec<(String, String)>> {
        let document_id = document_id.trim();
        let wing = wing.trim();
        if document_id.is_empty() || wing.is_empty() {
            return Ok(Vec::new());
        }
        let r = self.read()?;
        let Some(target_doc) = r.get_by_id("documents", document_id)? else {
            return Ok(Vec::new());
        };
        if cell_str(&target_doc, "wing") != Some(wing) {
            return Ok(Vec::new());
        }
        let target_nodes = r.find_by_eq("graph_nodes", "document_id", document_id)?;
        if target_nodes.is_empty() {
            return Ok(Vec::new());
        }

        // source_document_id -> MIN(source_node.label)
        let mut by_source_doc: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for tnode in &target_nodes {
            let Some(tnode_id) = cell_str(tnode, "id") else {
                continue;
            };
            let in_edges = r.find_by_eq("graph_edges", "target_id", tnode_id)?;
            for edge in &in_edges {
                if cell_str(edge, "rel_type") != Some("wikilink") {
                    continue;
                }
                let Some(src_node_id) = cell_str(edge, "source_id") else {
                    continue;
                };
                let Some(src_node) = r.get_by_id("graph_nodes", src_node_id)? else {
                    continue;
                };
                let Some(src_doc_id) = cell_str(&src_node, "document_id").filter(|s| !s.is_empty())
                else {
                    continue;
                };
                let Some(src_doc) = r.get_by_id("documents", src_doc_id)? else {
                    continue;
                };
                if cell_str(&src_doc, "wing") != Some(wing) {
                    continue;
                }
                let label = cell_string(&src_node, "label");
                by_source_doc
                    .entry(src_doc_id.to_string())
                    .and_modify(|cur| {
                        if label < *cur {
                            *cur = label.clone();
                        }
                    })
                    .or_insert(label);
            }
        }

        let mut out: Vec<(String, String)> = by_source_doc
            .into_iter()
            .map(|(doc_id, min_label)| (min_label, doc_id))
            .collect();
        out.sort();
        Ok(out)
    }

    /// List documents in the given `wing`, ordered by `created_at`.
    pub fn list_documents_by_wing(&self, wing: &str) -> Result<Vec<Document>> {
        let r = self.read()?;
        let rows = r.find_by_eq("documents", "wing", wing)?;
        let mut out: Vec<Document> = rows
            .iter()
            .map(rows::document)
            .collect::<Result<Vec<_>>>()?;
        out.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    /// List documents matching optional wing / room / status / layer / kind filters.
    pub fn list_documents_filtered(&self, filter: &DocumentFilter) -> Result<Vec<Document>> {
        let r = self.read()?;
        let include_archived = filter.include_archived.unwrap_or(false);
        let mut out = r.filter_map_rows("documents", |row| {
            if let Some(ref w) = filter.wing
                && !w.is_empty()
                && cell_str(row, "wing") != Some(w.as_str())
            {
                return Ok(None);
            }
            if let Some(ref rm) = filter.room
                && !rm.is_empty()
                && cell_str(row, "room") != Some(rm.as_str())
            {
                return Ok(None);
            }
            let row_status = cell_nonempty_string(row, "status").unwrap_or_else(|| "active".into());
            if let Some(ref s) = filter.status
                && !s.is_empty()
            {
                if row_status != *s {
                    return Ok(None);
                }
            } else if !include_archived && (row_status == "archived" || row_status == "tombstone") {
                return Ok(None);
            }
            if let Some(ref layer) = filter.layer
                && !layer.is_empty()
                && cell_str(row, "layer") != Some(layer.as_str())
            {
                return Ok(None);
            }
            if let Some(ref kind) = filter.kind
                && !kind.is_empty()
                && cell_str(row, "kind") != Some(kind.as_str())
            {
                return Ok(None);
            }
            if let Some(ref src) = filter.source_file
                && !src.is_empty()
                && cell_str(row, "source_file") != Some(src.as_str())
            {
                return Ok(None);
            }
            Ok(Some(rows::document(row)?))
        })?;

        out.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| {
                    b.boost
                        .partial_cmp(&a.boost)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| a.title.cmp(&b.title))
                .then_with(|| a.id.cmp(&b.id))
        });

        if let Some(limit) = filter.limit {
            out.truncate(limit.max(1));
        }
        Ok(out)
    }

    /// Distinct wings with document counts (non-null, non-empty wing), sorted by name.
    pub fn list_wings(&self) -> Result<Vec<WingCount>> {
        let r = self.read()?;
        let (_, buckets) = r.group_count_str1("documents", "wing")?;
        let mut counts: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
        for (w_opt, cnt) in buckets {
            if let Some(w) = w_opt
                && !w.trim().is_empty()
            {
                *counts.entry(w).or_insert(0) += cnt;
            }
        }
        Ok(counts
            .into_iter()
            .map(|(wing, document_count)| WingCount {
                wing,
                document_count,
            })
            .collect())
    }

    /// First-class project catalog backed by the compatible `wing` storage key.
    pub fn list_projects(&self) -> Result<Vec<crate::models::ProjectSummary>> {
        self.get_taxonomy()?
            .wings
            .into_iter()
            .map(|wing| {
                Ok(crate::models::ProjectSummary {
                    project_id: crate::models::ProjectId::parse(wing.wing)?,
                    document_count: wing.document_count,
                    rooms: wing.rooms,
                })
            })
            .collect()
    }

    /// Distinct rooms with document counts.
    pub fn list_rooms(&self, wing: Option<&str>) -> Result<Vec<RoomCount>> {
        let r = self.read()?;
        if let Some(w) = wing {
            let mut counts: std::collections::BTreeMap<String, u64> =
                std::collections::BTreeMap::new();
            r.for_each_by_eq_str3(
                "documents",
                "wing",
                w,
                "room",
                "id",
                None,
                |rm_opt, _, _| {
                    if let Some(rm) = rm_opt
                        && !rm.trim().is_empty()
                    {
                        *counts.entry(rm.to_string()).or_insert(0) += 1;
                    }
                    Ok(true)
                },
            )?;
            Ok(counts
                .into_iter()
                .map(|(room, document_count)| RoomCount {
                    wing: Some(w.to_string()),
                    room,
                    document_count,
                })
                .collect())
        } else {
            // Key: (wing.is_none_or_empty, wing_opt, room) so NULLS LAST on wing!
            let (_, pair_buckets) = r.group_count_str2("documents", "wing", "room")?;
            let mut counts: std::collections::BTreeMap<(bool, Option<String>, String), u64> =
                std::collections::BTreeMap::new();
            for ((w_raw, rm_raw), cnt) in pair_buckets {
                if let Some(rm) = rm_raw
                    && !rm.trim().is_empty()
                {
                    let w_opt = w_raw.filter(|s| !s.is_empty());
                    let is_null = w_opt.is_none();
                    *counts.entry((is_null, w_opt, rm)).or_insert(0) += cnt;
                }
            }
            Ok(counts
                .into_iter()
                .map(|((_, w_opt, room), document_count)| RoomCount {
                    wing: w_opt,
                    room,
                    document_count,
                })
                .collect())
        }
    }

    /// Full taxonomy tree: wings → rooms with document counts, plus unscoped totals.
    /// Single-pass `group_count_str2("documents", "wing", "room")` over `TextDict` `u16` ordinals!
    pub fn get_taxonomy(&self) -> Result<Taxonomy> {
        let r = self.read()?;
        let (total, pair_buckets) = r.group_count_str2("documents", "wing", "room")?;

        let mut wing_totals: std::collections::BTreeMap<String, u64> =
            std::collections::BTreeMap::new();
        let mut wing_rooms: std::collections::BTreeMap<
            String,
            std::collections::BTreeMap<String, u64>,
        > = std::collections::BTreeMap::new();
        let mut unscoped = 0u64;

        for ((w_raw, rm_raw), cnt) in pair_buckets {
            let has_wing = w_raw.as_deref().is_some_and(|w| !w.trim().is_empty());
            if !has_wing {
                unscoped += cnt;
                continue;
            }
            let w = w_raw.unwrap();
            *wing_totals.entry(w.clone()).or_insert(0) += cnt;
            if let Some(rm) = rm_raw
                && !rm.trim().is_empty()
            {
                *wing_rooms.entry(w).or_default().entry(rm).or_insert(0) += cnt;
            }
        }

        let mut wings = Vec::with_capacity(wing_totals.len());
        for (wing, document_count) in wing_totals {
            let rooms = wing_rooms
                .remove(&wing)
                .map(|rm_map| {
                    rm_map
                        .into_iter()
                        .map(|(room, document_count)| TaxonomyRoom {
                            room,
                            document_count,
                        })
                        .collect()
                })
                .unwrap_or_default();
            wings.push(TaxonomyWing {
                wing,
                document_count,
                rooms,
            });
        }

        Ok(Taxonomy {
            wings,
            unscoped_count: unscoped,
            total_documents: total,
        })
    }

    /// Apply a partial metadata update without re-embedding unless body text changes.
    pub fn update_document_meta(
        &self,
        id: &str,
        update: &DocumentMetaUpdate,
    ) -> Result<Option<DocumentMetaApplyResult>> {
        self.update_document_meta_with_identity(id, update, None)
    }

    pub fn update_document_meta_for_sync_test(
        &self,
        id: &str,
        update: &DocumentMetaUpdate,
        identity: &crate::db::sync::SyncIdentity,
    ) -> Result<Option<DocumentMetaApplyResult>> {
        self.update_document_meta_with_identity(id, update, Some(identity))
    }

    pub(super) fn update_document_meta_with_identity(
        &self,
        id: &str,
        update: &DocumentMetaUpdate,
        identity: Option<&crate::db::sync::SyncIdentity>,
    ) -> Result<Option<DocumentMetaApplyResult>> {
        let Some(mut applied) = self.prepare_document_meta_update(id, update)? else {
            return Ok(None);
        };
        let expected_revision = applied.document.revision;
        applied.document.revision = self
            .write_document_atomic_with_manifest(
                &applied.document,
                Some(expected_revision),
                DocumentDerivedWrite::Preserve,
                None,
                identity,
            )?
            .revision;
        Ok(Some(applied))
    }

    /// Prepare a metadata/body update without mutating storage.
    pub fn prepare_document_meta_update(
        &self,
        id: &str,
        update: &DocumentMetaUpdate,
    ) -> Result<Option<DocumentMetaApplyResult>> {
        self.get_document(id)?
            .map(|doc| apply_document_meta_update(doc, update))
            .transpose()
    }

    /// Placement-only alias for [`Self::update_document_meta`] (returns document only).
    pub fn update_document_placement(
        &self,
        id: &str,
        update: &PlacementUpdate,
    ) -> Result<Option<Document>> {
        Ok(self.update_document_meta(id, update)?.map(|r| r.document))
    }
}

/// `(label, document key)` of each wikilink source in a backlinks `view` of
/// `target_id`, sorted by label with adjacent duplicate keys collapsed.
pub fn wiki_backlink_rows(
    target_id: &str,
    view: &crate::models::GraphView,
) -> Vec<(String, String)> {
    let nodes: std::collections::HashMap<&str, &crate::models::GraphNode> = view
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut out = Vec::new();
    for e in &view.edges {
        if e.target_id != target_id || e.rel_type != "wikilink" {
            continue;
        }
        let Some(src) = nodes.get(e.source_id.as_str()) else {
            continue;
        };
        let key = src.document_id.clone().unwrap_or_else(|| src.id.clone());
        out.push((src.label.clone(), key));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.dedup_by(|a, b| a.1 == b.1);
    out
}
