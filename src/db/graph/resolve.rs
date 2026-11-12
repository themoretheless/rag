//! Resolve extracted links into graph nodes/edges for a document.

use std::path::Path;

use uuid::Uuid;

use crate::db::graph::{
    delete_derived_edges_from_locked, find_node_by_document_id_locked as find_node_by_document_id,
    find_node_by_uri_locked as find_node_by_uri, find_nodes_by_label_locked as find_nodes_by_label,
    insert_derived_graph_edges_locked as insert_graph_edges,
    upsert_graph_node_locked as upsert_graph_node, DerivedEdge,
};
use crate::db::lin_engine::{
    cell_i64, cell_nonempty_string, cell_str, cell_string, LinReader, LinWriter,
};
use crate::db::Store;
use crate::error::Result;
use crate::graph::extract::{
    extract_links_with, ExtractOptions, ExtractedLink, DEFAULT_MAX_LINKS_PER_DOC,
};
use crate::models::{Document, GraphEdge, GraphNode, Layer, NodeKind};
use crate::util::{slugify, wiki_slug_from_uri, SlugPolicy};

/// Rebuild the object-graph slice for `doc`.
///
/// 1. Upsert (or promote stub for) the document node — stable id by `document_id` / `uri`.
/// 2. Replace extracted outgoing edges, preserving explicit user/dependency links.
/// 3. Extract wikilinks + tags; resolve targets by label, slug, or `wiki://` uri; write edges.
///
/// Returns `(node_id, edge_count)` for the document node and edges written this pass.
pub fn rebuild_document_graph(store: &Store, doc: &Document) -> Result<(String, usize)> {
    store.with_tx(|tx| rebuild_document_graph_locked(tx, doc))
}

/// Transaction-aware graph rebuild used by atomic document writes.
pub(crate) fn rebuild_document_graph_locked(
    conn: &mut impl LinWriter,
    doc: &Document,
) -> Result<(String, usize)> {
    let node_id = ensure_document_node(conn, &doc.id, &doc.title, &doc.uri)?;
    delete_derived_edges_from_locked(conn, &node_id)?;

    // Obsidian markup is meaningful in prose, but `[[ ... ]]` is also ordinary
    // syntax in shell and generated source files. Parsing every source file
    // creates thousands of fake stubs such as `[[ -f "$path" ]]`.
    let mut links = if document_supports_knowledge_markup(doc) {
        extract_links_with(&doc.content, &extract_options())
    } else {
        Vec::new()
    };
    for metadata_link in metadata_tag_links(&doc.metadata_json) {
        if !links.iter().any(|link| {
            link.rel_type == metadata_link.rel_type && link.target_key == metadata_link.target_key
        }) {
            links.push(metadata_link);
        }
    }
    let chunks = load_chunk_spans(conn, &doc.id)?;
    let mut edges: Vec<DerivedEdge> = Vec::with_capacity(links.len() + 2);

    // Repeated links (`occurrence > 1`) resolve to the same node: each
    // distinct target costs its lookups once per rebuild.
    let mut resolved: std::collections::HashMap<(bool, &str), String> =
        std::collections::HashMap::new();
    for link in &links {
        let key = (link.rel_type == "tagged", link.target_label.as_str());
        let target_id = match resolved.get(&key) {
            Some(id) => id.clone(),
            None => {
                let id = resolve_target(conn, link)?;
                resolved.insert(key, id.clone());
                id
            }
        };
        let char_start = i64::try_from(link.char_start).unwrap_or(i64::MAX);
        let char_end = i64::try_from(link.char_end).unwrap_or(i64::MAX);
        edges.push(DerivedEdge {
            edge: GraphEdge {
                id: Uuid::new_v4().to_string(),
                source_id: node_id.clone(),
                target_id,
                rel_type: link.rel_type.clone(),
                weight: 1.0,
                context: link.context.clone(),
            },
            alias: link.alias.clone(),
            heading: link.heading.clone(),
            char_start: Some(char_start),
            char_end: Some(char_end),
            occurrence: i64::from(link.occurrence),
            chunk_id: chunk_id_for_span(&chunks, char_start, char_end),
        });
    }

    append_structural_edges(conn, doc, &node_id, &mut edges)?;

    let edge_count = edges.len();
    insert_graph_edges(conn, &edges)?;
    Ok((node_id, edge_count))
}

/// §4.5: `(chunk id, char_start, char_end)` for a document, in document order.
fn load_chunk_spans(r: &impl LinReader, document_id: &str) -> Result<Vec<(String, i64, i64)>> {
    let rows = r.find_by_eq("chunks", "document_id", document_id)?;
    let mut indexed: Vec<(i64, String, i64, i64)> = rows
        .iter()
        .map(|row| {
            (
                cell_i64(row, "chunk_index", 0),
                cell_string(row, "id"),
                cell_i64(row, "char_start", 0),
                cell_i64(row, "char_end", 0),
            )
        })
        .collect();
    indexed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    Ok(indexed
        .into_iter()
        .map(|(_, id, cs, ce)| (id, cs, ce))
        .collect())
}

/// Id of the chunk holding a link span (§4.5). A span that straddles a boundary
/// keeps the chunk its start falls in rather than reporting nothing.
fn chunk_id_for_span(chunks: &[(String, i64, i64)], start: i64, end: i64) -> Option<String> {
    if let Some((id, _, _)) = chunks.iter().find(|(_, cs, ce)| *cs <= start && end <= *ce) {
        return Some(id.clone());
    }
    chunks
        .iter()
        .find(|(_, cs, ce)| *cs <= start && start < *ce)
        .map(|(id, _, _)| id.clone())
}

