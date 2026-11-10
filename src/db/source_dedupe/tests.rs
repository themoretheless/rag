
use chrono::Utc;

use super::*;
use crate::db::SourceManifestWrite;
use crate::db::Store;
use crate::models::{Chunk, GraphEdge, GraphNode};
use crate::util::content_hash;

fn document(
    id: &str,
    uri: &str,
    source_file: &str,
    body: &str,
    layer: &str,
    status: &str,
) -> Document {
    let now = Utc::now();
    Document {
        id: id.into(),
        uri: uri.into(),
        title: id.into(),
        content: body.into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        source_file: Some(source_file.into()),
        content_hash: Some(content_hash(body)),
        layer: layer.into(),
        status: status.into(),
        ..Default::default()
    }
}

fn graph_node(id: &str, document_id: Option<&str>) -> GraphNode {
    GraphNode {
        id: id.into(),
        kind: if document_id.is_some() {
            "document".into()
        } else {
            "entity".into()
        },
        label: id.into(),
        document_id: document_id.map(str::to_string),
        uri: None,
        resolved: true,
        metadata_json: "{}".into(),
    }
}

fn edge(id: &str, source_id: &str, target_id: &str, context: &str) -> GraphEdge {
    GraphEdge {
        id: id.into(),
        source_id: source_id.into(),
        target_id: target_id.into(),
        rel_type: "related".into(),
        weight: 1.0,
        context: Some(context.into()),
    }
}

