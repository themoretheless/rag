//! Resolve extracted links into graph nodes/edges for a document.

use std::path::Path;

use duckdb::{params, Connection};
use uuid::Uuid;

use crate::db::graph::{
    find_node_by_document_id_locked as find_node_by_document_id,
    find_node_by_uri_locked as find_node_by_uri, find_nodes_by_label_locked as find_nodes_by_label,
    insert_graph_edges_locked as insert_graph_edges, upsert_graph_node_locked as upsert_graph_node,
};
use crate::db::Store;
use crate::error::Result;
use crate::graph::extract::{extract_links, ExtractedLink, REL_TAGGED};
use crate::models::{Document, GraphEdge, GraphNode};
use crate::util::{slugify, wiki_slug_from_uri, SlugPolicy};

/// Rebuild the object-graph slice for `doc`.
///
/// 1. Upsert (or promote stub for) the document node — stable id by `document_id` / `uri`.
/// 2. Delete existing outgoing edges from that node.
/// 3. Extract wikilinks + tags; resolve targets by label, slug, or `wiki://` uri; write edges.
///
/// Returns `(node_id, edge_count)` for the document node and edges written this pass.
pub fn rebuild_document_graph(store: &Store, doc: &Document) -> Result<(String, usize)> {
    let conn = store.lock()?;
    rebuild_document_graph_locked(&conn, doc)
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
    conn.execute(
        "DELETE FROM graph_edges WHERE source_id = ?",
        params![node_id],
    )?;

    // Obsidian markup is meaningful in prose, but `[[ ... ]]` is also ordinary
    // syntax in shell and generated source files. Parsing every source file
    // creates thousands of fake stubs such as `[[ -f "$path" ]]`.
    let mut links = if document_supports_knowledge_markup(doc) {
        extract_links(&doc.content)
    } else {
        Vec::new()
    };
    for metadata_link in metadata_tag_links(&doc.metadata_json) {
        if !links.iter().any(|link| {
            link.rel_type == metadata_link.rel_type
                && link.target_label.eq_ignore_ascii_case(&metadata_link.target_label)
        }) {
            links.push(metadata_link);
        }
    }
    let mut edges: Vec<GraphEdge> = Vec::with_capacity(links.len() + 2);

    for link in &links {
        let target_id = resolve_target(conn, link)?;
        edges.push(GraphEdge {
            id: Uuid::new_v4().to_string(),
            source_id: node_id.clone(),
            target_id,
            rel_type: link.rel_type.clone(),
            weight: 1.0,
            context: link.context.clone(),
        });
    }

    append_structural_edges(conn, doc, &node_id, &mut edges)?;

    let edge_count = edges.len();
    insert_graph_edges(conn, &edges)?;
    Ok((node_id, edge_count))
}

fn metadata_tag_links(metadata_json: &str) -> Vec<ExtractedLink> {
    serde_json::from_str::<serde_json::Value>(metadata_json)
        .ok()
        .and_then(|metadata| metadata.get("tags").and_then(|tags| tags.as_array()).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|tag| tag.as_str().map(str::trim).map(str::to_string))
        .filter(|tag| !tag.is_empty())
        .map(|tag| ExtractedLink {
            target_label: tag,
            rel_type: REL_TAGGED.into(),
            context: Some("document metadata tag".into()),
            alias: None,
        })
        .collect()
}

fn document_supports_knowledge_markup(doc: &Document) -> bool {
    if matches!(doc.layer.as_str(), "wiki" | "diary") {
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
    edges: &mut Vec<GraphEdge>,
) -> Result<()> {
    if let Some(project) = doc.wing.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let target_id =
            ensure_structural_node(conn, "project", project, &format!("project://{project}"))?;
        edges.push(structural_edge(
            document_node_id,
            target_id,
            "project membership",
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
        "directory membership",
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

fn structural_edge(source_id: &str, target_id: String, context: &str) -> GraphEdge {
    GraphEdge {
        id: Uuid::new_v4().to_string(),
        source_id: source_id.to_string(),
        target_id,
        rel_type: "related".into(),
        weight: 0.5,
        context: Some(context.to_string()),
    }
}

/// Ensure a resolved document node exists for `doc`; promote matching stubs by title/slug/uri.
fn ensure_document_node(
    conn: &Connection,
    document_id: &str,
    title: &str,
    uri: &str,
) -> Result<String> {
    // Prefer existing node for this document id (stable across re-ingest with same doc id).
    if let Some(existing) = find_node_by_document_id(conn, document_id)? {
        let mut node = existing;
        node.kind = "document".into();
        node.label = title.to_string();
        node.document_id = Some(document_id.to_string());
        node.uri = Some(uri.to_string());
        node.resolved = true;
        upsert_graph_node(conn, &node)?;
        return Ok(node.id);
    }

    // Stable by uri when node survived a content-only re-ingest path.
    if !uri.is_empty() {
        if let Some(existing) = find_node_by_uri(conn, uri)? {
            let mut node = existing;
            node.kind = "document".into();
            node.label = title.to_string();
            node.document_id = Some(document_id.to_string());
            node.uri = Some(uri.to_string());
            node.resolved = true;
            upsert_graph_node(conn, &node)?;
            return Ok(node.id);
        }
    }

    // Promote stub whose label matches title, uri basename, or wiki slug
    // (so [[rag-mcp-overview]] stubs resolve when page title differs).
    let promote_labels = promote_label_candidates(title, uri);
    for label in &promote_labels {
        let matches = find_nodes_by_label(conn, label)?;
        if let Some(stub) = matches
            .into_iter()
            .find(|n| n.kind == "stub" || !n.resolved)
        {
            let mut node = stub;
            node.kind = "document".into();
            node.label = title.to_string();
            node.document_id = Some(document_id.to_string());
            node.uri = Some(uri.to_string());
            node.resolved = true;
            upsert_graph_node(conn, &node)?;
            return Ok(node.id);
        }
    }

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

fn upsert_tag_node(conn: &Connection, label: &str) -> Result<String> {
    let existing = find_nodes_by_label(conn, label)?;
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

    // 1. Exact label on graph (prefer resolved document).
    let matches = find_nodes_by_label(conn, label)?;
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

fn pick_best_node(matches: &[GraphNode]) -> Option<&GraphNode> {
    matches
        .iter()
        .find(|n| n.kind == "document" && n.resolved)
        .or_else(|| matches.iter().find(|n| n.resolved))
        .or_else(|| matches.first())
}

fn upsert_stub(conn: &Connection, label: &str) -> Result<String> {
    let matches = find_nodes_by_label(conn, label)?;
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
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(params![value])?;
    match rows.next()? {
        Some(row) => Ok(Some((row.get(0)?, row.get(1)?, row.get(2)?))),
        None => Ok(None),
    }
}

fn find_wiki_index_page_id(conn: &Connection, slug: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare(
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
