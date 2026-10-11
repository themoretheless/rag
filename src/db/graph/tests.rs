use super::*;
use crate::models::Document;
use chrono::Utc;

/// The inlined SQL literals are a projection of §7.1, never a second copy.
#[test]
fn scoped_sql_name_lists_derive_from_the_models_sources() {
    assert_eq!(
        scoped_relation_types(false),
        sql_string_list(
            crate::models::pkb_rel_types(false)
                .iter()
                .map(String::as_str)
        )
    );
    assert_eq!(
        scoped_relation_types(true),
        sql_string_list(
            crate::models::pkb_rel_types(true)
                .iter()
                .map(String::as_str)
        )
    );
    assert!(crate::models::pkb_rel_types(true)
        .iter()
        .any(|rel| rel == "реализует"));
    assert!(!scoped_companion_kinds(true).contains("document"));
    assert_eq!(
        ui_node_kinds(false),
        sql_string_list(
            crate::models::pkb_node_kinds(false)
                .iter()
                .map(String::as_str)
        )
    );
}

/// §1.6: the rebuild delete predicate must inline the extraction-owned names
/// from the models list, never a second copy of them.
#[test]
fn delete_predicate_inlines_the_models_extract_owned_names() {
    let sql = delete_derived_edges_sql();
    assert!(sql.contains(&sql_string_list(
        crate::models::EXTRACT_OWNED_REL_TYPES.iter().copied()
    )));
    assert!(sql.contains("origin = 'extract'"), "{sql}");
    assert!(sql.contains("origin IS NULL"), "{sql}");
}

#[test]
fn sql_string_list_doubles_quotes() {
    assert_eq!(sql_string_list(["a", "b'c"]), "'a', 'b''c'");
}

#[test]
fn hand_authored_wiki_relation_survives_the_boundary_and_the_pkb_walk() {
    let store = open_temp();
    for (id, label) in [("wiki-a", "A"), ("wiki-b", "B")] {
        store
            .upsert_graph_node(&node(id, "document", label, None))
            .expect("insert node");
    }

    let edge = store
        .link_nodes("wiki-a", "wiki-b", "реализует", 1.0)
        .expect("link_nodes with a wiki semantic rel_type");
    assert_eq!(edge.rel_type, "реализует");

    let pkb_rels = crate::models::pkb_rel_types(false);
    let view = store
        .neighbors_filtered("wiki-a", 1, 100, Some(&pkb_rels))
        .expect("PKB default walk");
    assert!(
        view.edges.iter().any(|e| e.rel_type == "реализует"),
        "the §7.1 default must keep hand-authored relations: {:?}",
        view.edges
    );

    let rejected = store.link_nodes("wiki-a", "wiki-b", "зависимость", 1.0);
    assert!(
        rejected
            .err()
            .is_some_and(|error| error.to_string().contains("invalid rel_type")),
        "an unknown rel_type must stay rejected"
    );
}

fn open_temp() -> Store {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("graph.duckdb");
    std::mem::forget(dir);
    Store::open(&path).expect("open")
}

fn node(id: &str, kind: &str, label: &str, doc_id: Option<&str>) -> GraphNode {
    GraphNode {
        id: id.into(),
        kind: kind.into(),
        label: label.into(),
        document_id: doc_id.map(str::to_string),
        uri: None,
        resolved: kind != "stub",
        metadata_json: "{}".into(),
    }
}

fn edge(id: &str, source_id: &str, target_id: &str, rel_type: &str) -> GraphEdge {
    GraphEdge {
        id: id.into(),
        source_id: source_id.into(),
        target_id: target_id.into(),
        rel_type: rel_type.into(),
        weight: 1.0,
        context: None,
    }
}