fn metadata_tag_links(metadata_json: &str) -> Vec<ExtractedLink> {
    serde_json::from_str::<serde_json::Value>(metadata_json)
        .ok()
        .and_then(|metadata| {
            metadata
                .get("tags")
                .and_then(|tags| tags.as_array())
                .cloned()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|tag| tag.as_str().map(str::trim).map(str::to_string))
        .filter(|tag| !tag.is_empty())
        .map(|tag| ExtractedLink::from_metadata_tag(&tag))
        .collect()
}

static MAX_LINKS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

/// §4.2 rule 6: set the per-document link ceiling from `Config::max_links_per_doc`.
/// The first value wins; later calls are ignored so ingest sees one ceiling.
pub fn configure_max_links(max_links: usize) {
    let _ = MAX_LINKS.set(max_links.max(1));
}

fn extract_options() -> ExtractOptions {
    ExtractOptions {
        max_links: *MAX_LINKS.get_or_init(|| DEFAULT_MAX_LINKS_PER_DOC),
    }
}

fn document_supports_knowledge_markup(doc: &Document) -> bool {
    if Layer::Wiki.is(&doc.layer) || Layer::Diary.is(&doc.layer) {
        return true;
    }
    let Some(source_file) = doc.source_file.as_deref() else {
        // API-authored documents have no filesystem extension and historically
        // support wikilinks/tags; preserve that public behavior.
        return true;
    };
    let extension = Path::new(source_file)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "md" | "mdx" | "markdown" | "txt" | "rst" | "adoc" | "org"
    )
}

/// Attach filesystem-backed documents to stable project and directory nodes.
/// This keeps otherwise linkless source files inside a navigable hierarchy
/// without producing an O(n²) clique between sibling files.
fn append_structural_edges(
    conn: &mut impl LinWriter,
    doc: &Document,
    document_node_id: &str,
    edges: &mut Vec<DerivedEdge>,
) -> Result<()> {
    if let Some(project) = doc.wing.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let target_id =
            ensure_structural_node(conn, "project", project, &format!("project://{project}"))?;
        edges.push(structural_edge(
            document_node_id,
            target_id,
            crate::models::CONTEXT_PROJECT_MEMBERSHIP,
        ));
    }

    let Some(source_file) = doc
        .source_file
        .as_deref()
        .map(str::trim)
        .filter(|source| !source.is_empty())
    else {
        return Ok(());
    };
    let Some(parent) = Path::new(source_file).parent() else {
        return Ok(());
    };
    let directory = parent.to_string_lossy();
    if directory.is_empty() {
        return Ok(());
    }
    let label = parent
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(directory.as_ref());
    let target_id = ensure_structural_node(
        conn,
        "directory",
        label,
        &format!("directory://{directory}"),
    )?;
    edges.push(structural_edge(
        document_node_id,
        target_id,
        crate::models::CONTEXT_DIRECTORY_MEMBERSHIP,
    ));
    Ok(())
}

fn ensure_structural_node(
    conn: &mut impl LinWriter,
    structural_kind: &str,
    label: &str,
    uri: &str,
) -> Result<String> {
    if let Some(node) = find_node_by_uri(conn, uri)? {
        return Ok(node.id);
    }
    let node = GraphNode {
        id: Uuid::new_v4().to_string(),
        kind: "entity".into(),
        label: label.to_string(),
        document_id: None,
        uri: Some(uri.to_string()),
        resolved: true,
        metadata_json: serde_json::json!({"structural_kind": structural_kind}).to_string(),
    };
    let id = node.id.clone();
    upsert_graph_node(conn, &node)?;
    Ok(id)
}

/// Structural edges are generated from the document's placement, not from markup,
/// so they carry no span, alias or occurrence.
fn structural_edge(source_id: &str, target_id: String, context: &str) -> DerivedEdge {
    DerivedEdge {
        edge: GraphEdge {
            id: Uuid::new_v4().to_string(),
            source_id: source_id.to_string(),
            target_id,
            rel_type: "related".into(),
            weight: 0.5,
            context: Some(context.to_string()),
        },
        alias: None,
        heading: None,
        char_start: None,
        char_end: None,
        occurrence: 0,
        chunk_id: None,
    }
}

/// Ensure a resolved document node exists for `doc`; promote matching stubs by title/slug/uri.
///
/// §5.2 order: own document_id → own uri → promote a matching stub → create. Each
/// reuse keeps the existing node id so edges written before the document arrived
/// stay attached after promotion.
fn ensure_document_node(
    conn: &mut impl LinWriter,
    document_id: &str,
    title: &str,
    uri: &str,
) -> Result<String> {
    // Prefer existing node for this document id (stable across re-ingest with same doc id).
    if let Some(existing) = find_node_by_document_id(conn, document_id)? {
        return bind_as_document_node(conn, existing, document_id, title, uri);
    }

    // Stable by uri when node survived a content-only re-ingest path. A node
    // already owned by a *different* document is not a uri to reuse: taking it
    // would fuse two documents into one graph node (§5.2 "never steal").
    if !uri.is_empty() {
        if let Some(existing) = find_node_by_uri(conn, uri)? {
            if owned_by_other_document(&existing, document_id) {
                return create_document_node(conn, document_id, title, uri);
            }
            return bind_as_document_node(conn, existing, document_id, title, uri);
        }
    }

    // Promote stub whose label matches title, uri basename, or wiki slug
    // (so [[rag-mcp-overview]] stubs resolve when page title differs).
    let promote_labels = promote_label_candidates(title, uri);
    for label in &promote_labels {
        let matches = find_nodes_by_label(conn, label)?;
        if let Some(stub) = matches
            .into_iter()
            .find(|n| promotable_as_document(n, document_id))
        {
            return bind_as_document_node(conn, stub, document_id, title, uri);
        }
    }

    create_document_node(conn, document_id, title, uri)
}

