
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
    store.documents().upsert_document(&d).unwrap();

    let (node_id, edge_count) = rebuild_document_graph(&store, &d).unwrap();
    assert!(!node_id.is_empty());
    assert_eq!(edge_count, 2);

    let node = store
        .graph()
        .find_node_by_document_id("d1")
        .unwrap()
        .expect("node");
    assert_eq!(node.id, node_id);
    assert_eq!(node.kind, "document");
    assert!(node.resolved);

    let stubs = store.graph().find_nodes_by_label("Other Note").unwrap();
    assert_eq!(stubs.len(), 1);
    assert_eq!(stubs[0].kind, "stub");
    assert!(!stubs[0].resolved);

    let tags = store.graph().find_nodes_by_label("inbox").unwrap();
    assert_eq!(tags[0].kind, "tag");
}

#[test]
fn rebuild_preserves_explicit_edges_even_when_their_type_is_extractable() {
    let store = open_temp();
    let mut source = doc("source", "Source", "doc://source", "[[Old target]] #oldtag");
    let target = doc("target", "Target", "doc://target", "Target body");
    store.documents().upsert_document(&source).unwrap();
    store.documents().upsert_document(&target).unwrap();
    let (source_node, _) = rebuild_document_graph(&store, &source).unwrap();
    let (target_node, _) = rebuild_document_graph(&store, &target).unwrap();
    let manual = ["related", "tunnel", "wikilink", "tagged"].map(|relation| {
        store
            .graph()
            .link_nodes(&source_node, &target_node, relation, 1.0)
            .unwrap()
            .id
    });
    source.content = "[[New target]]".into();
    store.documents().upsert_document(&source).unwrap();
    for _ in 0..2 {
        rebuild_document_graph(&store, &source).unwrap();
    }
    let conn = store.lock().unwrap();
    for id in manual {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM graph_edges WHERE id = ? AND origin = 'explicit'",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
    let derived: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE source_id = ? AND origin = 'extract'",
            params![source_node],
            |row| row.get(0),
        )
        .unwrap();
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
    store.documents().upsert_document(&source).unwrap();
    store
        .chunks()
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
        let conn = store.lock().unwrap();
        let mut stmt = conn
            .prepare(
                r#"
                    SELECT rel_type, origin, alias, heading, chunk_id,
                           char_start, char_end, occurrence
                    FROM graph_edges
                    WHERE source_id = ?
                    ORDER BY rel_type ASC, id ASC
                    "#,
            )
            .unwrap();
        let found = stmt
            .query_map(duckdb::params![node_id], |row| {
                Ok(Row {
                    rel: row.get(0)?,
                    origin: row.get(1)?,
                    alias: row.get(2)?,
                    heading: row.get(3)?,
                    chunk_id: row.get(4)?,
                    char_start: row.get(5)?,
                    char_end: row.get(6)?,
                    occurrence: row.get(7)?,
                })
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
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
        .graph()
        .find_nodes_by_label("Target")
        .unwrap()
        .into_iter()
        .find(|n| n.kind == "stub")
        .expect("stub for the link target")
        .id;
    store
        .graph()
        .link_nodes(&node_id, &stub_id, "related", 1.0)
        .unwrap();
    let explicit = {
        let conn = store.lock().unwrap();
        conn.prepare(
            "SELECT origin, alias, heading, chunk_id, char_start, char_end, occurrence
                 FROM graph_edges WHERE source_id = ? AND rel_type = 'related'",
        )
        .unwrap()
        .query_map(duckdb::params![node_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<i64>>(6)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
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
    store.documents().upsert_document(&source).unwrap();
    let (node, _) = rebuild_document_graph(&store, &source).unwrap();
    let target = store
        .graph()
        .find_nodes_by_label("Old target")
        .unwrap()
        .remove(0);
    let explicit = store
        .graph()
        .link_nodes(&node, &target.id, "related", 1.0)
        .unwrap();
    store
        .lock()
        .unwrap()
        .execute("UPDATE graph_edges SET origin = NULL", [])
        .unwrap();

    {
        let conn = store.lock().unwrap();
        let attributed = crate::db::schema::backfill_graph_edge_origins(&conn).unwrap();
        assert_eq!(attributed, 2, "both legacy rows gain an owner");
    }

    rebuild_document_graph(&store, &source).unwrap();
    let counts = {
        let conn = store.lock().unwrap();
        conn.query_row(
            "SELECT \
                 COUNT(*) FILTER (WHERE origin = 'extract'), \
                 COUNT(*) FILTER (WHERE origin = 'explicit') \
                 FROM graph_edges WHERE source_id = ?",
            params![node],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap()
    };
    assert_eq!(
        counts,
        (1, 1),
        "the wikilink is re-extracted once while the explicit edge survives"
    );
    let remaining = store.graph().list_graph_edges().unwrap();
    assert_eq!(remaining.len(), 2);
    assert!(remaining.iter().any(|edge| edge.id == explicit.id));
}

/// The pipeline emits its project/directory membership edges as `related`,
/// which §1.6 otherwise treats as an explicit relation. Classifying them by
/// name alone would stamp them `explicit`, hide them from the delete
/// predicate, and stack a fresh pair on every re-ingest.
#[test]
fn legacy_membership_edges_are_attributed_by_their_structural_context() {
    let store = open_temp();
    let mut source = doc("d-legacy", "Legacy", "file:///notes/legacy.md", "Body");
    source.wing = Some("notes".into());
    source.source_file = Some("/notes/legacy.md".into());
    store.documents().upsert_document(&source).unwrap();
    let (node, _) = rebuild_document_graph(&store, &source).unwrap();
    let project = store
        .graph()
        .find_node_by_uri("project://notes")
        .unwrap()
        .expect("project node");
    // The control row: same endpoints and rel_type as the project membership
    // edge, but `link_nodes` cannot write a context, so it stays explicit.
    let hand_made = store
        .graph()
        .link_nodes(&node, &project.id, "related", 1.0)
        .unwrap();

    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE graph_edges SET origin = NULL, edge_origin = NULL",
            [],
        )
        .unwrap();
    let attributed = {
        let conn = store.lock().unwrap();
        crate::db::schema::backfill_graph_edge_origins(&conn).unwrap()
    };
    assert_eq!(attributed, 3, "two membership edges plus the hand-made one");

    let owners = || {
        let conn = store.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FILTER (WHERE origin = 'extract'), \
                 COUNT(*) FILTER (WHERE origin = 'explicit') FROM graph_edges",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap()
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
        let conn = store.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE id = ? AND origin = 'explicit'",
            params![hand_made.id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
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
    store.documents().upsert_document(&d1).unwrap();
    let (id1, _) = rebuild_document_graph(&store, &d1).unwrap();

    let d2 = doc("d1", "Home", "doc://home", "Link [[Y]] #t");
    store.documents().upsert_document(&d2).unwrap();
    let (id2, edges) = rebuild_document_graph(&store, &d2).unwrap();
    assert_eq!(id1, id2);
    assert_eq!(edges, 2);
}

#[test]
fn equal_titles_with_different_uris_keep_distinct_nodes() {
    let store = open_temp();
    let first = doc("d1", "README.md", "file:///project-a/README.md", "A");
    let second = doc("d2", "README.md", "file:///project-b/README.md", "B");
    store.documents().upsert_document(&first).unwrap();
    store.documents().upsert_document(&second).unwrap();

    let (first_node, _) = rebuild_document_graph(&store, &first).unwrap();
    let (second_node, _) = rebuild_document_graph(&store, &second).unwrap();

    assert_ne!(first_node, second_node);
    assert_eq!(
        store
            .graph()
            .find_node_by_document_id("d1")
            .unwrap()
            .unwrap()
            .id,
        first_node
    );
    assert_eq!(
        store
            .graph()
            .find_node_by_document_id("d2")
            .unwrap()
            .unwrap()
            .id,
        second_node
    );
}

#[test]
fn filesystem_document_is_linked_to_project_and_directory() {
    let store = open_temp();
    let mut source = doc("d1", "tab.rs", "file:///work/rag/src/tab.rs", "");
    source.wing = Some("rag".into());
    source.source_file = Some("/work/rag/src/tab.rs".into());
    store.documents().upsert_document(&source).unwrap();

    let (document_node_id, edge_count) = rebuild_document_graph(&store, &source).unwrap();
    assert_eq!(edge_count, 2);

    let project = store
        .graph()
        .find_node_by_uri("project://rag")
        .unwrap()
        .expect("project node");
    let directory = store
        .graph()
        .find_node_by_uri("directory:///work/rag/src")
        .unwrap()
        .expect("directory node");
    assert_eq!(project.kind, "entity");
    assert_eq!(directory.label, "src");

    let edges = store.graph().list_graph_edges().unwrap();
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
    assert_eq!(store.graph().list_graph_edges().unwrap().len(), 2);
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
    store.documents().upsert_document(&source).unwrap();

    let (_, edge_count) = rebuild_document_graph(&store, &source).unwrap();
    assert_eq!(edge_count, 2, "only project and directory edges remain");
    assert!(store
        .graph()
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
    store.documents().upsert_document(&source).unwrap();

    let (_, edge_count) = rebuild_document_graph(&store, &source).unwrap();
    assert_eq!(edge_count, 3);
    assert_eq!(
        store
            .graph()
            .find_nodes_by_label("Architecture")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn promote_stub_on_title_match() {
    let store = open_temp();
    let a = doc("da", "A", "doc://a", "See [[B]]");
    store.documents().upsert_document(&a).unwrap();
    rebuild_document_graph(&store, &a).unwrap();

    let stub = store.graph().find_nodes_by_label("B").unwrap();
    assert_eq!(stub[0].kind, "stub");
    let stub_id = stub[0].id.clone();

    let b = doc("db", "B", "doc://b", "I am B");
    store.documents().upsert_document(&b).unwrap();
    let (node_id, _) = rebuild_document_graph(&store, &b).unwrap();
    assert_eq!(node_id, stub_id);
    let promoted = store.graph().find_node_by_id(&stub_id).unwrap().unwrap();
    assert_eq!(promoted.kind, "document");
    assert!(promoted.resolved);
    assert_eq!(promoted.document_id.as_deref(), Some("db"));

    // The backlink written before the page existed must still point at it.
    let incoming = store
        .graph()
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
    store.documents().upsert_document(&tagged).unwrap();
    rebuild_document_graph(&store, &tagged).unwrap();

    let linker = doc("linker", "Linker", "doc://linker", "See [[inbox]].");
    store.documents().upsert_document(&linker).unwrap();
    let (node_id, _) = rebuild_document_graph(&store, &linker).unwrap();

    let edge = store
        .graph()
        .list_graph_edges()
        .unwrap()
        .into_iter()
        .find(|e| e.source_id == node_id && e.rel_type == "wikilink")
        .expect("wikilink edge");
    let target = store
        .graph()
        .find_node_by_id(&edge.target_id)
        .unwrap()
        .unwrap();
    assert_eq!(target.kind, "stub");
    assert!(!target.resolved);
    // One hub per tag, and the stub is a separate node sharing its label_key.
    assert_eq!(
        store
            .graph()
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
        store.documents().upsert_document(d).unwrap();
        rebuild_document_graph(&store, d).unwrap();
    }
    let documents = store
        .graph()
        .find_nodes_by_label("Duplicate")
        .unwrap()
        .into_iter()
        .filter(|n| n.kind == "document")
        .map(|n| n.id)
        .collect::<Vec<_>>();
    assert_eq!(documents.len(), 2, "equal titles keep distinct nodes");

    let linker = doc("linker", "Linker", "doc://linker", "See [[Duplicate]].");
    store.documents().upsert_document(&linker).unwrap();
    let (node_id, _) = rebuild_document_graph(&store, &linker).unwrap();
    let edge = store
        .graph()
        .list_graph_edges()
        .unwrap()
        .into_iter()
        .find(|e| e.source_id == node_id && e.rel_type == "wikilink")
        .expect("wikilink edge");
    assert!(!documents.contains(&edge.target_id));
    let target = store
        .graph()
        .find_node_by_id(&edge.target_id)
        .unwrap()
        .unwrap();
    assert_eq!(target.kind, "stub");

    // Rebuild is idempotent: the ambiguity stub is reused, not minted again.
    rebuild_document_graph(&store, &linker).unwrap();
    assert_eq!(
        store
            .graph()
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
        let conn = store.lock().unwrap();
        let first = ensure_document_node(&conn, "doc-a", "First", "wiki://shared").unwrap();
        let second = ensure_document_node(&conn, "doc-b", "Second", "wiki://shared").unwrap();
        // The original owner is untouched by the second document's arrival.
        let first_node = find_node_by_document_id(&conn, "doc-a").unwrap().unwrap();
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
        let conn = store.lock().unwrap();
        for (kind, label, uri) in [
            ("tag", "Deploy Runbook", "tag://deploy-runbook"),
            ("entity", "Deploy Runbook", "directory:///deploy"),
        ] {
            upsert_graph_node(
                &conn,
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
    store.documents().upsert_document(&doc_row).unwrap();
    let (node_id, _) = rebuild_document_graph(&store, &doc_row).unwrap();
    let nodes = store.graph().find_nodes_by_label("Deploy Runbook").unwrap();
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
    store.documents().upsert_document(&target).unwrap();
    rebuild_document_graph(&store, &target).unwrap();

    let linker = doc(
        "l1",
        "Architecture",
        "wiki://rag-mcp-architecture",
        "See [[rag-mcp-overview]] for context.",
    );
    store.documents().upsert_document(&linker).unwrap();
    let (_, edges) = rebuild_document_graph(&store, &linker).unwrap();
    assert_eq!(edges, 1);

    let stubs = store
        .graph()
        .find_nodes_by_label("rag-mcp-overview")
        .unwrap();
    // Should not leave a stub; edge points at resolved document node.
    let overview = store
        .graph()
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
    store.documents().upsert_document(&a).unwrap();
    rebuild_document_graph(&store, &a).unwrap();
    let stub = store
        .graph()
        .find_nodes_by_label("rag-mcp-overview")
        .unwrap();
    assert_eq!(stub[0].kind, "stub");
    let stub_id = stub[0].id.clone();

    let page = doc(
        "db",
        "rag-mcp: обзор проекта",
        "wiki://rag-mcp-overview",
        "Body",
    );
    store.documents().upsert_document(&page).unwrap();
    let (node_id, _) = rebuild_document_graph(&store, &page).unwrap();
    assert_eq!(node_id, stub_id);
    let promoted = store.graph().find_node_by_id(&stub_id).unwrap().unwrap();
    assert!(promoted.resolved);
    assert_eq!(promoted.kind, "document");
}