#[test]
fn upsert_link_neighbors_backlinks() {
    let store = open_temp();
    let now = Utc::now();
    let doc = Document {
        id: "d1".into(),
        uri: "doc://a".into(),
        title: "A".into(),
        content: "x".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        ..Default::default()
    };
    store.upsert_document(&doc).unwrap();

    store
        .upsert_graph_node(&node("n1", "document", "A", Some("d1")))
        .unwrap();
    store
        .upsert_graph_node(&node("n2", "stub", "B", None))
        .unwrap();
    store
        .upsert_graph_node(&node("t1", "tag", "idea", None))
        .unwrap();

    store.link_nodes("n1", "n2", "wikilink", 1.0).unwrap();
    store.link_nodes("n1", "t1", "tagged", 1.0).unwrap();
    store.link_nodes("n1", "t1", "tunnel", 0.5).unwrap();

    let view = store.neighbors("n1", 1, 100).unwrap();
    assert_eq!(view.nodes.len(), 3);
    assert_eq!(view.edges.len(), 3);

    // Undirected: from stub n2 see n1
    let from_stub = store.neighbors("n2", 1, 100).unwrap();
    assert!(from_stub.nodes.iter().any(|n| n.id == "n1"));

    let bl = store.backlinks("n2").unwrap();
    assert_eq!(bl.edges.len(), 1);
    assert!(bl.nodes.iter().any(|n| n.id == "n1"));

    let by_doc = store
        .find_node_by_document_id("d1")
        .unwrap()
        .expect("doc node");
    assert_eq!(by_doc.id, "n1");

    let by_label = store.find_nodes_by_label("idea").unwrap();
    assert_eq!(by_label.len(), 1);

    let resolved = store
        .resolve_node(None, Some("d1"), None)
        .unwrap()
        .expect("resolve");
    assert_eq!(resolved.id, "n1");

    store.delete_edges_from("n1").unwrap();
    let bl_after = store.backlinks("n2").unwrap();
    assert!(bl_after.edges.is_empty());

    store.link_nodes("n1", "n2", "related", 1.0).unwrap();

    // §6.3: deleting the document demotes its node; the explicit edge survives
    // and n2 keeps a backlink to what used to be a note.
    assert!(store.delete_document("d1").unwrap());
    assert!(store.find_node_by_document_id("d1").unwrap().is_none());
    let demoted = store.find_node_by_id("n1").unwrap().expect("stub remains");
    assert_eq!(demoted.kind, "stub");
    assert!(!demoted.resolved);
    assert_eq!(demoted.document_id, None);
    assert_eq!(demoted.uri.as_deref(), None);
    assert!(store.find_node_by_id("n2").unwrap().is_some());
    let bl_after_delete = store.backlinks("n2").unwrap();
    assert_eq!(bl_after_delete.edges.len(), 1);
    assert_eq!(bl_after_delete.edges[0].rel_type, "related");
}

/// §5.2 allows two nodes to carry one uri, so the lookup must name one of them the
/// same way on every read: a resolved node beats the stub that was created for the
/// uri before the document arrived, the first writer beats a newer copy (§5.2
/// rejects resolve-by-newest), and the id breaks an exact tie.
#[test]
fn find_node_by_uri_names_one_canonical_copy() {
    let store = open_temp();
    {
        let conn = store.lock().unwrap();
        conn.execute(
                "INSERT INTO graph_nodes (id, kind, label, document_id, uri, resolved, created_at, updated_at) VALUES
                 ('stub-from-a-wikilink', 'stub', 'dup.cs', NULL, 'file:///dup.cs', false, '2026-08-01 10:00:00', '2026-08-01 10:00:00'),
                 ('older-copy',    'document', 'dup.cs',      'doc-a', 'file:///dup.cs', true,  '2026-09-02 10:00:00', '2026-09-04 10:00:00'),
                 ('newer-copy',    'document', 'rag: dup.cs', 'doc-a', 'file:///dup.cs', true,  '2026-09-20 10:00:00', '2026-09-20 10:00:00'),
                 ('zz-tie',        'document', 'tie.cs',   'doc-b', 'file:///tie.cs', true,  '2026-09-01 10:00:00', '2026-09-20 10:00:00'),
                 ('aa-tie',        'document', 'tie.cs',   'doc-b', 'file:///tie.cs', true,  '2026-09-01 10:00:00', '2026-09-20 10:00:00')",
                [],
            )
            .unwrap();
    }

    for _ in 0..3 {
        let found = store
            .find_node_by_uri("file:///dup.cs")
            .unwrap()
            .expect("canonical copy");
        assert_eq!(found.id, "older-copy");
        assert_eq!(found.label, "dup.cs");
    }
    let tie = store
        .find_node_by_uri("file:///tie.cs")
        .unwrap()
        .expect("either copy is canonical");
    assert_eq!(tie.id, "aa-tie");
    assert!(store
        .find_node_by_uri("file:///absent.cs")
        .unwrap()
        .is_none());
}

/// §6.3: extraction noise is reclaimed on delete, user-authored inbound links
/// from surviving notes are not.
#[test]
fn delete_document_keeps_inbound_and_drops_outbound_extract_edges() {
    let store = open_temp();
    let now = Utc::now();
    for (id, uri, title) in [("d1", "doc://a", "A"), ("d2", "doc://b", "B")] {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: uri.into(),
                title: title.into(),
                content: "x".into(),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                ..Default::default()
            })
            .unwrap();
    }
    store
        .upsert_graph_node(&node("n1", "document", "A", Some("d1")))
        .unwrap();
    store
        .upsert_graph_node(&node("n2", "document", "B", Some("d2")))
        .unwrap();
    // A -> B: one edge body extraction owns, one the user created.
    {
        let conn = store.lock().unwrap();
        let extracted = DerivedEdge {
            edge: edge("e-out-extract", "n1", "n2", "wikilink"),
            alias: None,
            heading: None,
            char_start: Some(0),
            char_end: Some(10),
            occurrence: 0,
            chunk_id: None,
        };
        insert_derived_graph_edges_locked(&conn, &[extracted]).unwrap();
    }
    let out_explicit = store.link_nodes("n1", "n2", "tunnel", 1.0).unwrap().id;
    // B -> A: inbound to the doomed node.
    let in_explicit = store.link_nodes("n2", "n1", "related", 1.0).unwrap().id;

    assert!(store.delete_document("d1").unwrap());

    let mut kept: Vec<String> = store
        .list_graph_edges()
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect();
    kept.sort();
    let mut expected = vec![in_explicit, out_explicit];
    expected.sort();
    assert_eq!(kept, expected);
    let stub = store.find_node_by_id("n1").unwrap().expect("demoted");
    assert_eq!(stub.kind, "stub");
    assert!(!stub.resolved);
    assert_eq!(stub.document_id, None);
    // The demoted stub stays reachable from B, so the backlink keeps its meaning.
    let bl = store.backlinks("n1").unwrap();
    assert_eq!(bl.edges.len(), 1);
    assert_eq!(bl.edges[0].source_id, "n2");
}