/// Mark an existing node as the document node for `document_id`, keeping its id.
fn bind_as_document_node(
    conn: &mut impl LinWriter,
    mut node: GraphNode,
    document_id: &str,
    title: &str,
    uri: &str,
) -> Result<String> {
    node.kind = "document".into();
    node.label = title.to_string();
    node.document_id = Some(document_id.to_string());
    node.uri = Some(uri.to_string());
    node.resolved = true;
    let id = node.id.clone();
    upsert_graph_node(conn, &node)?;
    Ok(id)
}

fn create_document_node(
    conn: &mut impl LinWriter,
    document_id: &str,
    title: &str,
    uri: &str,
) -> Result<String> {
    let node = GraphNode {
        id: Uuid::new_v4().to_string(),
        kind: "document".into(),
        label: title.to_string(),
        document_id: Some(document_id.to_string()),
        uri: Some(uri.to_string()),
        resolved: true,
        metadata_json: "{}".into(),
    };
    let id = node.id.clone();
    upsert_graph_node(conn, &node)?;
    Ok(id)
}

/// True when the node is already the graph identity of a *different* document.
fn owned_by_other_document(node: &GraphNode, document_id: &str) -> bool {
    node.document_id
        .as_deref()
        .is_some_and(|owner| owner != document_id)
}

/// §5.2 step 3: only an unbound node may be promoted. A tag hub or a structural
/// entity carries its own identity, and a node already owned by another document
/// must not be stolen by title match.
fn promotable_as_document(node: &GraphNode, document_id: &str) -> bool {
    !(NodeKind::Tag.is(&node.kind) || NodeKind::Entity.is(&node.kind))
        && (NodeKind::Stub.is(&node.kind) || !node.resolved)
        && !owned_by_other_document(node, document_id)
}

fn promote_label_candidates(title: &str, uri: &str) -> Vec<String> {
    let mut labels = vec![title.to_string()];
    if let Some(base) = uri_basename(uri) {
        if base != title {
            labels.push(base);
        }
    }
    if let Some(slug) = wiki_slug_from_uri(uri) {
        if !labels.iter().any(|l| l == &slug) {
            labels.push(slug);
        }
    }
    labels
}

fn uri_basename(uri: &str) -> Option<String> {
    let path = uri.rsplit("://").next().unwrap_or(uri);
    let name = path.rsplit('/').next().unwrap_or(path);
    let name = name.trim();
    if name.is_empty() {
        None
    } else {
        // Strip common extension for title-like match.
        let stem = name
            .strip_suffix(".md")
            .or_else(|| name.strip_suffix(".txt"))
            .unwrap_or(name);
        Some(stem.to_string())
    }
}

fn resolve_target(conn: &mut impl LinWriter, link: &ExtractedLink) -> Result<String> {
    match link.rel_type.as_str() {
        "tagged" => upsert_tag_node(conn, &link.target_label),
        _ => resolve_wikilink_target(conn, &link.target_label),
    }
}

/// Find or create the canonical `tag://{tag_key}` hub node for `label`.
///
/// Every tag writer (extraction and maintenance `set_tags`) goes through this so
/// `#Alpha`, `alpha` and legacy `tag://{label}` nodes resolve to one hub.
pub(crate) fn upsert_tag_node(conn: &mut impl LinWriter, label: &str) -> Result<String> {
    let key = crate::graph::normalize::tag_key(label);
    let uri = format!("tag://{key}");
    if let Some(node) = find_node_by_uri(conn, &uri)? {
        return Ok(node.id);
    }
    // Tags written before §5.4 carry `tag://{label}` verbatim, so match those by
    // normalized label instead of minting a second hub for the same tag.
    if let Some(node) = find_nodes_by_label(conn, label)?
        .into_iter()
        .find(|n| NodeKind::Tag.is(&n.kind))
    {
        return Ok(node.id);
    }
    let node = GraphNode {
        id: Uuid::new_v4().to_string(),
        kind: "tag".into(),
        label: crate::graph::normalize::display_label(label),
        document_id: None,
        uri: Some(uri),
        resolved: true,
        metadata_json: "{}".into(),
    };
    let id = node.id.clone();
    upsert_graph_node(conn, &node)?;
    Ok(id)
}

/// Resolve `[[target]]` to a graph node.
///
/// Order (prefer resolved documents over stubs):
/// 1. Exact label match among graph nodes
/// 2. `wiki://{label}` / `wiki://{slugified}` node uri
/// 3. Document by those uris → its graph node
/// 4. `wiki_index.slug` → page document node
/// 5. Create a new stub (label only; uri left unset until page write)
fn resolve_wikilink_target(conn: &mut impl LinWriter, label: &str) -> Result<String> {
    let label = label.trim();
    if label.is_empty() {
        // Should not happen from extract; keep a stable empty stub.
        return upsert_stub(conn, "");
    }

    // 1. Label match on graph, document-only and unambiguous (§5.3).
    let matches = find_nodes_by_label(conn, label)?;
    if let Some(n) = pick_wikilink_node(&matches) {
        return Ok(n.id.clone());
    }

    // Candidate wiki URIs for slug-style links: [[rag-mcp-overview]] → wiki://rag-mcp-overview
    let mut uri_candidates = vec![format!("wiki://{label}")];
    let slugified = slugify_link(label);
    if slugified != label {
        uri_candidates.push(format!("wiki://{slugified}"));
    }
    // Bare filename links: [[SPINE_TOOLS.md]] already matched by label if title is that;
    // also try without extension as wiki slug.
    if let Some(stem) = label
        .strip_suffix(".md")
        .or_else(|| label.strip_suffix(".txt"))
    {
        if !stem.is_empty() {
            uri_candidates.push(format!("wiki://{stem}"));
        }
    }

    // 2. Node by uri
    for uri in &uri_candidates {
        if let Some(n) = find_node_by_uri(conn, uri)? {
            return Ok(n.id);
        }
    }

    // 3. Document by uri → ensure graph node for that document
    for uri in &uri_candidates {
        if let Some((id, title, uri)) = find_document_by_uri(conn, uri)? {
            return ensure_document_node(conn, &id, &title, &uri);
        }
    }

    // 4. wiki_index by slug (label or slugified)
    for slug in [label, slugified.as_str()] {
        if let Some(page_id) = find_wiki_index_page_id(conn, slug)? {
            if let Some((id, title, uri)) = find_document_by_id(conn, &page_id)? {
                return ensure_document_node(conn, &id, &title, &uri);
            }
        }
    }

    // 5. Stub
    upsert_stub(conn, label)
}