#[test]
fn precise_cleanup_is_dry_run_then_atomic_apply() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("source-dedupe.duckdb")).unwrap();
    let source = "/vault/project/a.md";
    let body = "same source body";
    let canonical = document(
        "canonical",
        "file:///vault/project/a.md",
        source,
        body,
        "raw",
        "active",
    );
    let legacy = document(
        "legacy",
        "project://project/a.md",
        source,
        body,
        "raw",
        "active",
    );
    let other_source = document(
        "other-source",
        "project://other/a.md",
        "/vault/other/a.md",
        body,
        "raw",
        "active",
    );
    let wiki = document("wiki", "wiki://same-body", source, body, "wiki", "active");
    let archived = document(
        "archived",
        "project://archive/a.md",
        source,
        body,
        "raw",
        "archived",
    );
    for doc in [&canonical, &legacy, &other_source, &wiki, &archived] {
        store.documents().upsert_document(doc).unwrap();
    }
    store
        .chunks()
        .insert_chunks(&[
            Chunk {
                id: "canonical-chunk".into(),
                document_id: canonical.id.clone(),
                chunk_index: 0,
                content: body.into(),
                embedding: vec![1.0, 0.0],
                char_start: 0,
                char_end: body.len() as i32,
                metadata_json: "{}".into(),
            },
            Chunk {
                id: "legacy-chunk".into(),
                document_id: legacy.id.clone(),
                chunk_index: 0,
                content: body.into(),
                embedding: vec![1.0, 0.0],
                char_start: 0,
                char_end: body.len() as i32,
                metadata_json: "{}".into(),
            },
        ])
        .unwrap();
    for node in [
        graph_node("canonical-node", Some("canonical")),
        graph_node("legacy-node", Some("legacy")),
        graph_node("external-node", None),
    ] {
        store.graph().upsert_graph_node(&node).unwrap();
    }
    store
        .graph()
        .insert_graph_edges(&[
            edge("z-canonical-out", "canonical-node", "external-node", "same"),
            edge("a-legacy-out", "legacy-node", "external-node", "same"),
            edge("legacy-in", "external-node", "legacy-node", "unique"),
        ])
        .unwrap();
    let hash = content_hash(body);
    store
        .source_manifest()
        .upsert_source_manifest(SourceManifestWrite {
            canonical_path: source,
            canonical_root: "/vault/project",
            size_bytes: body.len() as u64,
            mtime_ns: 1,
            content_hash: &hash,
            document_id: "legacy",
        })
        .unwrap();
    {
        let conn = store.lock().unwrap();
        conn.execute(
            r#"INSERT INTO kg_facts
                   (id, subject, predicate, object, status, source_document_id,
                    metadata_json, created_at, updated_at)
                   VALUES ('fact', 'a', 'rel', 'b', 'active', 'legacy', '{}', now(), now())"#,
            [],
        )
        .unwrap();
    }

    let preview = store
        .maintenance()
        .cleanup_source_duplicates(true, false, 10)
        .unwrap();
    assert!(preview.dry_run);
    assert!(!preview.applied);
    assert_eq!(preview.candidate_count, 1);
    assert_eq!(preview.deleted_documents, 0);
    assert!(store.documents().get_document("legacy").unwrap().is_some());
    assert_eq!(preview.groups[0].members.len(), 2);

    let applied = store
        .maintenance()
        .cleanup_source_duplicates(false, true, 10)
        .unwrap();
    assert!(applied.applied);
    assert_eq!(applied.deleted_documents, 1);
    assert_eq!(applied.rewired_graph_edge_endpoints, 1);
    assert_eq!(applied.deduplicated_graph_edges, 1);
    assert_eq!(applied.rewired_kg_facts, 1);
    assert_eq!(applied.rebound_source_manifests, 1);
    assert!(store
        .documents()
        .get_document("canonical")
        .unwrap()
        .is_some());
    assert!(store.documents().get_document("legacy").unwrap().is_none());
    assert!(store
        .documents()
        .get_document("other-source")
        .unwrap()
        .is_some());
    assert!(store.documents().get_document("wiki").unwrap().is_some());
    assert!(store
        .documents()
        .get_document("archived")
        .unwrap()
        .is_some());
    assert!(store
        .chunks()
        .list_chunks_for_document("legacy")
        .unwrap()
        .is_empty());
    assert_eq!(
        store
            .chunks()
            .list_chunks_for_document("canonical")
            .unwrap()
            .len(),
        1
    );

    let conn = store.lock().unwrap();
    let manifest_document: String = conn
        .query_row(
            "SELECT document_id FROM source_manifest WHERE canonical_path = ?",
            params![source],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(manifest_document, "canonical");
    let fact_document: String = conn
        .query_row(
            "SELECT source_document_id FROM kg_facts WHERE id = 'fact'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fact_document, "canonical");
    let legacy_nodes: i64 = conn
        .query_row(
            "SELECT COUNT(*)::BIGINT FROM graph_nodes WHERE document_id = 'legacy'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy_nodes, 0);
    let canonical_edges: i64 = conn
            .query_row(
                "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE source_id = 'canonical-node' OR target_id = 'canonical-node'",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(canonical_edges, 2);
    let preserved_canonical_edge: i64 = conn
        .query_row(
            "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE id = 'z-canonical-out'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(preserved_canonical_edge, 1);
    let removed_legacy_edge: i64 = conn
        .query_row(
            "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE id = 'a-legacy-out'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(removed_legacy_edge, 0);
    let dangling_edges: i64 = conn
            .query_row(
                "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE source_id = 'legacy-node' OR target_id = 'legacy-node'",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(dangling_edges, 0);
}

#[test]
fn cleanup_requires_confirm_and_drains_an_oversized_group_in_bounded_batches() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("source-dedupe-cap.duckdb")).unwrap();
    let source = "/vault/cap.md";
    let body = "cap body";
    for doc in [
        document(
            "canonical",
            "file:///vault/cap.md",
            source,
            body,
            "raw",
            "active",
        ),
        document("legacy-a", "project://cap/a", source, body, "raw", "active"),
        document("legacy-b", "project://cap/b", source, body, "raw", "active"),
    ] {
        store.documents().upsert_document(&doc).unwrap();
    }

    let error = store
        .maintenance()
        .cleanup_source_duplicates(false, false, 10)
        .unwrap_err();
    assert!(error.to_string().contains("confirm=true"));

    let first = store
        .maintenance()
        .cleanup_source_duplicates(false, true, 1)
        .unwrap();
    assert!(first.cap_exceeded);
    assert!(first.applied);
    assert_eq!(first.candidate_count, 2);
    assert_eq!(first.batch_groups, 1);
    assert_eq!(first.batch_candidates, 1);
    assert_eq!(first.remaining_groups, 1);
    assert_eq!(first.remaining_candidates, 1);
    assert_eq!(first.deleted_documents, 1);
    assert_eq!(first.groups[0].batch_candidate_count, 1);
    assert_eq!(first.groups[0].remaining_candidate_count, 1);
    assert!(store
        .documents()
        .get_document("legacy-a")
        .unwrap()
        .is_none());
    assert!(store
        .documents()
        .get_document("legacy-b")
        .unwrap()
        .is_some());

    let second = store
        .maintenance()
        .cleanup_source_duplicates(false, true, 1)
        .unwrap();
    assert!(!second.cap_exceeded);
    assert!(second.applied);
    assert_eq!(second.batch_candidates, 1);
    assert_eq!(second.remaining_groups, 0);
    assert_eq!(second.remaining_candidates, 0);
    assert!(store
        .documents()
        .get_document("legacy-b")
        .unwrap()
        .is_none());

    let converged = store
        .maintenance()
        .cleanup_source_duplicates(false, true, 1)
        .unwrap();
    assert_eq!(converged.candidate_count, 0);
    assert_eq!(converged.batch_candidates, 0);
    assert!(!converged.applied);
}

#[test]
fn cleanup_promotes_a_legacy_graph_node_when_survivor_has_none() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("source-dedupe-promote.duckdb")).unwrap();
    let source = "/vault/promote.md";
    let body = "promote body";
    for doc in [
        document(
            "canonical",
            "file:///vault/promote.md",
            source,
            body,
            "raw",
            "active",
        ),
        document(
            "legacy",
            "project://promote.md",
            source,
            body,
            "raw",
            "active",
        ),
    ] {
        store.documents().upsert_document(&doc).unwrap();
    }
    let mut legacy_node = graph_node("legacy-node", Some("legacy"));
    legacy_node.metadata_json = r#"{"preserved":true}"#.into();
    store.graph().upsert_graph_node(&legacy_node).unwrap();
    store
        .graph()
        .upsert_graph_node(&graph_node("external", None))
        .unwrap();
    store
        .graph()
        .insert_graph_edges(&[edge("incoming", "external", "legacy-node", "keep")])
        .unwrap();

    let report = store
        .maintenance()
        .cleanup_source_duplicates(false, true, 10)
        .unwrap();
    assert!(report.applied);
    assert_eq!(report.deleted_documents, 1);
    let conn = store.lock().unwrap();
    let (document_id, uri, metadata): (String, String, String) = conn
        .query_row(
            "SELECT document_id, uri, metadata_json FROM graph_nodes WHERE id = 'legacy-node'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(document_id, "canonical");
    assert_eq!(uri, "file:///vault/promote.md");
    assert_eq!(metadata, r#"{"preserved":true}"#);
    // §3: the promoted label and its match key must move together.
    let (label, label_key): (String, Option<String>) = conn
        .query_row(
            "SELECT label, label_key FROM graph_nodes WHERE id = 'legacy-node'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        label_key.as_deref(),
        Some(rag_mcp_markup::normalize::label_key(&label).as_str())
    );
    let preserved_edge: i64 = conn
            .query_row(
                "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE id = 'incoming' AND target_id = 'legacy-node'",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(preserved_edge, 1);
}

#[test]
fn cleanup_does_not_create_graph_state_when_duplicates_have_no_nodes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("source-dedupe-no-graph.duckdb")).unwrap();
    let source = "/vault/no-graph.md";
    let body = "no graph body";
    for doc in [
        document(
            "canonical",
            "file:///vault/no-graph.md",
            source,
            body,
            "raw",
            "active",
        ),
        document(
            "legacy",
            "project://no-graph.md",
            source,
            body,
            "raw",
            "active",
        ),
    ] {
        store.documents().upsert_document(&doc).unwrap();
    }

    let report = store
        .maintenance()
        .cleanup_source_duplicates(false, true, 10)
        .unwrap();
    assert!(report.applied);
    assert_eq!(report.deleted_documents, 1);
    assert_eq!(report.rewired_graph_edge_endpoints, 0);
    let graph_nodes: i64 = store
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*)::BIGINT FROM graph_nodes", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(graph_nodes, 0);
}

#[test]
fn cleanup_removes_only_edges_that_its_node_collapse_would_make_self_loops() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("source-dedupe-self-edges.duckdb")).unwrap();
    let source = "/vault/self-edges.md";
    let body = "same graph body";
    for doc in [
        document(
            "canonical",
            "file:///vault/self-edges.md",
            source,
            body,
            "raw",
            "active",
        ),
        document(
            "legacy",
            "project://self-edges.md",
            source,
            body,
            "raw",
            "active",
        ),
    ] {
        store.documents().upsert_document(&doc).unwrap();
    }
    for node in [
        graph_node("canonical-node", Some("canonical")),
        graph_node("legacy-node", Some("legacy")),
        graph_node("external-node", None),
    ] {
        store.graph().upsert_graph_node(&node).unwrap();
    }
    store
        .graph()
        .insert_graph_edges(&[
            edge(
                "preexisting-self",
                "canonical-node",
                "canonical-node",
                "preexisting",
            ),
            edge(
                "canonical-to-legacy",
                "canonical-node",
                "legacy-node",
                "collapse",
            ),
            edge(
                "legacy-to-canonical",
                "legacy-node",
                "canonical-node",
                "collapse",
            ),
            edge("legacy-self", "legacy-node", "legacy-node", "collapse"),
            edge(
                "legacy-external",
                "legacy-node",
                "external-node",
                "preserve",
            ),
        ])
        .unwrap();

    let report = store
        .maintenance()
        .cleanup_source_duplicates(false, true, 10)
        .unwrap();
    assert!(report.applied);
    assert_eq!(report.removed_graph_self_edges, 3);
    assert_eq!(report.rewired_graph_edge_endpoints, 1);
    let conn = store.lock().unwrap();
    let preexisting_self: i64 = conn
            .query_row(
                "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE id = 'preexisting-self' AND source_id = 'canonical-node' AND target_id = 'canonical-node'",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(preexisting_self, 1);
    let new_self_edges: i64 = conn
            .query_row(
                "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE source_id = target_id AND id <> 'preexisting-self'",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(new_self_edges, 0);
    let preserved_external: i64 = conn
            .query_row(
                "SELECT COUNT(*)::BIGINT FROM graph_edges WHERE id = 'legacy-external' AND source_id = 'canonical-node' AND target_id = 'external-node'",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(preserved_external, 1);
}

#[test]
fn cleanup_skips_candidate_that_owns_a_foreign_manifest_path() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("source-dedupe-manifest.duckdb")).unwrap();
    let source = "/vault/owned.md";
    let body = "owned body";
    for doc in [
        document(
            "canonical",
            "file:///vault/owned.md",
            source,
            body,
            "raw",
            "active",
        ),
        document(
            "legacy",
            "project://owned.md",
            source,
            body,
            "raw",
            "active",
        ),
    ] {
        store.documents().upsert_document(&doc).unwrap();
    }
    let hash = content_hash(body);
    store
        .source_manifest()
        .upsert_source_manifest(SourceManifestWrite {
            canonical_path: "/vault/foreign.md",
            canonical_root: "/vault",
            size_bytes: body.len() as u64,
            mtime_ns: 1,
            content_hash: &hash,
            document_id: "legacy",
        })
        .unwrap();

    let report = store
        .maintenance()
        .cleanup_source_duplicates(false, true, 10)
        .unwrap();
    assert_eq!(report.actionable_groups, 0);
    assert_eq!(report.skipped_groups, 1);
    assert!(report.groups[0]
        .skip_reasons
        .iter()
        .any(|reason| reason.contains("foreign source_manifest references")));
    assert!(store.documents().get_document("legacy").unwrap().is_some());
    let conn = store.lock().unwrap();
    let manifest_document_id: String = conn
        .query_row(
            "SELECT document_id FROM source_manifest WHERE canonical_path = '/vault/foreign.md'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(manifest_document_id, "legacy");
}

#[test]
fn cleanup_skips_manifest_path_owned_by_a_non_member_document() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("source-dedupe-third-owner.duckdb")).unwrap();
    let source = "/vault/duplicate.md";
    let body = "duplicate body";
    for doc in [
        document(
            "canonical",
            "file:///vault/duplicate.md",
            source,
            body,
            "raw",
            "active",
        ),
        document(
            "legacy",
            "project://duplicate.md",
            source,
            body,
            "raw",
            "active",
        ),
        document(
            "third-owner",
            "file:///vault/third.md",
            "/vault/third.md",
            "different body",
            "raw",
            "active",
        ),
    ] {
        store.documents().upsert_document(&doc).unwrap();
    }
    let hash = content_hash(body);
    store
        .source_manifest()
        .upsert_source_manifest(SourceManifestWrite {
            canonical_path: source,
            canonical_root: "/vault",
            size_bytes: body.len() as u64,
            mtime_ns: 1,
            content_hash: &hash,
            document_id: "third-owner",
        })
        .unwrap();

    let report = store
        .maintenance()
        .cleanup_source_duplicates(false, true, 10)
        .unwrap();
    assert_eq!(report.actionable_groups, 0);
    assert_eq!(report.skipped_groups, 1);
    assert!(!report.applied);
    assert!(report.groups[0]
        .skip_reasons
        .iter()
        .any(|reason| reason.contains("owned by non-member document")));
    assert!(store.documents().get_document("legacy").unwrap().is_some());
    let manifest_document_id: String = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT document_id FROM source_manifest WHERE canonical_path = ?",
            params![source],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(manifest_document_id, "third-owner");
}