fn ids<T, F: Fn(&T) -> &str>(rows: &[T], key: F) -> Vec<String> {
    let mut out: Vec<String> = rows.iter().map(|r| key(r).to_string()).collect();
    out.sort();
    out
}

/// §7.2: the walk is per-hop frontier SQL, so the budget has to be applied
/// while discovering. Tags stay reachable here because `neighbors` is the raw
/// local graph, not the UI's PKB projection.
#[test]
fn neighbors_bounds_multihop_walks_and_repeats_identically() {
    let store = open_temp();
    for (id, kind, label) in [
        ("c1", "document", "C1"),
        ("c2", "document", "C2"),
        ("c3", "tag", "inbox"),
    ] {
        store
            .upsert_graph_node(&node(id, kind, label, None))
            .unwrap();
    }
    store
        .insert_graph_edges(&[
            edge("e1", "c1", "c2", "wikilink"),
            edge("e2", "c2", "c3", "tagged"),
        ])
        .unwrap();

    let one_hop = store.neighbors("c1", 1, 100).unwrap();
    assert_eq!(ids(&one_hop.nodes, |n| n.id.as_str()), ["c1", "c2"]);
    assert_eq!(one_hop.edges.len(), 1);

    let two_hop = store.neighbors("c1", 2, 100).unwrap();
    assert_eq!(
        ids(&two_hop.nodes, |n| n.id.as_str()),
        ["c1", "c2", "c3"],
        "hop two reaches the tag hub"
    );
    assert_eq!(ids(&two_hop.edges, |e| e.id.as_str()), ["e1", "e2"]);

    let capped = store.neighbors("c1", 2, 2).unwrap();
    assert_eq!(capped.nodes.len(), 2, "max_nodes bounds the walk");
    assert_eq!(
        capped.edges.len(),
        1,
        "an edge to an unbudgeted node is not reported"
    );

    let again = store.neighbors("c1", 2, 100).unwrap();
    assert_eq!(
        ids(&again.nodes, |n| n.id.as_str()),
        ids(&two_hop.nodes, |n| n.id.as_str()),
        "the frontier order is stable across calls"
    );
}

/// §7.1: the walk can be narrowed to a named relation set, which is how MCP
/// `get_neighbors` applies its PKB default. The raw contract stays untouched.
#[test]
fn neighbors_filtered_walks_only_the_named_relations() {
    let store = open_temp();
    for (id, kind, label) in [
        ("a", "document", "A"),
        ("b", "document", "B"),
        ("t", "tag", "inbox"),
    ] {
        store
            .upsert_graph_node(&node(id, kind, label, None))
            .unwrap();
    }
    store.link_nodes("a", "b", "wikilink", 1.0).unwrap();
    store.link_nodes("a", "t", "tagged", 1.0).unwrap();
    store.link_nodes("b", "t", "tunnel", 1.0).unwrap();

    let literary = vec!["wikilink".to_string(), "related".to_string()];
    let pkb = store
        .neighbors_filtered("a", 3, 100, Some(&literary))
        .unwrap();
    assert_eq!(ids(&pkb.nodes, |n| n.id.as_str()), vec!["a", "b"]);
    assert!(pkb
        .edges
        .iter()
        .all(|edge| edge.rel_type == "wikilink" || edge.rel_type == "related"));

    let with_tags = vec![
        "wikilink".to_string(),
        "related".to_string(),
        "tagged".to_string(),
    ];
    let tagged = store
        .neighbors_filtered("a", 3, 100, Some(&with_tags))
        .unwrap();
    assert_eq!(ids(&tagged.nodes, |n| n.id.as_str()), vec!["a", "b", "t"]);

    // Raw walk: the tunnel is followed, so the tag hub is reached through b.
    let raw = store.neighbors("a", 3, 100).unwrap();
    assert_eq!(ids(&raw.nodes, |n| n.id.as_str()), vec!["a", "b", "t"]);
    assert!(raw.edges.iter().any(|edge| edge.rel_type == "tunnel"));
}