fn pick_wikilink_node(matches: &[GraphNode]) -> Option<&GraphNode> {
    let mut documents = matches
        .iter()
        .filter(|n| NodeKind::Document.is(&n.kind) && n.resolved);
    let document = documents.next()?;
    if documents.next().is_some() {
        return pick_stub_node(matches);
    }
    Some(document)
}

fn pick_stub_node(matches: &[GraphNode]) -> Option<&GraphNode> {
    matches
        .iter()
        .find(|n| NodeKind::Stub.is(&n.kind) || !n.resolved)
}

fn upsert_stub(conn: &mut impl LinWriter, label: &str) -> Result<String> {
    let matches = find_nodes_by_label(conn, label)?;
    if let Some(n) = pick_stub_node(&matches) {
        return Ok(n.id.clone());
    }
    let node = GraphNode {
        id: Uuid::new_v4().to_string(),
        kind: "stub".into(),
        label: label.to_string(),
        document_id: None,
        uri: None,
        resolved: false,
        metadata_json: "{}".into(),
    };
    let id = node.id.clone();
    upsert_graph_node(conn, &node)?;
    Ok(id)
}

fn find_document_by_uri(r: &impl LinReader, uri: &str) -> Result<Option<(String, String, String)>> {
    let rows = r.find_by_eq("documents", "uri", uri)?;
    Ok(rows.first().map(|row| {
        (
            cell_string(row, "id"),
            cell_string(row, "title"),
            cell_string(row, "uri"),
        )
    }))
}

fn find_document_by_id(
    r: &impl LinReader,
    document_id: &str,
) -> Result<Option<(String, String, String)>> {
    Ok(r.get_by_id("documents", document_id)?.map(|row| {
        (
            cell_string(&row, "id"),
            cell_string(&row, "title"),
            cell_string(&row, "uri"),
        )
    }))
}

fn find_wiki_index_page_id(r: &impl LinReader, slug: &str) -> Result<Option<String>> {
    if let Some(row) = r.get_by_id("wiki_index", slug)? {
        return Ok(cell_nonempty_string(&row, "page_id")
            .or_else(|| cell_nonempty_string(&row, "document_id")));
    }
    let mut found: Option<String> = None;
    r.scan_each("wiki_index", |row| {
        if cell_str(row, "slug") == Some(slug)
            || cell_str(row, "label") == Some(slug)
            || cell_str(row, "id") == Some(slug)
        {
            found = cell_nonempty_string(row, "page_id")
                .or_else(|| cell_nonempty_string(row, "document_id"));
            return Ok(false);
        }
        Ok(true)
    })?;
    Ok(found)
}

