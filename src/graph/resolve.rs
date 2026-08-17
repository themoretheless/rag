//! Resolve extracted links into graph nodes/edges for a document.

use uuid::Uuid;

use crate::db::Store;
use crate::error::Result;
use crate::graph::extract::{extract_links, ExtractedLink};
use crate::models::{Document, GraphEdge, GraphNode};
use crate::util::wiki_slug_from_uri;

/// Rebuild the object-graph slice for `doc`.
///
/// 1. Upsert (or promote stub for) the document node — stable id by `document_id` / `uri` / title / slug.
/// 2. Delete existing outgoing edges from that node.
/// 3. Extract wikilinks + tags; resolve targets by label, slug, or `wiki://` uri; write edges.
///
/// Returns `(node_id, edge_count)` for the document node and edges written this pass.
pub fn rebuild_document_graph(store: &Store, doc: &Document) -> Result<(String, usize)> {
    let node_id = ensure_document_node(store, doc)?;
    store.delete_edges_from(&node_id)?;

    let links = extract_links(&doc.content);
    let mut edges: Vec<GraphEdge> = Vec::with_capacity(links.len());

    for link in &links {
        let target_id = resolve_target(store, link)?;
        edges.push(GraphEdge {
            id: Uuid::new_v4().to_string(),
            source_id: node_id.clone(),
            target_id,
            rel_type: link.rel_type.clone(),
            weight: 1.0,
            context: link.context.clone(),
        });
    }

    let edge_count = edges.len();
    store.insert_graph_edges(&edges)?;
    Ok((node_id, edge_count))
}

/// Ensure a resolved document node exists for `doc`; promote matching stubs by title/slug/uri.
fn ensure_document_node(store: &Store, doc: &Document) -> Result<String> {
    // Prefer existing node for this document id (stable across re-ingest with same doc id).
    if let Some(existing) = store.find_node_by_document_id(&doc.id)? {
        let mut node = existing;
        node.kind = "document".into();
        node.label = doc.title.clone();
        node.document_id = Some(doc.id.clone());
        node.uri = Some(doc.uri.clone());
        node.resolved = true;
        store.upsert_graph_node(&node)?;
        return Ok(node.id);
    }

    // Stable by uri when node survived a content-only re-ingest path.
    if !doc.uri.is_empty() {
        if let Some(existing) = store.find_node_by_uri(&doc.uri)? {
            let mut node = existing;
            node.kind = "document".into();
            node.label = doc.title.clone();
            node.document_id = Some(doc.id.clone());
            node.uri = Some(doc.uri.clone());
            node.resolved = true;
            store.upsert_graph_node(&node)?;
            return Ok(node.id);
        }
    }

    // Promote stub whose label matches title, uri basename, or wiki slug
    // (so [[rag-mcp-overview]] stubs resolve when page title differs).
    let promote_labels = promote_label_candidates(doc);
    for label in &promote_labels {
        let matches = store.find_nodes_by_label(label)?;
        if let Some(stub) = matches.into_iter().find(|n| n.kind == "stub" || !n.resolved)
        {
            let mut node = stub;
            node.kind = "document".into();
            node.label = doc.title.clone();
            node.document_id = Some(doc.id.clone());
            node.uri = Some(doc.uri.clone());
            node.resolved = true;
            store.upsert_graph_node(&node)?;
            return Ok(node.id);
        }
    }

    // Also reuse an existing document node that already has this title.
    if let Some(existing) = store
        .find_nodes_by_label(&doc.title)?
        .into_iter()
        .find(|n| n.kind == "document" && n.resolved)
    {
        let mut node = existing;
        node.document_id = Some(doc.id.clone());
        node.uri = Some(doc.uri.clone());
        node.label = doc.title.clone();
        store.upsert_graph_node(&node)?;
        return Ok(node.id);
    }

    let node = GraphNode {
        id: Uuid::new_v4().to_string(),
        kind: "document".into(),
        label: doc.title.clone(),
        document_id: Some(doc.id.clone()),
        uri: Some(doc.uri.clone()),
        resolved: true,
        metadata_json: "{}".into(),
    };
    let id = node.id.clone();
    store.upsert_graph_node(&node)?;
    Ok(id)
}

fn promote_label_candidates(doc: &Document) -> Vec<String> {
    let mut labels = vec![doc.title.clone()];
    if let Some(base) = uri_basename(&doc.uri) {
        if base != doc.title {
            labels.push(base);
        }
    }
    if let Some(slug) = wiki_slug_from_uri(&doc.uri) {
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

fn resolve_target(store: &Store, link: &ExtractedLink) -> Result<String> {
    match link.rel_type.as_str() {
        "tagged" => upsert_tag_node(store, &link.target_label),
        _ => resolve_wikilink_target(store, &link.target_label),
    }
}

fn upsert_tag_node(store: &Store, label: &str) -> Result<String> {
    let existing = store.find_nodes_by_label(label)?;
    if let Some(node) = existing.into_iter().find(|n| n.kind == "tag") {
        return Ok(node.id);
    }
    let node = GraphNode {
        id: Uuid::new_v4().to_string(),
        kind: "tag".into(),
        label: label.to_string(),
        document_id: None,
        uri: Some(format!("tag://{label}")),
        resolved: true,
        metadata_json: "{}".into(),
    };
    let id = node.id.clone();
    store.upsert_graph_node(&node)?;
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
fn resolve_wikilink_target(store: &Store, label: &str) -> Result<String> {
    let label = label.trim();
    if label.is_empty() {
        // Should not happen from extract; keep a stable empty stub.
        return upsert_stub(store, "");
    }

    // 1. Exact label on graph (prefer resolved document).
    let matches = store.find_nodes_by_label(label)?;
    if let Some(n) = pick_best_node(&matches) {
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
        if let Some(n) = store.find_node_by_uri(uri)? {
            return Ok(n.id);
        }
    }

    // 3. Document by uri → ensure graph node for that document
    for uri in &uri_candidates {
        if let Some(doc) = store.find_by_uri(uri)? {
            return ensure_document_node(store, &doc);
        }
    }

    // 4. wiki_index by slug (label or slugified)
    for slug in [label, slugified.as_str()] {
        if let Some(entry) = store.get_wiki_index_by_slug(slug)? {
            if let Some(pid) = entry.page_id.as_deref() {
                if let Some(doc) = store.get_document(pid)? {
                    return ensure_document_node(store, &doc);
                }
            }
        }
    }

    // 5. Stub
    upsert_stub(store, label)
}

fn pick_best_node(matches: &[GraphNode]) -> Option<&GraphNode> {
    matches
        .iter()
        .find(|n| n.kind == "document" && n.resolved)
        .or_else(|| matches.iter().find(|n| n.resolved))
        .or_else(|| matches.first())
}

fn upsert_stub(store: &Store, label: &str) -> Result<String> {
    let matches = store.find_nodes_by_label(label)?;
    if let Some(n) = matches.into_iter().next() {
        return Ok(n.id);
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
    store.upsert_graph_node(&node)?;
    Ok(id)
}

/// Light slug for link targets (keep dots for file names; collapse spaces).
fn slugify_link(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
            out.push(c.to_ascii_lowercase());
        } else if c.is_whitespace() || c == '/' {
            if !out.ends_with('-') {
                out.push('-');
            }
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn open_temp() -> Store {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("resolve.duckdb");
        std::mem::forget(dir);
        Store::open(&path).expect("open")
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