#[test]
fn neighbors_clamps_depth_to_the_local_graph_cap() {
    let store = open_temp();
    let chain: Vec<String> = (0..10).map(|i| format!("n{i}")).collect();
    for id in &chain {
        store
            .upsert_graph_node(&node(id, "document", id, None))
            .unwrap();
    }
    let links: Vec<GraphEdge> = chain
        .windows(2)
        .map(|pair| {
            let id = format!("e-{}-{}", pair[0], pair[1]);
            edge(&id, &pair[0], &pair[1], "wikilink")
        })
        .collect();
    store.insert_graph_edges(&links).unwrap();

    let view = store.neighbors("n0", u32::MAX, 100).unwrap();
    assert_eq!(
        view.nodes.len(),
        MAX_LOCAL_GRAPH_DEPTH as usize + 1,
        "depth beyond the §9 cap is not walked"
    );
}

/// An edge whose endpoint has no node row cannot carry the walk onward; the
/// old adjacency-map BFS happily traversed through such ghosts.
#[test]
fn neighbors_does_not_route_through_missing_nodes() {
    let store = open_temp();
    for id in ["here", "beyond"] {
        store
            .upsert_graph_node(&node(id, "document", id, None))
            .unwrap();
    }
    store
        .insert_graph_edges(&[
            edge("e1", "here", "ghost", "wikilink"),
            edge("e2", "ghost", "beyond", "wikilink"),
        ])
        .unwrap();

    let view = store.neighbors("here", 3, 100).unwrap();
    assert_eq!(ids(&view.nodes, |n| n.id.as_str()), ["here"]);
    assert!(view.edges.is_empty());
}

#[test]
fn prune_orphan_stubs_keeps_referenced_placeholders() {
    let store = open_temp();
    store
        .upsert_graph_node(&node("doc", "document", "Source", Some("d1")))
        .unwrap();
    store
        .upsert_graph_node(&node("used", "stub", "Expected", None))
        .unwrap();
    store
        .upsert_graph_node(&node("orphan", "stub", "Obsolete", None))
        .unwrap();
    store.link_nodes("doc", "used", "wikilink", 1.0).unwrap();

    assert_eq!(store.prune_orphan_stubs().unwrap(), 1);
    assert!(store.find_node_by_id("used").unwrap().is_some());
    assert!(store.find_node_by_id("orphan").unwrap().is_none());
}

#[test]
fn prune_orphan_tags_keeps_referenced_tags() {
    let store = open_temp();
    store
        .upsert_graph_node(&node("doc", "document", "Source", Some("d1")))
        .unwrap();
    store
        .upsert_graph_node(&node("used-tag", "tag", "rust", None))
        .unwrap();
    store
        .upsert_graph_node(&node("orphan-tag", "tag", "000000", None))
        .unwrap();
    store.link_nodes("doc", "used-tag", "tagged", 1.0).unwrap();

    assert_eq!(store.prune_orphan_tags().unwrap(), 1);
    assert!(store.find_node_by_id("used-tag").unwrap().is_some());
    assert!(store.find_node_by_id("orphan-tag").unwrap().is_none());
}