#[test]
fn cleanup_skips_candidate_with_revision_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("source-dedupe-revisions.duckdb")).unwrap();
    let source = "/vault/history.md";
    let body = "same current body";
    let canonical = document(
        "canonical",
        "file:///vault/history.md",
        source,
        body,
        "raw",
        "active",
    );
    let mut legacy = document(
        "legacy",
        "project://history.md",
        source,
        body,
        "raw",
        "active",
    );
    store.documents().upsert_document(&canonical).unwrap();
    store.documents().upsert_document(&legacy).unwrap();
    legacy.title = "legacy updated title".into();
    legacy.updated_at = Utc::now();
    store.documents().upsert_document(&legacy).unwrap();
    assert_eq!(
        store
            .documents()
            .list_document_revision_summaries("legacy", 10, 0)
            .unwrap()
            .total,
        1
    );

    let report = store
        .maintenance()
        .cleanup_source_duplicates(false, true, 10)
        .unwrap();
    assert_eq!(report.actionable_groups, 0);
    assert_eq!(report.skipped_groups, 1);
    assert!(!report.applied);
    assert!(report.groups[0]
        .skip_reasons
        .iter()
        .any(|reason| reason.contains("document revisions")));
    assert!(store.documents().get_document("legacy").unwrap().is_some());
    assert_eq!(
        store
            .documents()
            .list_document_revision_summaries("legacy", 10, 0)
            .unwrap()
            .total,
        1
    );
}