/// Light slug for link targets (keep dots for file names; collapse spaces).
fn slugify_link(s: &str) -> String {
    slugify(s, SlugPolicy::LinkTarget)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn open_temp() -> Store {
        crate::testkit::temp_store()
    }

    fn doc(id: &str, title: &str, uri: &str, content: &str) -> Document {
        let now = Utc::now();
        Document {
            id: id.into(),
            uri: uri.into(),
            title: title.into(),
            content: content.into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            ..Default::default()
        }
    }

    #[test]
    fn rebuild_creates_doc_stub_and_tag() {
        let store = open_temp();
        let d = doc(
            "d1",
            "Home",
            "doc://home",
            "See [[Other Note]] and #inbox stuff.",
        );
        store.upsert_document(&d).unwrap();

        let (node_id, edge_count) = rebuild_document_graph(&store, &d).unwrap();
        assert!(!node_id.is_empty());
        assert_eq!(edge_count, 2);

        let node = store.find_node_by_document_id("d1").unwrap().expect("node");
        assert_eq!(node.id, node_id);
        assert_eq!(node.kind, "document");
        assert!(node.resolved);

        let stubs = store.find_nodes_by_label("Other Note").unwrap();
        assert_eq!(stubs.len(), 1);
        assert_eq!(stubs[0].kind, "stub");
        assert!(!stubs[0].resolved);

        let tags = store.find_nodes_by_label("inbox").unwrap();
        assert_eq!(tags[0].kind, "tag");
    }

    #[test]
    fn rebuild_preserves_explicit_edges_even_when_their_type_is_extractable() {
        let store = open_temp();
        let mut source = doc("source", "Source", "doc://source", "[[Old target]] #oldtag");
        let target = doc("target", "Target", "doc://target", "Target body");
        store.upsert_document(&source).unwrap();
        store.upsert_document(&target).unwrap();
        let (source_node, _) = rebuild_document_graph(&store, &source).unwrap();
        let (target_node, _) = rebuild_document_graph(&store, &target).unwrap();
        let manual = ["related", "tunnel", "wikilink", "tagged"].map(|relation| {
            store
                .link_nodes(&source_node, &target_node, relation, 1.0)
                .unwrap()
                .id
        });
        source.content = "[[New target]]".into();
        store.upsert_document(&source).unwrap();
        for _ in 0..2 {
            rebuild_document_graph(&store, &source).unwrap();
        }
        let r = store.read().unwrap();
        for id in manual {
            let row = r.get_by_id("graph_edges", &id).unwrap().expect("manual edge");
            assert_eq!(cell_str(&row, "origin"), Some("explicit"));
        }
        let derived = r
            .find_by_eq("graph_edges", "source_id", source_node.as_str())
            .unwrap()
            .into_iter()
            .filter(|row| cell_str(row, "origin") == Some("extract"))
            .count();
        assert_eq!(
            derived, 1,
            "rebuild replaces rather than duplicates extracted edges"
        );
    }

    /// §4.1/§6.1 step 5: an extracted edge carries the span, alias, heading,
    /// occurrence and containing chunk it was read from, while an explicit
    /// `link_nodes` edge keeps those columns NULL — it has no source span.
    #[test]
    fn rebuild_records_link_provenance_and_explicit_edges_stay_bare() {
        let store = open_temp();
        let body = "Intro [[Target#Chapter|see]].";
        let source = doc("src", "Src", "doc://src", body);
        store.upsert_document(&source).unwrap();
        store
            .insert_chunks(&[
                chunk("c-head", "src", 0, "Intro ", 0, 6),
                chunk("c-link", "src", 1, "[[Target#Chapter|see]].", 6, 29),
            ])
            .unwrap();

        let (node_id, _) = rebuild_document_graph(&store, &source).unwrap();

        #[derive(Debug)]
        struct Row {
            rel: String,
            origin: Option<String>,
            alias: Option<String>,
            heading: Option<String>,
            chunk_id: Option<String>,
            char_start: Option<i64>,
            char_end: Option<i64>,
            occurrence: Option<i64>,
        }
        let rows = {
            let r = store.read().unwrap();
            let mut found: Vec<Row> = r
                .find_by_eq("graph_edges", "source_id", node_id.as_str())
                .unwrap()
                .iter()
                .map(|row| Row {
                    rel: cell_string(row, "rel_type"),
                    origin: crate::db::lin_engine::cell_opt_string(row, "origin"),
                    alias: crate::db::lin_engine::cell_opt_string(row, "alias"),
                    heading: crate::db::lin_engine::cell_opt_string(row, "heading"),
                    chunk_id: crate::db::lin_engine::cell_opt_string(row, "chunk_id"),
                    char_start: crate::db::lin_engine::cell_i64_opt(row, "char_start"),
                    char_end: crate::db::lin_engine::cell_i64_opt(row, "char_end"),
                    occurrence: crate::db::lin_engine::cell_i64_opt(row, "occurrence"),
                })
                .collect();
            found.sort_by(|a, b| a.rel.cmp(&b.rel));
            found
        };

        let wikilink = rows
            .iter()
            .find(|r| r.rel == "wikilink")
            .expect("wikilink edge");
        assert_eq!(wikilink.origin.as_deref(), Some("extract"));
        assert_eq!(wikilink.alias.as_deref(), Some("see"));
        assert_eq!(wikilink.heading.as_deref(), Some("Chapter"));
        assert_eq!(wikilink.chunk_id.as_deref(), Some("c-link"));
        assert_eq!(wikilink.char_start, Some(6));
        assert_eq!(wikilink.char_end, Some(28));
        assert_eq!(wikilink.occurrence, Some(0));

        let stub_id = store
            .find_nodes_by_label("Target")
            .unwrap()
            .into_iter()
            .find(|n| n.kind == "stub")
            .expect("stub for the link target")
            .id;
        store
            .link_nodes(&node_id, &stub_id, "related", 1.0)
            .unwrap();
        let explicit = {
            let r = store.read().unwrap();
            r.find_by_eq("graph_edges", "source_id", node_id.as_str())
                .unwrap()
                .into_iter()
                .filter(|row| cell_str(row, "rel_type") == Some("related"))
                .map(|row| {
                    (
                        crate::db::lin_engine::cell_opt_string(&row, "origin"),
                        crate::db::lin_engine::cell_opt_string(&row, "alias"),
                        crate::db::lin_engine::cell_opt_string(&row, "heading"),
                        crate::db::lin_engine::cell_opt_string(&row, "chunk_id"),
                        crate::db::lin_engine::cell_i64_opt(&row, "char_start"),
                        crate::db::lin_engine::cell_i64_opt(&row, "char_end"),
                        crate::db::lin_engine::cell_i64_opt(&row, "occurrence"),
                    )
                })
                .collect::<Vec<_>>()
        }
        .remove(0);
        assert_eq!(explicit.0.as_deref(), Some("explicit"));
        assert!(
            explicit.1.is_none()
                && explicit.2.is_none()
                && explicit.3.is_none()
                && explicit.4.is_none()
                && explicit.5.is_none()
                && explicit.6.is_none(),
            "explicit edges carry no invented span: {explicit:?}"
        );
    }

    fn chunk(
        id: &str,
        document_id: &str,
        index: i32,
        content: &str,
        char_start: i32,
        char_end: i32,
    ) -> crate::models::Chunk {
        crate::models::Chunk {
            id: id.into(),
            document_id: document_id.into(),
            chunk_index: index,
            content: content.into(),
            embedding: vec![0.0; 8],
            char_start,
            char_end,
            metadata_json: "{}".into(),
        }
    }

    /// A database upgraded from the pre-§1.6 vocabulary has no `origin`; the
    /// migrate backfill has to claim extraction-owned relations as `extract` and
    /// leave everything else on the side rebuild never deletes.
    #[test]
    fn legacy_edges_are_attributed_by_the_origin_backfill() {
        let store = open_temp();
        let source = doc("source", "Source", "doc://source", "Updated [[Old target]]");
        store.upsert_document(&source).unwrap();
        let (node, _) = rebuild_document_graph(&store, &source).unwrap();
        let target = store.find_nodes_by_label("Old target").unwrap().remove(0);
        let explicit = store.link_nodes(&node, &target.id, "related", 1.0).unwrap();
        {
            let mut w = store.write().unwrap();
            let all_ids: Vec<String> = w
                .filter_map_rows("graph_edges", |r| Ok(Some(cell_string(r, "id"))))
                .unwrap();
            for eid in all_ids {
                w.update_where(
                    "graph_edges",
                    lin::query::pred::eq("id", eid.as_str()),
                    crate::db::lin_engine::RecBuilder::new()
                        .opt_str("origin", None)
                        .build(),
                )
                .unwrap();
            }
        }

        {
            let mut conn = store.write().unwrap();
            let attributed = crate::db::schema::backfill_graph_edge_origins(&mut conn).unwrap();
            assert_eq!(attributed, 2, "both legacy rows gain an owner");
        }

        rebuild_document_graph(&store, &source).unwrap();
        let counts = {
            let r = store.read().unwrap();
            let rows = r.find_by_eq("graph_edges", "source_id", node.as_str()).unwrap();
            let ext = rows
                .iter()
                .filter(|row| cell_str(row, "origin") == Some("extract"))
                .count() as i64;
            let exp = rows
                .iter()
                .filter(|row| cell_str(row, "origin") == Some("explicit"))
                .count() as i64;
            (ext, exp)
        };
        assert_eq!(
            counts,
            (1, 1),
            "the wikilink is re-extracted once while the explicit edge survives"
        );
        let remaining = store.list_graph_edges().unwrap();
        assert_eq!(remaining.len(), 2);
        assert!(remaining.iter().any(|edge| edge.id == explicit.id));
    }

    #[test]
    fn legacy_membership_edges_are_attributed_by_their_structural_context() {
        let store = open_temp();
        let mut source = doc("d-legacy", "Legacy", "file:///notes/legacy.md", "Body");
        source.wing = Some("notes".into());
        source.source_file = Some("/notes/legacy.md".into());
        store.upsert_document(&source).unwrap();
        let (node, _) = rebuild_document_graph(&store, &source).unwrap();
        let project = store
            .find_node_by_uri("project://notes")
            .unwrap()
            .expect("project node");
        let hand_made = store
            .link_nodes(&node, &project.id, "related", 1.0)
            .unwrap();

        {
            let mut w = store.write().unwrap();
            let all_ids: Vec<String> = w
                .filter_map_rows("graph_edges", |r| Ok(Some(cell_string(r, "id"))))
                .unwrap();
            for eid in all_ids {
                w.update_where(
                    "graph_edges",
                    lin::query::pred::eq("id", eid.as_str()),
                    crate::db::lin_engine::RecBuilder::new()
                        .opt_str("origin", None)
                        .opt_str("edge_origin", None)
                        .build(),
                )
                .unwrap();
            }
        }
        let attributed = {
            let mut conn = store.write().unwrap();
            crate::db::schema::backfill_graph_edge_origins(&mut conn).unwrap()
        };
        assert_eq!(attributed, 3, "two membership edges plus the hand-made one");

        let owners = || {
            let r = store.read().unwrap();
            let mut ext = 0i64;
            let mut exp = 0i64;
            r.scan_each("graph_edges", |row| {
                match cell_str(row, "origin") {
                    Some("extract") => ext += 1,
                    Some("explicit") => exp += 1,
                    _ => {}
                }
                Ok(true)
            })
            .unwrap();
            (ext, exp)
        };
        assert_eq!(
            owners(),
            (2, 1),
            "only the context-free hand-made edge stays explicit"
        );

        rebuild_document_graph(&store, &source).unwrap();
        assert_eq!(
            owners(),
            (2, 1),
            "the rebuild re-creates the membership edges in place instead of duplicating them"
        );
        let survives = {
            let r = store.read().unwrap();
            r.get_by_id("graph_edges", &hand_made.id)
                .unwrap()
                .filter(|row| cell_str(row, "origin") == Some("explicit"))
                .map_or(0i64, |_| 1)
        };
        assert_eq!(
            survives, 1,
            "a hand-made `related` edge outlives the rebuild"
        );
    }

    #[test]
    fn re_ingest_keeps_stable_node_id() {
        let store = open_temp();
        let d1 = doc("d1", "Home", "doc://home", "Link [[X]]");
        store.upsert_document(&d1).unwrap();
        let (id1, _) = rebuild_document_graph(&store, &d1).unwrap();

        let d2 = doc("d1", "Home", "doc://home", "Link [[Y]] #t");
        store.upsert_document(&d2).unwrap();
        let (id2, edges) = rebuild_document_graph(&store, &d2).unwrap();
        assert_eq!(id1, id2);
        assert_eq!(edges, 2);
    }

    #[test]
    fn equal_titles_with_different_uris_keep_distinct_nodes() {
        let store = open_temp();
        let first = doc("d1", "README.md", "file:///project-a/README.md", "A");
        let second = doc("d2", "README.md", "file:///project-b/README.md", "B");
        store.upsert_document(&first).unwrap();
        store.upsert_document(&second).unwrap();

        let (first_node, _) = rebuild_document_graph(&store, &first).unwrap();
        let (second_node, _) = rebuild_document_graph(&store, &second).unwrap();

        assert_ne!(first_node, second_node);
        assert_eq!(
            store.find_node_by_document_id("d1").unwrap().unwrap().id,
            first_node
        );
        assert_eq!(
            store.find_node_by_document_id("d2").unwrap().unwrap().id,
            second_node
        );
    }

    #[test]
    fn filesystem_document_is_linked_to_project_and_directory() {
        let store = open_temp();
        let mut source = doc("d1", "tab.rs", "file:///work/rag/src/tab.rs", "");
        source.wing = Some("rag".into());
        source.source_file = Some("/work/rag/src/tab.rs".into());
        store.upsert_document(&source).unwrap();

        let (document_node_id, edge_count) = rebuild_document_graph(&store, &source).unwrap();
        assert_eq!(edge_count, 2);

        let project = store
            .find_node_by_uri("project://rag")
            .unwrap()
            .expect("project node");
        let directory = store
            .find_node_by_uri("directory:///work/rag/src")
            .unwrap()
            .expect("directory node");
        assert_eq!(project.kind, "entity");
        assert_eq!(directory.label, "src");

        let edges = store.list_graph_edges().unwrap();
        assert!(edges.iter().any(|edge| {
            edge.source_id == document_node_id
                && edge.target_id == project.id
                && edge.rel_type == "related"
        }));
        assert!(edges.iter().any(|edge| {
            edge.source_id == document_node_id
                && edge.target_id == directory.id
                && edge.rel_type == "related"
        }));

        let (_, rebuilt_count) = rebuild_document_graph(&store, &source).unwrap();
        assert_eq!(rebuilt_count, 2);
        assert_eq!(store.list_graph_edges().unwrap().len(), 2);
    }

    #[test]
    fn source_code_brackets_do_not_create_fake_wikilink_stubs() {
        let store = open_temp();
        let mut source = doc(
            "shell",
            "check.sh",
            "file:///work/check.sh",
            "if [[ ! -f \"$path\" ]]; then exit 1; fi",
        );
        source.wing = Some("project".into());
        source.source_file = Some("/work/check.sh".into());
        store.upsert_document(&source).unwrap();

        let (_, edge_count) = rebuild_document_graph(&store, &source).unwrap();
        assert_eq!(edge_count, 2, "only project and directory edges remain");
        assert!(store
            .find_nodes_by_label("! -f \"$path\"")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn markdown_source_files_still_create_wikilinks() {
        let store = open_temp();
        let mut source = doc(
            "readme",
            "README.md",
            "file:///work/README.md",
            "See [[Architecture]]",
        );
        source.wing = Some("project".into());
        source.source_file = Some("/work/README.md".into());
        store.upsert_document(&source).unwrap();

        let (_, edge_count) = rebuild_document_graph(&store, &source).unwrap();
        assert_eq!(edge_count, 3);
        assert_eq!(store.find_nodes_by_label("Architecture").unwrap().len(), 1);
    }

    #[test]
    fn promote_stub_on_title_match() {
        let store = open_temp();
        let a = doc("da", "A", "doc://a", "See [[B]]");
        store.upsert_document(&a).unwrap();
        rebuild_document_graph(&store, &a).unwrap();

        let stub = store.find_nodes_by_label("B").unwrap();
        assert_eq!(stub[0].kind, "stub");
        let stub_id = stub[0].id.clone();

        let b = doc("db", "B", "doc://b", "I am B");
        store.upsert_document(&b).unwrap();
        let (node_id, _) = rebuild_document_graph(&store, &b).unwrap();
        assert_eq!(node_id, stub_id);
        let promoted = store.find_node_by_id(&stub_id).unwrap().unwrap();
        assert_eq!(promoted.kind, "document");
        assert!(promoted.resolved);
        assert_eq!(promoted.document_id.as_deref(), Some("db"));

        // The backlink written before the page existed must still point at it.
        let incoming = store
            .list_graph_edges()
            .unwrap()
            .into_iter()
            .filter(|e| e.target_id == stub_id)
            .collect::<Vec<_>>();
        assert_eq!(incoming.len(), 1, "promotion keeps the incoming wikilink");
    }

    /// §5.3: `[[inbox]]` names a page, not the `#inbox` hub, so it must land on a
    /// stub. Binding tag nodes made every popular tag look like a document.
    #[test]
    fn wikilink_never_binds_a_tag_hub() {
        let store = open_temp();
        let tagged = doc("tagged", "Tagged", "doc://tagged", "triage #inbox");
        store.upsert_document(&tagged).unwrap();
        rebuild_document_graph(&store, &tagged).unwrap();

        let linker = doc("linker", "Linker", "doc://linker", "See [[inbox]].");
        store.upsert_document(&linker).unwrap();
        let (node_id, _) = rebuild_document_graph(&store, &linker).unwrap();

        let edge = store
            .list_graph_edges()
            .unwrap()
            .into_iter()
            .find(|e| e.source_id == node_id && e.rel_type == "wikilink")
            .expect("wikilink edge");
        let target = store.find_node_by_id(&edge.target_id).unwrap().unwrap();
        assert_eq!(target.kind, "stub");
        assert!(!target.resolved);
        // One hub per tag, and the stub is a separate node sharing its label_key.
        assert_eq!(
            store
                .find_nodes_by_label("inbox")
                .unwrap()
                .iter()
                .filter(|n| n.kind == "tag")
                .count(),
            1
        );
    }

    /// §5.3 step 2: several documents under one `label_key` is ambiguity. The
    /// linker stays on an unresolved stub instead of silently picking a winner.
    #[test]
    fn ambiguous_label_binds_neither_of_two_same_titled_documents() {
        let store = open_temp();
        let first = doc("first", "Duplicate", "file:///one/Duplicate.md", "A body");
        let second = doc("second", "Duplicate", "file:///two/Duplicate.md", "B body");
        for d in [&first, &second] {
            store.upsert_document(d).unwrap();
            rebuild_document_graph(&store, d).unwrap();
        }
        let documents = store
            .find_nodes_by_label("Duplicate")
            .unwrap()
            .into_iter()
            .filter(|n| n.kind == "document")
            .map(|n| n.id)
            .collect::<Vec<_>>();
        assert_eq!(documents.len(), 2, "equal titles keep distinct nodes");

        let linker = doc("linker", "Linker", "doc://linker", "See [[Duplicate]].");
        store.upsert_document(&linker).unwrap();
        let (node_id, _) = rebuild_document_graph(&store, &linker).unwrap();
        let edge = store
            .list_graph_edges()
            .unwrap()
            .into_iter()
            .find(|e| e.source_id == node_id && e.rel_type == "wikilink")
            .expect("wikilink edge");
        assert!(!documents.contains(&edge.target_id));
        let target = store.find_node_by_id(&edge.target_id).unwrap().unwrap();
        assert_eq!(target.kind, "stub");

        // Rebuild is idempotent: the ambiguity stub is reused, not minted again.
        rebuild_document_graph(&store, &linker).unwrap();
        assert_eq!(
            store
                .list_graph_edges()
                .unwrap()
                .into_iter()
                .filter(|e| e.source_id == node_id && e.rel_type == "wikilink")
                .count(),
            1
        );
    }

    /// §5.2 step 2: uri reuse is for the *same* document surviving a re-ingest.
    /// Taking another document's node would fuse two identities into one.
    #[test]
    fn uri_owned_by_another_document_is_not_stolen() {
        let store = open_temp();
        let (first, second, first_node) = {
            let mut conn = store.write().unwrap();
            let first = ensure_document_node(&mut conn, "doc-a", "First", "wiki://shared").unwrap();
            let second =
                ensure_document_node(&mut conn, "doc-b", "Second", "wiki://shared").unwrap();
            // The original owner is untouched by the second document's arrival.
            let first_node = crate::db::graph::find_node_by_document_id_locked(&conn, "doc-a")
                .unwrap()
                .unwrap();
            (first, second, first_node)
        };
        assert_ne!(first, second, "one uri is not the identity of two docs");
        assert_eq!(first_node.id, first);
        assert_eq!(first_node.document_id.as_deref(), Some("doc-a"));
        assert_eq!(first_node.label, "First");
    }

    /// §5.2 step 3: promotion may consume a stub, never a tag hub or a structural
    /// entity that happens to carry the document's title.
    #[test]
    fn promote_skips_tag_and_structural_entity_nodes() {
        let store = open_temp();
        {
            let mut conn = store.write().unwrap();
            for (kind, label, uri) in [
                ("tag", "Deploy Runbook", "tag://deploy-runbook"),
                ("entity", "Deploy Runbook", "directory:///deploy"),
            ] {
                crate::db::graph::upsert_graph_node_locked(
                    &mut conn,
                    &GraphNode {
                        id: Uuid::new_v4().to_string(),
                        kind: kind.into(),
                        label: label.into(),
                        document_id: None,
                        uri: Some(uri.into()),
                        resolved: kind == "tag",
                        metadata_json: "{}".into(),
                    },
                )
                .unwrap();
            }
        }

        let doc_row = doc("runbook", "Deploy Runbook", "wiki://deploy-runbook", "body");
        store.upsert_document(&doc_row).unwrap();
        let (node_id, _) = rebuild_document_graph(&store, &doc_row).unwrap();
        let nodes = store.find_nodes_by_label("Deploy Runbook").unwrap();
        let promoted = nodes.iter().find(|n| n.id == node_id).unwrap();
        assert_eq!(promoted.kind, "document");
        assert_eq!(
            nodes.iter().filter(|n| n.kind == "document").count(),
            1,
            "the tag and entity hubs keep their own nodes"
        );
    }

    #[test]
    fn wikilink_resolves_by_slug_when_title_differs() {
        // Page exists first with title ≠ slug; later page links via [[slug]].
        let store = open_temp();
        let target = doc(
            "t1",
            "rag-mcp: обзор проекта",
            "wiki://rag-mcp-overview",
            "# Overview",
        );
        store.upsert_document(&target).unwrap();
        rebuild_document_graph(&store, &target).unwrap();

        let linker = doc(
            "l1",
            "Architecture",
            "wiki://rag-mcp-architecture",
            "See [[rag-mcp-overview]] for context.",
        );
        store.upsert_document(&linker).unwrap();
        let (_, edges) = rebuild_document_graph(&store, &linker).unwrap();
        assert_eq!(edges, 1);

        let stubs = store.find_nodes_by_label("rag-mcp-overview").unwrap();
        // Should not leave a stub; edge points at resolved document node.
        let overview = store
            .find_node_by_document_id("t1")
            .unwrap()
            .expect("overview node");
        assert!(overview.resolved);
        assert_eq!(overview.kind, "document");
        // No unresolved stub with that label, or if label match exists it is the document.
        if let Some(n) = stubs.first() {
            assert!(n.resolved || n.kind == "document");
            assert_eq!(n.id, overview.id);
        }
    }

    #[test]
    fn promote_stub_by_wiki_slug_when_title_differs() {
        // Stub created first via [[slug]]; page written later with different title.
        let store = open_temp();
        let a = doc("da", "Arch", "wiki://arch", "See [[rag-mcp-overview]]");
        store.upsert_document(&a).unwrap();
        rebuild_document_graph(&store, &a).unwrap();
        let stub = store.find_nodes_by_label("rag-mcp-overview").unwrap();
        assert_eq!(stub[0].kind, "stub");
        let stub_id = stub[0].id.clone();

        let page = doc(
            "db",
            "rag-mcp: обзор проекта",
            "wiki://rag-mcp-overview",
            "Body",
        );
        store.upsert_document(&page).unwrap();
        let (node_id, _) = rebuild_document_graph(&store, &page).unwrap();
        assert_eq!(node_id, stub_id);
        let promoted = store.find_node_by_id(&stub_id).unwrap().unwrap();
        assert!(promoted.resolved);
        assert_eq!(promoted.kind, "document");
    }
}