#[test]
fn get_graph_view_filters() {
    let store = open_temp();
    store
        .upsert_graph_node(&node("n1", "document", "A", Some("d1")))
        .unwrap();
    store
        .upsert_graph_node(&node("n2", "document", "B", Some("d2")))
        .unwrap();
    store
        .upsert_graph_node(&node("t1", "tag", "idea", None))
        .unwrap();
    store.link_nodes("n1", "n2", "wikilink", 1.0).unwrap();
    store.link_nodes("n1", "t1", "tagged", 1.0).unwrap();

    let all = store.get_graph_view(GraphFilter::default()).unwrap();
    assert_eq!(all.nodes.len(), 3);
    assert_eq!(all.edges.len(), 2);

    let docs = store
        .get_graph_view(GraphFilter {
            kinds: Some(vec!["document".into()]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(docs.nodes.len(), 2);
    assert_eq!(docs.edges.len(), 1);

    let seeded = store
        .get_graph_view(GraphFilter {
            seed_ids: Some(vec!["n1".into(), "t1".into()]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(seeded.nodes.len(), 2);
    assert_eq!(seeded.edges.len(), 1);
    assert_eq!(seeded.edges[0].rel_type, "tagged");

    let missing = store.link_nodes("n1", "nope", "related", 1.0);
    assert!(matches!(missing, Err(AppError::NotFound(_))));
}

#[test]
fn export_graph_for_ui_pkb_defaults() {
    let store = open_temp();
    store
        .upsert_graph_node(&node("n1", "document", "A", Some("d1")))
        .unwrap();
    store
        .upsert_graph_node(&node("n2", "document", "B", Some("d2")))
        .unwrap();
    store
        .upsert_graph_node(&node("s1", "stub", "Missing", None))
        .unwrap();
    store
        .upsert_graph_node(&node("t1", "tag", "idea", None))
        .unwrap();
    store.link_nodes("n1", "n2", "wikilink", 1.0).unwrap();
    store.link_nodes("n1", "s1", "related", 1.0).unwrap();
    store.link_nodes("n1", "t1", "tagged", 1.0).unwrap();
    store.link_nodes("n1", "n2", "tunnel", 0.5).unwrap();

    let view = store.export_graph_for_ui(None, false).unwrap();
    let kinds: HashSet<_> = view.nodes.iter().map(|n| n.kind.as_str()).collect();
    assert!(kinds.contains("document"));
    assert!(kinds.contains("stub"));
    assert!(!kinds.contains("tag"));
    assert_eq!(view.nodes.len(), 3);
    for e in &view.edges {
        assert!(e.rel_type == "wikilink" || e.rel_type == "related");
    }
    assert_eq!(view.edges.len(), 2);

    let with_tags = store.export_graph_for_ui(Some(300), true).unwrap();
    assert!(with_tags.nodes.iter().any(|n| n.kind == "tag"));
    assert!(with_tags.edges.iter().any(|e| e.rel_type == "tagged"));

    let by_label = store.find_seed_node("A").unwrap().expect("label seed");
    assert_eq!(by_label.id, "n1");
    let by_doc = store.find_seed_node("d1").unwrap().expect("doc seed");
    assert_eq!(by_doc.id, "n1");
    let local = store.export_neighbors_for_ui("A", 1, 100).unwrap();
    assert!(local.nodes.iter().any(|n| n.id == "n1"));
    assert!(local.nodes.iter().any(|n| n.id == "n2"));
}

#[test]
fn pkb_neighbors_filter_hidden_relations_before_budget_and_honor_tags() {
    let store = open_temp();
    for (id, kind, label) in [
        ("seed", "document", "Seed"),
        ("wanted", "document", "Wanted"),
        ("hidden", "document", "Hidden"),
        ("tag", "tag", "Tag"),
    ] {
        store
            .upsert_graph_node(&node(id, kind, label, None))
            .unwrap();
    }
    store
        .insert_graph_edges(&[
            edge("00-tunnel", "seed", "hidden", "tunnel"),
            edge("01-wikilink", "seed", "wanted", "wikilink"),
            edge("02-tagged", "seed", "tag", "tagged"),
            edge("03-dependency", "seed", "hidden", "depends_on"),
        ])
        .unwrap();

    let without_tags = store
        .export_pkb_neighbors_for_ui("Seed", 1, 2, false)
        .unwrap();
    let without_ids = without_tags
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    assert_eq!(without_ids, HashSet::from(["seed", "wanted"]));
    assert_eq!(without_tags.edges.len(), 1);
    assert_eq!(without_tags.edges[0].rel_type, "wikilink");

    let with_tags = store
        .export_pkb_neighbors_for_ui("Seed", 1, 3, true)
        .unwrap();
    let with_ids = with_tags
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    assert_eq!(with_ids, HashSet::from(["seed", "tag", "wanted"]));
    assert!(with_tags.edges.iter().any(|edge| edge.rel_type == "tagged"));
    assert!(!with_tags.nodes.iter().any(|node| node.id == "hidden"));
    assert!(with_tags
        .edges
        .iter()
        .all(|edge| matches!(edge.rel_type.as_str(), "wikilink" | "tagged")));
}

#[test]
fn project_graph_scopes_before_cap_and_lazy_expansion_cannot_cross_projects() {
    let store = open_temp();
    for (id, wing, title) in [
        ("alpha-doc", "alpha", "Zed project note"),
        ("beta-doc", "beta", "Aardvark other note"),
    ] {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("file:///{wing}/{id}.md"),
                title: title.into(),
                content: title.into(),
                wing: Some(wing.into()),
                room: (wing == "alpha").then(|| "docs".into()),
                layer: "raw".into(),
                ..Default::default()
            })
            .unwrap();
    }
    // Additive migrations historically allowed NULL here. Project exports
    // must expose the same effective `raw` layer as the document codec.
    store
        .lock()
        .unwrap()
        .execute(
            "UPDATE documents SET layer = NULL WHERE id = ?",
            duckdb::params!["alpha-doc"],
        )
        .unwrap();
    let mut alpha_node = node(
        "alpha-node",
        "document",
        "Zed project note",
        Some("alpha-doc"),
    );
    alpha_node.metadata_json =
        r#"{"wing":"stale","room":"old","layer":"index","custom":true}"#.into();
    store.upsert_graph_node(&alpha_node).unwrap();
    let mut beta_node = node(
        "beta-node",
        "document",
        "Aardvark other note",
        Some("beta-doc"),
    );
    beta_node.metadata_json =
        r#"{"wing":"stale","room":"old","layer":"index","custom":"beta"}"#.into();
    store.upsert_graph_node(&beta_node).unwrap();
    store
        .upsert_graph_node(&node("alpha-stub", "stub", "Missing alpha", None))
        .unwrap();
    store
        .upsert_graph_node(&node("alpha-entity", "entity", "Entity alpha", None))
        .unwrap();
    store
        .upsert_graph_node(&node("alpha-tag", "tag", "Tag alpha", None))
        .unwrap();
    store
        .link_nodes("alpha-node", "alpha-stub", "wikilink", 1.0)
        .unwrap();
    store
        .link_nodes("alpha-node", "alpha-entity", "related", 1.0)
        .unwrap();
    store
        .link_nodes("alpha-node", "alpha-tag", "tagged", 1.0)
        .unwrap();
    store
        .link_nodes("alpha-node", "beta-node", "related", 1.0)
        .unwrap();

    let capped = store
        .export_project_graph_for_ui("alpha", Some(1), false)
        .unwrap();
    assert_eq!(capped.nodes.len(), 1);
    assert_eq!(capped.nodes[0].id, "alpha-node");

    let graph = store
        .export_project_graph_for_ui("alpha", Some(10), false)
        .unwrap();
    assert!(graph.nodes.iter().any(|node| node.id == "alpha-node"));
    assert!(graph.nodes.iter().any(|node| node.id == "alpha-stub"));
    assert!(graph.nodes.iter().any(|node| node.id == "alpha-entity"));
    assert!(!graph.nodes.iter().any(|node| node.id == "alpha-tag"));
    assert!(!graph.nodes.iter().any(|node| node.id == "beta-node"));
    assert_eq!(graph.edges.len(), 2);
    let alpha = graph
        .nodes
        .iter()
        .find(|node| node.id == "alpha-node")
        .expect("project document node");
    let alpha_meta: serde_json::Value =
        serde_json::from_str(&alpha.metadata_json).expect("placement metadata");
    assert_eq!(alpha_meta["wing"], "alpha");
    assert_eq!(alpha_meta["room"], "docs");
    assert_eq!(alpha_meta["layer"], "raw");
    assert_eq!(alpha_meta["custom"], true);

    let beta_graph = store
        .export_project_graph_for_ui("beta", Some(10), false)
        .unwrap();
    let beta = beta_graph
        .nodes
        .iter()
        .find(|node| node.id == "beta-node")
        .expect("other project document node");
    let beta_meta: serde_json::Value =
        serde_json::from_str(&beta.metadata_json).expect("placement metadata");
    assert_eq!(beta_meta["wing"], "beta");
    assert!(beta_meta.get("room").is_none());
    assert_eq!(beta_meta["layer"], "raw");
    assert_eq!(beta_meta["custom"], "beta");

    let graph_with_tags = store
        .export_project_graph_for_ui("alpha", Some(10), true)
        .unwrap();
    assert!(graph_with_tags
        .nodes
        .iter()
        .any(|node| node.id == "alpha-tag"));
    assert!(graph_with_tags
        .edges
        .iter()
        .any(|edge| edge.rel_type == "tagged"));
    assert!(!graph_with_tags
        .nodes
        .iter()
        .any(|node| node.id == "beta-node"));

    let lazy = store
        .export_project_neighbors_for_ui("alpha", "Zed project note", 1, 10, false)
        .unwrap();
    assert!(lazy.nodes.iter().any(|node| node.id == "alpha-stub"));
    assert!(lazy.nodes.iter().any(|node| node.id == "alpha-entity"));
    assert!(!lazy.nodes.iter().any(|node| node.id == "alpha-tag"));
    assert!(!lazy.nodes.iter().any(|node| node.id == "beta-node"));
    let lazy_alpha = lazy
        .nodes
        .iter()
        .find(|node| node.id == "alpha-node")
        .expect("lazy project document node");
    let lazy_meta: serde_json::Value =
        serde_json::from_str(&lazy_alpha.metadata_json).expect("placement metadata");
    assert_eq!(lazy_meta["wing"], "alpha");
    assert_eq!(lazy_meta["room"], "docs");
    assert!(store
        .export_project_neighbors_for_ui("alpha", "Aardvark other note", 1, 10, false)
        .unwrap()
        .nodes
        .is_empty());
}

#[test]
fn project_neighbors_are_deterministic_bounded_and_depth_clamped() {
    let store = open_temp();
    for (id, wing) in [
        ("seed-doc", "alpha"),
        ("a-doc", "alpha"),
        ("b-doc", "alpha"),
        ("c-doc", "alpha"),
        ("d-doc", "alpha"),
        ("e-doc", "alpha"),
        ("beta-doc", "beta"),
    ] {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("file:///{wing}/{id}.md"),
                title: id.into(),
                content: id.into(),
                wing: Some(wing.into()),
                ..Default::default()
            })
            .unwrap();
    }
    for (id, label, document_id) in [
        ("seed", "Seed", "seed-doc"),
        ("a", "A", "a-doc"),
        ("b", "B", "b-doc"),
        ("c", "C", "c-doc"),
        ("d", "D", "d-doc"),
        ("e", "E", "e-doc"),
        ("beta", "Beta", "beta-doc"),
    ] {
        store
            .upsert_graph_node(&node(id, "document", label, Some(document_id)))
            .unwrap();
    }
    store
        .insert_graph_edges(&[
            edge("00-cross-project", "seed", "beta", "related"),
            edge("01-first", "seed", "b", "related"),
            edge("02-second", "seed", "a", "related"),
            edge("03-depth-two", "b", "c", "related"),
            edge("04-depth-three", "c", "d", "related"),
            edge("05-depth-four", "d", "e", "related"),
        ])
        .unwrap();

    let first = store
        .export_project_neighbors_for_ui("alpha", "Seed", 1, 2, false)
        .unwrap();
    let second = store
        .export_project_neighbors_for_ui("alpha", "Seed", 1, 2, false)
        .unwrap();
    let first_ids = first
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<Vec<_>>();
    let second_ids = second
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(first_ids, vec!["b", "seed"]);
    assert_eq!(second_ids, first_ids);
    assert_eq!(
        first
            .edges
            .iter()
            .map(|edge| edge.id.as_str())
            .collect::<Vec<_>>(),
        vec!["01-first"]
    );

    let clamped = store
        .export_project_neighbors_for_ui("alpha", "seed-doc", u32::MAX, u32::MAX, false)
        .unwrap();
    let clamped_ids = clamped
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    assert!(clamped.nodes.len() <= crate::models::UI_GRAPH_EXPORT_MAX_NODES as usize);
    assert!(clamped_ids.contains("a"));
    assert!(clamped_ids.contains("b"));
    assert!(clamped_ids.contains("c"));
    assert!(clamped_ids.contains("d"));
    assert!(!clamped_ids.contains("e"));
    assert!(!clamped_ids.contains("beta"));
}

#[test]
fn direct_companion_seed_stays_inside_project_scope() {
    let store = open_temp();
    for (id, wing) in [("alpha-doc", "alpha"), ("beta-doc", "beta")] {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("file:///{wing}/{id}.md"),
                title: id.into(),
                content: id.into(),
                wing: Some(wing.into()),
                ..Default::default()
            })
            .unwrap();
    }
    for (id, label, document_id) in [
        ("alpha-node", "Alpha", Some("alpha-doc")),
        ("beta-node", "Beta", Some("beta-doc")),
        ("shared-stub", "Shared", None),
        ("orphan-stub", "Orphan", None),
        ("alpha-tag", "Alpha tag", None),
    ] {
        let kind = if id == "alpha-tag" {
            "tag"
        } else if document_id.is_none() {
            "stub"
        } else {
            "document"
        };
        store
            .upsert_graph_node(&node(id, kind, label, document_id))
            .unwrap();
    }
    store
        .insert_graph_edges(&[
            edge("01-alpha-shared", "alpha-node", "shared-stub", "wikilink"),
            edge("02-beta-shared", "beta-node", "shared-stub", "wikilink"),
            edge("03-alpha-tag", "alpha-node", "alpha-tag", "tagged"),
        ])
        .unwrap();

    let shared = store
        .export_project_neighbors_for_ui("alpha", "shared-stub", 1, 10, false)
        .unwrap();
    assert!(shared.nodes.iter().any(|node| node.id == "alpha-node"));
    assert!(shared.nodes.iter().any(|node| node.id == "shared-stub"));
    assert!(!shared.nodes.iter().any(|node| node.id == "beta-node"));
    assert_eq!(shared.edges.len(), 1);

    assert!(store
        .export_project_neighbors_for_ui("alpha", "orphan-stub", 1, 10, false)
        .unwrap()
        .nodes
        .is_empty());
    assert!(store
        .export_project_neighbors_for_ui("alpha", "alpha-tag", 1, 10, false)
        .unwrap()
        .nodes
        .is_empty());

    let tag = store
        .export_project_neighbors_for_ui("alpha", "alpha-tag", 1, 10, true)
        .unwrap();
    assert!(tag.nodes.iter().any(|node| node.id == "alpha-node"));
    assert!(tag.nodes.iter().any(|node| node.id == "alpha-tag"));
    assert!(tag.edges.iter().any(|edge| edge.rel_type == "tagged"));
}