#[test]
fn cleanup_skips_when_only_legacy_candidate_has_search_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("source-dedupe-chunks.duckdb")).unwrap();
    let source = "/vault/chunks.md";
    let body = "searchable duplicate body";
    for doc in [
        document(
            "canonical",
            "file:///vault/chunks.md",
            source,
            body,
            "raw",
            "active",
        ),
        document(
            "legacy",
            "project://chunks.md",
            source,
            body,
            "raw",
            "active",
        ),
    ] {
        store.documents().upsert_document(&doc).unwrap();
    }
    store
        .chunks()
        .insert_chunks(&[Chunk {
            id: "legacy-only-chunk".into(),
            document_id: "legacy".into(),
            chunk_index: 0,
            content: body.into(),
            embedding: vec![1.0, 0.0],
            char_start: 0,
            char_end: body.len() as i32,
            metadata_json: "{}".into(),
        }])
        .unwrap();

    let report = store
        .maintenance()
        .cleanup_source_duplicates(false, true, 10)
        .unwrap();
    assert_eq!(report.actionable_groups, 0);
    assert_eq!(report.skipped_groups, 1);
    assert!(!report.applied);
    assert!(report.groups[0]
        .skip_reasons
        .iter()
        .any(|reason| reason.contains("canonical survivor has no chunks")));
    assert!(store.documents().get_document("legacy").unwrap().is_some());
    assert_eq!(
        store
            .chunks()
            .list_chunks_for_document("legacy")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn cleanup_preview_bounds_group_and_member_details_but_keeps_totals() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("source-dedupe-report-bound.duckdb")).unwrap();

    let dense_source = "/vault/000-dense.md";
    let dense_body = "dense duplicate body";
    store
        .documents()
        .upsert_document(&document(
            "dense-canonical",
            "file:///vault/000-dense.md",
            dense_source,
            dense_body,
            "raw",
            "active",
        ))
        .unwrap();
    for index in 0..(REPORT_MEMBER_LIMIT + 5) {
        store
            .documents()
            .upsert_document(&document(
                &format!("dense-legacy-{index:04}"),
                &format!("project://dense/{index}"),
                dense_source,
                dense_body,
                "raw",
                "active",
            ))
            .unwrap();
    }

    for index in 0..REPORT_GROUP_LIMIT {
        let source = format!("/vault/group-{index:04}.md");
        let body = format!("group body {index}");
        store
            .documents()
            .upsert_document(&document(
                &format!("group-canonical-{index:04}"),
                &format!("file://{source}"),
                &source,
                &body,
                "raw",
                "active",
            ))
            .unwrap();
        store
            .documents()
            .upsert_document(&document(
                &format!("group-legacy-{index:04}"),
                &format!("project://group/{index}"),
                &source,
                &body,
                "raw",
                "active",
            ))
            .unwrap();
    }

    let report = store
        .maintenance()
        .cleanup_source_duplicates(true, false, 1_000)
        .unwrap();
    assert_eq!(report.matched_groups, REPORT_GROUP_LIMIT + 1);
    assert_eq!(
        report.candidate_count,
        (REPORT_MEMBER_LIMIT + 5 + REPORT_GROUP_LIMIT) as u64
    );
    assert_eq!(report.reported_groups, REPORT_GROUP_LIMIT);
    assert_eq!(report.groups.len(), REPORT_GROUP_LIMIT);
    assert!(report.groups_truncated);
    assert_eq!(report.reported_members, REPORT_MEMBER_LIMIT);
    assert!(report.members_truncated);
    assert_eq!(
        report.groups[0].document_count,
        (REPORT_MEMBER_LIMIT + 6) as u64
    );
    assert_eq!(report.groups[0].members.len(), REPORT_MEMBER_LIMIT);
    assert!(report.groups[0].members_truncated);
}

