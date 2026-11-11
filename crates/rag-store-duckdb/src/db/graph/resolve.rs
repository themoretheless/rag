//! Resolve extracted links into graph nodes/edges for a document.

use std::path::Path;

use duckdb::{params, Connection};
use uuid::Uuid;

use crate::db::graph::{
    delete_derived_edges_from_locked, find_node_by_document_id_locked as find_node_by_document_id,
    find_node_by_uri_locked as find_node_by_uri, find_nodes_by_label_locked as find_nodes_by_label,
    insert_derived_graph_edges_locked as insert_graph_edges,
    upsert_graph_node_locked as upsert_graph_node, DerivedEdge,
};
use crate::db::Store;
use crate::error::Result;
use crate::models::{Document, GraphEdge, GraphNode, Layer, NodeKind};
use crate::util::{slugify, wiki_slug_from_uri, SlugPolicy};
use rag_mcp_markup::extract::{
    extract_links_with, ExtractOptions, ExtractedLink, DEFAULT_MAX_LINKS_PER_DOC,
};

/// Rebuild the object-graph slice for `doc`.
///
/// 1. Upsert (or promote stub for) the document node — stable id by `document_id` / `uri`.
/// 2. Replace extracted outgoing edges, preserving explicit user/dependency links.
/// 3. Extract wikilinks + tags; resolve targets by label, slug, or `wiki://` uri; write edges.
///
/// Returns `(node_id, edge_count)` for the document node and edges written this pass.
pub fn rebuild_document_graph(store: &Store, doc: &Document) -> Result<(String, usize)> {
    let mut conn = store.lock()?;
    let tx = conn.transaction()?;
    let result = rebuild_document_graph_locked(&tx, doc)?;
    tx.commit()?;
    Ok(result)
}

/// Transaction-aware graph rebuild used by atomic document writes.
///
/// `duckdb::Transaction` dereferences to [`Connection`], so callers can run the
/// exact same resolution logic under their surrounding document/chunk transaction.
pub(crate) fn rebuild_document_graph_locked(
    conn: &Connection,
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
fn load_chunk_spans(conn: &Connection, document_id: &str) -> Result<Vec<(String, i64, i64)>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, char_start, char_end
        FROM chunks
        WHERE document_id = ?
        ORDER BY chunk_index ASC
        "#,
    )?;
    let spans = crate::db::rows::query_vec(&mut stmt, params![document_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    Ok(spans)
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
    conn: &Connection,
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
    conn: &Connection,
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
    conn: &Connection,
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
    conn: &Connection,
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
    conn: &Connection,
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

fn resolve_target(conn: &Connection, link: &ExtractedLink) -> Result<String> {
    match link.rel_type.as_str() {
        "tagged" => upsert_tag_node(conn, &link.target_label),
        _ => resolve_wikilink_target(conn, &link.target_label),
    }
}

/// Find or create the canonical `tag://{tag_key}` hub node for `label`.
///
/// Every tag writer (extraction and maintenance `set_tags`) goes through this so
/// `#Alpha`, `alpha` and legacy `tag://{label}` nodes resolve to one hub.
pub(crate) fn upsert_tag_node(conn: &Connection, label: &str) -> Result<String> {
    let key = rag_mcp_markup::normalize::tag_key(label);
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
        label: rag_mcp_markup::normalize::display_label(label),
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
fn resolve_wikilink_target(conn: &Connection, label: &str) -> Result<String> {
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
    // §5.3: a wikilink names a page, so it binds to a *unique* resolved document
    // node, or to an unresolved stub. Tag hubs and structural entities are never
    // wikilink targets — binding [[inbox]] to the #inbox hub would make a tag
    // look like a note.
    let mut documents = matches
        .iter()
        .filter(|n| NodeKind::Document.is(&n.kind) && n.resolved);
    let document = documents.next()?;
    if documents.next().is_some() {
        // Two documents share one label_key: that is ambiguity, not a match. Stay
        // on a stub and let `link_health` surface it instead of picking a winner.
        return pick_stub_node(matches);
    }
    Some(document)
}

/// Existing promotion target for a wikilink: an unresolved stub-like node. Both
/// tag and structural-entity nodes carry `resolved = true`, so they never match.
fn pick_stub_node(matches: &[GraphNode]) -> Option<&GraphNode> {
    matches
        .iter()
        .find(|n| NodeKind::Stub.is(&n.kind) || !n.resolved)
}

fn upsert_stub(conn: &Connection, label: &str) -> Result<String> {
    let matches = find_nodes_by_label(conn, label)?;
    if let Some(n) = pick_stub_node(&matches) {
        return Ok(n.id.clone());
    }
    // A label that only matches resolved documents is an ambiguous or mistyped
    // target: mint an unbound stub for the link rather than borrowing that node.
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

fn find_document_by_uri(conn: &Connection, uri: &str) -> Result<Option<(String, String, String)>> {
    find_document_node_data(
        conn,
        "SELECT id, title, uri FROM documents WHERE uri = ? LIMIT 1",
        uri,
    )
}

fn find_document_by_id(
    conn: &Connection,
    document_id: &str,
) -> Result<Option<(String, String, String)>> {
    find_document_node_data(
        conn,
        "SELECT id, title, uri FROM documents WHERE id = ? LIMIT 1",
        document_id,
    )
}

fn find_document_node_data(
    conn: &Connection,
    sql: &str,
    value: &str,
) -> Result<Option<(String, String, String)>> {
    let mut stmt = conn.prepare_cached(sql)?;
    let mut rows = stmt.query(params![value])?;
    match rows.next()? {
        Some(row) => Ok(Some((row.get(0)?, row.get(1)?, row.get(2)?))),
        None => Ok(None),
    }
}

fn find_wiki_index_page_id(conn: &Connection, slug: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT COALESCE(page_id, document_id)
        FROM wiki_index
        WHERE slug = ? OR label = ? OR id = ?
        LIMIT 1
        "#,
    )?;
    let mut rows = stmt.query(params![slug, slug, slug])?;
    match rows.next()? {
        Some(row) => Ok(row.get(0)?),
        None => Ok(None),
    }
}

/// Light slug for link targets (keep dots for file names; collapse spaces).
fn slugify_link(s: &str) -> String {
    slugify(s, SlugPolicy::LinkTarget)
}

#[cfg(test)]
mod tests;