#[test]
fn graph_stats_by_kind_and_rel_type_includes_tunnel() {
    let store = open_temp();
    store
        .upsert_graph_node(&node("n1", "document", "A", Some("d1")))
        .unwrap();
    store
        .upsert_graph_node(&node("n2", "document", "B", Some("d2")))
        .unwrap();
    store
        .upsert_graph_node(&node("t1", "tag", "idea", None))
        .unwrap();
    store
        .upsert_graph_node(&node("s1", "stub", "Missing", None))
        .unwrap();

    store.link_nodes("n1", "n2", "wikilink", 1.0).unwrap();
    store.link_nodes("n1", "t1", "tagged", 1.0).unwrap();
    let tunnel = store
        .create_tunnel("n1", "n2", 0.75, Some("bridge a-b"))
        .unwrap();
    assert_eq!(tunnel.rel_type, "tunnel");
    assert!((tunnel.weight - 0.75).abs() < 1e-9);

    let s = store.graph_stats().expect("graph_stats");
    assert_eq!(s.total_nodes, 4);
    assert_eq!(s.total_edges, 3);
    assert_eq!(s.nodes_by_kind.get("document"), Some(&2));
    assert_eq!(s.nodes_by_kind.get("tag"), Some(&1));
    assert_eq!(s.nodes_by_kind.get("stub"), Some(&1));
    assert_eq!(s.edges_by_rel_type.get("wikilink"), Some(&1));
    assert_eq!(s.edges_by_rel_type.get("tagged"), Some(&1));
    assert_eq!(s.edges_by_rel_type.get("tunnel"), Some(&1));

    // The cut separates the tunnel's context from the context-less links, so
    // a hand-authored edge is distinguishable from a membership one.
    assert_eq!(
        s.edges_by_cut,
        vec![
            EdgeCutRow {
                rel_type: "tagged".into(),
                origin: "explicit".into(),
                context: String::new(),
                count: 1,
            },
            EdgeCutRow {
                rel_type: "tunnel".into(),
                origin: "explicit".into(),
                context: "bridge a-b".into(),
                count: 1,
            },
            EdgeCutRow {
                rel_type: "wikilink".into(),
                origin: "explicit".into(),
                context: String::new(),
                count: 1,
            },
        ]
    );
    // `graph_stats` is a JSON tool result: the cut must serialize as an array.
    let rendered = serde_json::to_value(&s).expect("graph_stats serializes");
    assert_eq!(rendered["edges_by_cut"][1]["context"], "bridge a-b");

    // Empty graph
    let empty = open_temp();
    let es = empty.graph_stats().expect("empty stats");
    assert_eq!(es.total_nodes, 0);
    assert_eq!(es.total_edges, 0);
    assert!(es.nodes_by_kind.is_empty());
    assert!(es.edges_by_rel_type.is_empty());
    assert!(es.edges_by_cut.is_empty());
}