#[test]
fn cleanup_report_prioritizes_the_applied_group_before_truncation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("source-dedupe-report-priority.duckdb")).unwrap();
    let body = "same body";
    for index in 0..REPORT_GROUP_LIMIT {
        let source = format!("/vault/aaa-skipped-{index:04}.md");
        for suffix in ["a", "b"] {
            store
                .documents()
                .upsert_document(&document(
                    &format!("skipped-{index:04}-{suffix}"),
                    &format!("project://skipped/{index}/{suffix}"),
                    &source,
                    body,
                    "raw",
                    "active",
                ))
                .unwrap();
        }
    }
    let selected_source = "/vault/zzz-selected.md";
    for doc in [
        document(
            "selected-canonical",
            "file:///vault/zzz-selected.md",
            selected_source,
            body,
            "raw",
            "active",
        ),
        document(
            "selected-legacy",
            "project://selected/legacy",
            selected_source,
            body,
            "raw",
            "active",
        ),
    ] {
        store.documents().upsert_document(&doc).unwrap();
    }

    let report = store
        .maintenance()
        .cleanup_source_duplicates(false, true, 1)
        .unwrap();
    assert!(report.applied);
    assert_eq!(report.deleted_documents, 1);
    assert_eq!(report.matched_groups, REPORT_GROUP_LIMIT + 1);
    assert!(report.groups_truncated);
    assert!(report
        .groups
        .iter()
        .any(|group| { group.source_file == selected_source && group.batch_candidate_count == 1 }));
}

#[test]
fn ambiguous_and_user_referenced_groups_are_reported_and_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("source-dedupe-skips.duckdb")).unwrap();
    let body = "same";
    let no_canonical = "/vault/no-canonical.md";
    for doc in [
        document(
            "old-a",
            "project://old/a",
            no_canonical,
            body,
            "raw",
            "active",
        ),
        document(
            "old-b",
            "project://old/b",
            no_canonical,
            body,
            "raw",
            "active",
        ),
    ] {
        store.documents().upsert_document(&doc).unwrap();
    }

    let protected = "/vault/protected.md";
    for doc in [
        document(
            "protected-canonical",
            "file:///vault/protected.md",
            protected,
            body,
            "raw",
            "active",
        ),
        document(
            "protected-legacy",
            "project://protected/legacy",
            protected,
            body,
            "raw",
            "active",
        ),
    ] {
        store.documents().upsert_document(&doc).unwrap();
    }
    {
        let conn = store.lock().unwrap();
        conn.execute(
                "INSERT INTO collections (id, name, metadata_json, created_at, updated_at) VALUES ('c', 'C', '{}', now(), now())",
                [],
            )
            .unwrap();
        conn.execute(
                "INSERT INTO collection_entries (collection_id, document_id, position) VALUES ('c', 'protected-legacy', 0)",
                [],
            )
            .unwrap();
    }

    let report = store
        .maintenance()
        .cleanup_source_duplicates(false, true, 10)
        .unwrap();
    assert_eq!(report.matched_groups, 2);
    assert_eq!(report.actionable_groups, 0);
    assert_eq!(report.skipped_groups, 2);
    assert_eq!(report.candidate_count, 0);
    assert!(!report.applied);
    assert!(store.documents().get_document("old-a").unwrap().is_some());
    assert!(store
        .documents()
        .get_document("protected-legacy")
        .unwrap()
        .is_some());
    assert!(report.groups.iter().any(|group| group
        .skip_reasons
        .iter()
        .any(|reason| reason.contains("canonical URI"))));
    assert!(report.groups.iter().any(|group| group
        .skip_reasons
        .iter()
        .any(|reason| reason.contains("collection references"))));
}