#[test]
fn tunnel_create_list_delete_follow_find() {
    let store = open_temp();
    store
        .upsert_graph_node(&node("n1", "document", "A", Some("d1")))
        .unwrap();
    store
        .upsert_graph_node(&node("n2", "document", "B", Some("d2")))
        .unwrap();
    store
        .upsert_graph_node(&node("n3", "document", "C", Some("d3")))
        .unwrap();

    // Non-tunnel noise must not appear in follow/list
    store.link_nodes("n1", "n2", "wikilink", 1.0).unwrap();

    let t12 = store
        .create_tunnel("n1", "n2", 1.0, Some("bridge a-b"))
        .unwrap();
    assert_eq!(t12.rel_type, REL_TUNNEL);
    assert_eq!(t12.context.as_deref(), Some("bridge a-b"));

    // Idempotent undirected: reverse order updates same edge
    let again = store
        .create_tunnel("n2", "n1", 0.5, Some("updated"))
        .unwrap();
    assert_eq!(again.id, t12.id);
    assert!((again.weight - 0.5).abs() < 1e-9);
    assert_eq!(again.context.as_deref(), Some("updated"));

    let t23 = store.create_tunnel("n2", "n3", 1.0, None).unwrap();
    assert_ne!(t23.id, t12.id);

    let all = store.list_tunnels(None).unwrap();
    assert_eq!(all.len(), 2);

    let at_n2 = store.list_tunnels(Some("n2")).unwrap();
    assert_eq!(at_n2.len(), 2);

    let at_n1 = store.list_tunnels(Some("n1")).unwrap();
    assert_eq!(at_n1.len(), 1);

    // Multi-hop follow: n1 → n2 → n3 along tunnels only
    let hop1 = store.follow_tunnels("n1", 1, 100).unwrap();
    assert_eq!(hop1.nodes.len(), 2); // n1, n2
    assert!(hop1.edges.iter().all(|e| e.rel_type == REL_TUNNEL));
    assert!(!hop1.nodes.iter().any(|n| n.id == "n3"));

    let hop2 = store.follow_tunnels("n1", 2, 100).unwrap();
    assert_eq!(hop2.nodes.len(), 3);
    assert!(hop2.nodes.iter().any(|n| n.id == "n3"));
    // Only tunnel edges (not wikilink)
    assert_eq!(hop2.edges.len(), 2);
    assert!(hop2.edges.iter().all(|e| e.rel_type == REL_TUNNEL));

    let pair = store
        .find_tunnels(Some("n1"), Some("n2"), None, None)
        .unwrap();
    assert_eq!(pair.len(), 1);
    assert_eq!(pair[0].id, t12.id);

    assert!(store.delete_tunnel(&t12.id).unwrap());
    assert!(!store.delete_tunnel(&t12.id).unwrap());
    assert_eq!(store.list_tunnels(None).unwrap().len(), 1);

    // Self-loop refused
    let loop_err = store.create_tunnel("n1", "n1", 1.0, None);
    assert!(matches!(loop_err, Err(AppError::Config(_))));
}
