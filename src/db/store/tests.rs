use super::*;
use crate::models::GraphNode;
use chrono::Utc;

fn open_temp() -> Store {
    crate::testkit::temp_store()
}

fn sample_doc(id: &str, uri: &str) -> Document {
    let now = Utc::now();
    Document {
        id: id.into(),
        uri: uri.into(),
        title: "Title".into(),
        content: "Hello world".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        ..Default::default()
    }
}

#[test]
fn prune_orphans_removes_edges_exposed_by_node_deletion_in_one_pass() {
    let store = open_temp();
    {
        let conn = store.lock().unwrap();
        conn.execute("INSERT INTO graph_nodes (id, uri, label, kind, document_id, resolved, created_at, updated_at) VALUES ('lost', 'doc://lost', 'Lost', 'document', 'missing', true, current_timestamp, current_timestamp), ('kept', 'tag://kept', 'Kept', 'tag', NULL, true, current_timestamp, current_timestamp)", []).unwrap();
        conn.execute("INSERT INTO graph_edges (id, source_id, target_id, rel_type, weight, created_at) VALUES ('edge', 'lost', 'kept', 'related', 1.0, current_timestamp)", []).unwrap();
    }
    let before = store.integrity_counts().unwrap();
    assert_eq!((before.2, before.3), (1, 0));
    assert_eq!(store.prune_orphans(true).unwrap(), (0, 1, 0));
    store.prune_orphans(false).unwrap();
    let after = store.integrity_counts().unwrap();
    assert_eq!((after.1, after.2, after.3), (0, 0, 0));
    assert_eq!(store.prune_orphans(false).unwrap(), (0, 0, 0));
}

#[test]
fn prune_orphans_failure_rolls_back_and_releases_the_connection() {
    let store = open_temp();
    {
        let conn = store.lock().unwrap();
        conn.execute("INSERT INTO graph_nodes (id, uri, label, kind, document_id, resolved, created_at, updated_at) VALUES ('lost', 'doc://lost', 'Lost', 'document', 'missing', true, current_timestamp, current_timestamp)", []).unwrap();
    }
    // Fail after the graph-node delete has already run in the same unit of work.
    PRUNE_ORPHANS_FAIL_AFTER_NODES.with(|flag| flag.set(true));
    let result = store.prune_orphans(false);
    PRUNE_ORPHANS_FAIL_AFTER_NODES.with(|flag| flag.set(false));
    assert!(result.is_err());
    let conn = store.lock().unwrap();
    let lost: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_nodes WHERE id = 'lost'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(lost, 1, "partial prune must roll back");
    conn.execute_batch("BEGIN TRANSACTION; COMMIT;")
        .expect("connection must not be left inside an open transaction");
}

#[test]
fn unchunked_document_audit_failure_writes_nothing() {
    let store = open_temp();
    let doc = Document {
        id: "schema-doc".into(),
        uri: "schema://agents".into(),
        title: "Schema".into(),
        content: "links [[Target]]".into(),
        metadata_json: "{}".into(),
        layer: "schema".into(),
        kind: "schema".into(),
        ..Default::default()
    };
    let bad_audit = OpsLogEntry {
        op: "   ".into(),
        payload_json: "{}".into(),
        ..Default::default()
    };
    assert!(store
        .write_unchunked_document_with_audit(&doc, &bad_audit)
        .is_err());
    assert!(store.get_document("schema-doc").unwrap().is_none());
    assert!(store
        .find_node_by_document_id("schema-doc")
        .unwrap()
        .is_none());

    let audit = OpsLogEntry {
        op: "schema_init".into(),
        ..bad_audit
    };
    store
        .write_unchunked_document_with_audit(&doc, &audit)
        .unwrap();
    assert!(store.get_document("schema-doc").unwrap().is_some());
    assert!(store
        .find_node_by_document_id("schema-doc")
        .unwrap()
        .is_some());
    assert_eq!(store.list_ops_log(10).unwrap().len(), 1);
}

#[test]
fn ops_log_pages_are_bounded_for_every_caller() {
    let store = open_temp();
    for n in 0..(MAX_OPS_LOG_PAGE + 3) {
        store
            .append_ops_log(&OpsLogEntry {
                op: format!("op_{n}"),
                payload_json: "{}".into(),
                ..Default::default()
            })
            .unwrap();
    }
    assert_eq!(
        store.list_recent_ops(usize::MAX).unwrap().len(),
        MAX_OPS_LOG_PAGE
    );
    assert_eq!(
        store.read_ops_log(None, None, Some(10_000)).unwrap().len(),
        MAX_OPS_LOG_PAGE
    );
}

#[test]
fn read_only_open_requires_an_existing_file_and_never_creates_parents() {
    let root = tempfile::tempdir().expect("tempdir");
    let missing_parent = root.path().join("missing");
    let missing = missing_parent.join("rag.duckdb");

    let error = match Store::open_read_only(&missing) {
        Ok(_) => panic!("missing DB must fail"),
        Err(error) => error,
    };

    assert!(matches!(error, AppError::NotFound(_)));
    assert!(error.to_string().contains("does not exist"));
    assert!(!missing.exists());
    assert!(!missing_parent.exists());
}

#[test]
fn read_only_open_supports_queries_rejects_writes_and_preserves_backup_bytes() {
    let root = tempfile::tempdir().expect("tempdir");
    let source = root.path().join("source.duckdb");
    let backup = root.path().join("backup.duckdb");
    let writer = Store::open(&source).expect("create source");
    writer
        .upsert_document(&sample_doc("kept", "recovery://kept"))
        .expect("seed source");
    drop(writer);
    std::fs::copy(&source, &backup).expect("copy backup");
    let bytes_before = std::fs::read(&backup).expect("backup bytes");
    let modified_before = std::fs::metadata(&backup)
        .expect("backup metadata")
        .modified()
        .expect("backup mtime");

    let reader = Store::open_read_only(&backup).expect("open backup read-only");
    assert_eq!(reader.stats().expect("read counts").0, 1);
    assert_eq!(
        reader
            .get_document("kept")
            .expect("read document")
            .expect("seeded document")
            .uri,
        "recovery://kept"
    );
    let mutation_error = reader
        .upsert_document(&sample_doc("blocked", "recovery://blocked"))
        .expect_err("read-only store must reject mutation");
    assert!(mutation_error
        .to_string()
        .to_ascii_lowercase()
        .contains("read-only"));
    drop(reader);

    assert_eq!(
        std::fs::read(&backup).expect("backup after read"),
        bytes_before
    );
    assert_eq!(
        std::fs::metadata(&backup)
            .expect("backup metadata after read")
            .modified()
            .expect("backup mtime after read"),
        modified_before
    );
    assert!(!PathBuf::from(format!("{}.wal", backup.display())).exists());
}

#[test]
fn read_only_open_rejects_old_schema_without_migrating_it() {
    let root = tempfile::tempdir().expect("tempdir");
    let old = root.path().join("old.duckdb");
    drop(Store::open(&old).expect("create current DB"));
    let conn = Connection::open(&old).expect("open old fixture for setup");
    conn.execute("DELETE FROM schema_version", [])
        .expect("remove current version");
    conn.execute(
            "INSERT INTO schema_version (version, applied_at, note) VALUES (?, CURRENT_TIMESTAMP, 'old fixture')",
            params![schema::SCHEMA_VERSION - 1],
        )
        .expect("record old version");
    drop(conn);
    let bytes_before = std::fs::read(&old).expect("old DB bytes");

    let error = match Store::open_read_only(&old) {
        Ok(_) => panic!("old schema must fail clearly"),
        Err(error) => error,
    };

    assert!(matches!(error, AppError::Config(_)));
    assert!(error.to_string().contains("incompatible schema version"));
    assert_eq!(
        std::fs::read(&old).expect("old DB after inspect"),
        bytes_before
    );
    assert!(!PathBuf::from(format!("{}.wal", old.display())).exists());
}

#[test]
fn upsert_get_find_list_delete_and_stats() {
    let store = open_temp();
    let doc = sample_doc("d1", "file://a.md");
    store.upsert_document(&doc).unwrap();

    let got = store.get_document("d1").unwrap().expect("doc");
    assert_eq!(got.uri, "file://a.md");
    assert_eq!(got.content, "Hello world");
    assert_eq!(got.layer, "raw");
    assert_eq!(got.kind, "document");
    assert_eq!(
        got.content_hash.as_deref(),
        Some(content_hash("Hello world").as_str())
    );

    let by_uri = store.find_by_uri("file://a.md").unwrap().expect("by uri");
    assert_eq!(by_uri.id, "d1");

    let listed = store.list_documents().unwrap();
    assert_eq!(listed.len(), 1);

    let chunk = Chunk {
        id: "c1".into(),
        document_id: "d1".into(),
        chunk_index: 0,
        content: "Hello".into(),
        embedding: vec![0.1, 0.2, 0.3],
        char_start: 0,
        char_end: 5,
        metadata_json: "{}".into(),
    };
    store.insert_chunks(&[chunk]).unwrap();

    let chunks = store.list_chunks_for_document("d1").unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].embedding, vec![0.1, 0.2, 0.3]);

    let all = store.all_chunks_with_embeddings().unwrap();
    assert_eq!(all.len(), 1);

    let representatives = store
        .representative_document_embeddings(1, false, false, false)
        .unwrap();
    assert_eq!(representatives.len(), 1);
    assert_eq!(representatives[0].document_id, "d1");
    assert_eq!(representatives[0].embedding, vec![0.1, 0.2, 0.3]);

    let (docs, ch, nodes, edges) = store.stats().unwrap();
    assert_eq!(docs, 1);
    assert_eq!(ch, 1);
    assert_eq!(nodes, 0);
    assert_eq!(edges, 0);

    let mut revised = got;
    revised.content = "Hello revision two".into();
    revised.updated_at = Utc::now();
    assert_eq!(store.upsert_document_cas(&revised, Some(1)).unwrap(), 2);
    assert_eq!(store.list_document_revisions("d1").unwrap().len(), 1);

    assert!(store.delete_document("d1").unwrap());
    assert!(store.get_document("d1").unwrap().is_none());
    assert!(store.list_chunks_for_document("d1").unwrap().is_empty());
    assert!(store.list_document_revisions("d1").unwrap().is_empty());
    assert!(!store.delete_document("d1").unwrap());
}

/// §3: `label_key` is the single match key, so a title change must move it with
/// `label`. Otherwise a renamed note stops answering `find_node_by_label`.
#[test]
fn graph_label_refresh_keeps_label_key_in_sync() {
    let store = open_temp();
    let mut doc = sample_doc("d1", "file://rename.md");
    doc.title = "Old Name".into();
    store.upsert_document(&doc).unwrap();
    store
        .upsert_graph_node(&GraphNode {
            id: "n1".into(),
            kind: "document".into(),
            label: "Old Name".into(),
            document_id: Some("d1".into()),
            uri: Some("file://rename.md".into()),
            resolved: true,
            metadata_json: "{}".into(),
        })
        .unwrap();

    doc.title = "  Renamed   TITLE ".into();
    let key = crate::graph::normalize::label_key(&doc.title);
    {
        let conn = store.lock().unwrap();
        refresh_document_graph_label_locked(&conn, &doc).unwrap();
    }

    let node = store.find_node_by_id("n1").unwrap().expect("node");
    assert_eq!(node.label, "  Renamed   TITLE ");
    let by_label = store.find_nodes_by_label(&key).unwrap();
    assert_eq!(by_label.len(), 1);
    assert_eq!(by_label[0].id, "n1");
}

#[test]
fn repeated_indexed_document_updates_survive_checkpoint_and_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("art-regression.duckdb");
    {
        let store = Store::open(&path).expect("open store");
        let mut doc = sample_doc("stable-id", "file://project/README.md");
        for revision in 0..50 {
            doc.title = format!("Title {revision}");
            doc.content = format!("content revision {revision}");
            doc.content_hash = Some(content_hash(&doc.content));
            doc.wing = Some(format!("project-{}", revision % 3));
            doc.room = Some(format!("room-{}", revision % 5));
            doc.updated_at = Utc::now();
            store.upsert_document(&doc).expect("indexed UPDATE");
        }
        store.checkpoint().expect("checkpoint");
    }

    let reopened = Store::open(&path).expect("reopen after repeated updates");
    let doc = reopened
        .get_document("stable-id")
        .unwrap()
        .expect("document");
    assert_eq!(doc.title, "Title 49");
    assert_eq!(doc.content, "content revision 49");
    assert_eq!(doc.revision, 50);
    assert_eq!(reopened.integrity_counts().unwrap().1, 0);
}

#[test]
fn atomic_document_dispositions_roll_back_a_partial_delete_prefix() {
    let store = open_temp();
    let document = sample_doc("atomic-delete", "wiki://atomic-delete");
    store.upsert_document(&document).unwrap();
    store
        .insert_chunks(&[Chunk {
            id: "atomic-delete-chunk".into(),
            document_id: document.id.clone(),
            chunk_index: 0,
            content: document.content.clone(),
            embedding: vec![1.0, 0.0],
            char_start: 0,
            char_end: document.content.len() as i32,
            metadata_json: "{}".into(),
        }])
        .unwrap();

    let error = store
        .apply_document_dispositions_atomic(
            &[document.id.clone(), "missing-after-prefix".into()],
            &[],
        )
        .expect_err("missing later target must roll back the entire batch");
    assert!(matches!(error, AppError::NotFound(_)));
    assert!(store.get_document(&document.id).unwrap().is_some());
    assert_eq!(store.count_chunks_for_document(&document.id).unwrap(), 1);
}

#[test]
fn exact_duplicate_merge_rolls_back_after_an_injected_late_failure() {
    let store = open_temp();
    let keep = sample_doc("merge-keep", "wiki://merge-keep");
    let source_a = sample_doc("merge-a", "wiki://merge-a");
    let source_b = sample_doc("merge-b", "wiki://merge-b");
    for document in [&keep, &source_a, &source_b] {
        store.upsert_document(document).unwrap();
        store
            .insert_chunks(&[Chunk {
                id: format!("{}-chunk", document.id),
                document_id: document.id.clone(),
                chunk_index: 0,
                content: document.content.clone(),
                embedding: vec![1.0, 0.0],
                char_start: 0,
                char_end: document.content.len() as i32,
                metadata_json: "{}".into(),
            }])
            .unwrap();
    }

    let error = store
        .merge_exact_duplicates_atomic_with(
            &keep.id,
            &[source_a.id.clone(), source_b.id.clone()],
            Some(&content_hash(&keep.content)),
            true,
            true,
            |mutated| {
                if mutated == 1 {
                    Err(AppError::db("injected merge failure"))
                } else {
                    Ok(())
                }
            },
        )
        .expect_err("late failure must roll back the first source deletion");
    assert!(error.to_string().contains("injected merge failure"));
    for source in [&source_a, &source_b] {
        assert!(store.get_document(&source.id).unwrap().is_some());
        assert_eq!(store.count_chunks_for_document(&source.id).unwrap(), 1);
    }
}

#[test]
fn atomic_document_write_rolls_back_document_chunks_and_graph_on_graph_fault() {
    let store = open_temp();
    let mut original = sample_doc("atomic", "wiki://atomic");
    original.title = "Original title".into();
    original.content = "Original body links to [[Old target]].".into();
    original.layer = "wiki".into();
    original.kind = "wiki".into();
    original.content_hash = Some(content_hash(&original.content));
    let original_chunk = Chunk {
        id: "old-chunk".into(),
        document_id: original.id.clone(),
        chunk_index: 0,
        content: original.content.clone(),
        embedding: vec![0.1, 0.2],
        char_start: 0,
        char_end: original.content.chars().count() as i32,
        metadata_json: "{}".into(),
    };
    let initial = store
        .write_document_atomic(
            &original,
            None,
            DocumentDerivedWrite::ReplaceChunksAndGraph(std::slice::from_ref(&original_chunk)),
        )
        .expect("initial atomic write");
    assert_eq!(initial.revision, 1);
    let generation_after_initial = {
        let conn = store.lock().unwrap();
        crate::db::fts::chunks_generation(&conn).unwrap()
    };
    assert_eq!(generation_after_initial, 1);
    let node_id = initial.node_id.expect("document node");
    let original_edges: Vec<_> = store
        .list_graph_edges()
        .unwrap()
        .into_iter()
        .filter(|edge| edge.source_id == node_id)
        .collect();
    assert_eq!(original_edges.len(), 1);

    // The unique key is a test-only fault injector: the rebuilt graph gets
    // as far as its second identical edge, after document/chunk mutations.
    store
        .lock()
        .unwrap()
        .execute(
            "CREATE UNIQUE INDEX rollback_edge_key ON graph_edges(source_id, target_id, rel_type)",
            [],
        )
        .expect("fault index");

    let mut updated = original.clone();
    updated.title = "Updated title".into();
    updated.content = "Updated body [[Repeated target]] and [[Repeated target]].".into();
    updated.content_hash = Some(content_hash(&updated.content));
    updated.updated_at = Utc::now();
    let replacement_chunk = Chunk {
        id: "new-chunk".into(),
        document_id: updated.id.clone(),
        chunk_index: 0,
        content: updated.content.clone(),
        embedding: vec![0.3, 0.4],
        char_start: 0,
        char_end: updated.content.chars().count() as i32,
        metadata_json: "{}".into(),
    };
    let error = store
        .write_document_atomic(
            &updated,
            Some(initial.revision),
            DocumentDerivedWrite::ReplaceChunksAndGraph(std::slice::from_ref(&replacement_chunk)),
        )
        .expect_err("duplicate graph edge must fail the transaction");
    assert!(
        error.to_string().contains("duplicate")
            || error.to_string().contains("constraint")
            || error.to_string().contains("unique"),
        "unexpected fault: {error}"
    );

    let persisted = store.get_document(&original.id).unwrap().expect("document");
    assert_eq!(persisted.revision, initial.revision);
    assert_eq!(persisted.title, original.title);
    assert_eq!(persisted.content, original.content);
    let chunks = store.list_chunks_for_document(&original.id).unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].id, original_chunk.id);
    assert_eq!(chunks[0].content, original_chunk.content);
    let node = store
        .find_node_by_document_id(&original.id)
        .unwrap()
        .expect("document node after rollback");
    assert_eq!(node.id, node_id);
    assert_eq!(node.label, original.title);
    let edges: Vec<_> = store
        .list_graph_edges()
        .unwrap()
        .into_iter()
        .filter(|edge| edge.source_id == node_id)
        .collect();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].id, original_edges[0].id);
    assert_eq!(edges[0].target_id, original_edges[0].target_id);
    assert_eq!(edges[0].rel_type, original_edges[0].rel_type);
    assert!(store
        .find_nodes_by_label("Repeated target")
        .unwrap()
        .is_empty());
    assert!(store
        .list_document_revisions(&original.id)
        .unwrap()
        .is_empty());
    let generation_after_rollback = {
        let conn = store.lock().unwrap();
        crate::db::fts::chunks_generation(&conn).unwrap()
    };
    assert_eq!(generation_after_rollback, generation_after_initial);
}

#[test]
fn content_hash_find_and_scope_list_delete_by_source() {
    let store = open_temp();
    let now = Utc::now();
    let text = "scoped body";
    let hash = content_hash(text);

    let doc = Document {
        id: "d1".into(),
        uri: "file://vault/a.md".into(),
        title: "A".into(),
        content: text.into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        wing: Some("projects".into()),
        room: Some("alpha".into()),
        source_file: Some("/vault/a.md".into()),
        layer: "raw".into(),
        kind: "document".into(),
        content_hash: None,
        ..Default::default()
    };
    store.upsert_document(&doc).unwrap();

    let wiki = Document {
        id: "d2".into(),
        uri: "wiki://page".into(),
        title: "Wiki".into(),
        content: "compiled".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        wing: Some("projects".into()),
        room: Some("beta".into()),
        source_file: Some("/vault/a.md".into()),
        layer: "wiki".into(),
        kind: "wiki".into(),
        content_hash: None,
        ..Default::default()
    };
    store.upsert_document(&wiki).unwrap();

    let other = Document {
        id: "d3".into(),
        uri: "file://other.md".into(),
        title: "Other".into(),
        content: "other body".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        wing: Some("archive".into()),
        source_file: Some("/vault/other.md".into()),
        ..Default::default()
    };
    store.upsert_document(&other).unwrap();

    let by_hash = store
        .find_by_content_hash(&hash)
        .unwrap()
        .expect("hash hit");
    assert_eq!(by_hash.id, "d1");

    let dup = store.check_duplicate(Some(text), None, None).unwrap();
    assert!(dup.is_duplicate);
    assert_eq!(dup.content_hash.as_deref(), Some(hash.as_str()));
    assert_eq!(dup.matches.len(), 1);
    assert_eq!(dup.matches[0].id, "d1");
    assert_eq!(dup.matches[0].match_reason, "content_hash");

    let dup_uri = store
        .check_duplicate(None, None, Some("file://vault/a.md"))
        .unwrap();
    assert!(dup_uri.is_duplicate);
    assert_eq!(dup_uri.matches[0].match_reason, "uri");

    let dup_both = store
        .check_duplicate(Some(text), None, Some("file://vault/a.md"))
        .unwrap();
    assert!(dup_both.is_duplicate);
    assert_eq!(dup_both.matches[0].match_reason, "content_hash+uri");

    let no_dup = store
        .check_duplicate(Some("totally new text"), None, None)
        .unwrap();
    assert!(!no_dup.is_duplicate);
    assert!(no_dup.matches.is_empty());

    assert!(store.check_duplicate(None, None, None).is_err());

    let raw = store.list_documents_by_layer("raw").unwrap();
    assert_eq!(raw.len(), 2);
    assert!(raw.iter().all(|d| d.layer == "raw"));

    let wiki_list = store.list_documents_by_layer("wiki").unwrap();
    assert_eq!(wiki_list.len(), 1);
    assert_eq!(wiki_list[0].id, "d2");

    let wing = store.list_documents_by_wing("projects").unwrap();
    assert_eq!(wing.len(), 2);

    for id in ["d1", "d2"] {
        store
            .insert_chunks(&[Chunk {
                id: format!("chunk-{id}"),
                document_id: id.into(),
                chunk_index: 0,
                content: "derived".into(),
                embedding: vec![1.0, 0.0],
                char_start: 0,
                char_end: 7,
                metadata_json: "{}".into(),
            }])
            .unwrap();
    }
    store
        .upsert_source_manifest(crate::db::SourceManifestWrite {
            canonical_path: "/vault/a.md",
            canonical_root: "/vault",
            size_bytes: 11,
            mtime_ns: 7,
            content_hash: &hash,
            document_id: "d1",
        })
        .unwrap();
    store
        .upsert_wiki_index_entry(&WikiIndexEntry {
            id: "idx-d2".into(),
            slug: "page".into(),
            title: "Wiki".into(),
            kind: "wiki".into(),
            category: None,
            summary: None,
            page_id: Some("d2".into()),
            updated_at: now,
        })
        .unwrap();

    let deleted = store.delete_by_source("/vault/a.md").unwrap();
    assert_eq!(deleted, 2);
    assert!(store.get_document("d1").unwrap().is_none());
    assert!(store.get_document("d2").unwrap().is_none());
    assert!(store.get_document("d3").unwrap().is_some());
    assert!(store.list_chunks_for_document("d1").unwrap().is_empty());
    assert!(store.list_chunks_for_document("d2").unwrap().is_empty());
    assert!(store.get_wiki_index_by_slug("page").unwrap().is_none());
    let remaining = store
        .load_source_manifest_root(Path::new("/vault"))
        .unwrap();
    assert!(!remaining.contains_key("/vault/a.md"));
    assert!(remaining.contains_key("/vault/other.md"));
    assert_eq!(store.delete_by_source("/vault/a.md").unwrap(), 0);
}

#[test]
fn source_delete_fault_rolls_back_documents_chunks_graph_and_manifest() {
    let store = open_temp();
    let source = "/vault/atomic.md";
    for id in ["atomic-a", "atomic-b"] {
        let mut document = sample_doc(id, &format!("file:///{id}.md"));
        document.title = format!("Document {id}");
        document.source_file = Some(source.into());
        document.content = format!("body for {id}");
        let chunk = Chunk {
            id: format!("chunk-{id}"),
            document_id: id.into(),
            chunk_index: 0,
            content: document.content.clone(),
            embedding: vec![1.0, 0.0],
            char_start: 0,
            char_end: document.content.chars().count() as i32,
            metadata_json: "{}".into(),
        };
        store
            .write_document_atomic(
                &document,
                None,
                DocumentDerivedWrite::ReplaceChunksAndGraph(std::slice::from_ref(&chunk)),
            )
            .unwrap();
    }
    store
        .upsert_source_manifest(crate::db::SourceManifestWrite {
            canonical_path: source,
            canonical_root: "/vault",
            size_bytes: 20,
            mtime_ns: 9,
            content_hash: "manifest-hash",
            document_id: "atomic-a",
        })
        .unwrap();

    let error = store
        .delete_source_state_with(source, |deleted| {
            if deleted == 1 {
                Err(AppError::db("injected source delete failure"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    assert!(error.to_string().contains("injected source delete failure"));
    for id in ["atomic-a", "atomic-b"] {
        assert!(store.get_document(id).unwrap().is_some());
        assert_eq!(store.list_chunks_for_document(id).unwrap().len(), 1);
        assert!(store.find_node_by_document_id(id).unwrap().is_some());
    }
    assert!(store
        .load_source_manifest_root(Path::new("/vault"))
        .unwrap()
        .contains_key(source));

    assert_eq!(store.delete_source_state(source).unwrap(), (2, true));
    for id in ["atomic-a", "atomic-b"] {
        assert!(store.get_document(id).unwrap().is_none());
        assert!(store.list_chunks_for_document(id).unwrap().is_empty());
        assert!(store.find_node_by_document_id(id).unwrap().is_none());
    }
    assert!(!store
        .load_source_manifest_root(Path::new("/vault"))
        .unwrap()
        .contains_key(source));
}

#[test]
fn taxonomy_placement_update_and_filtered_list() {
    let store = open_temp();
    let now = Utc::now();

    for (id, wing, room, status, pinned, boost) in [
        ("a", "research", "rag", "active", true, 2.0),
        ("b", "research", "rag", "active", false, 1.0),
        ("c", "research", "eval", "archived", false, 1.0),
        ("d", "ops", "runbooks", "active", false, 1.5),
        ("e", "", "", "active", false, 1.0),
    ] {
        let wing = if wing.is_empty() {
            None
        } else {
            Some(wing.into())
        };
        let room = if room.is_empty() {
            None
        } else {
            Some(room.into())
        };
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("doc://{id}"),
                title: id.into(),
                content: format!("body {id}"),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                wing,
                room,
                status: status.into(),
                pinned,
                boost,
                ..Default::default()
            })
            .unwrap();
    }

    let wings = store.list_wings().unwrap();
    assert_eq!(wings.len(), 2);
    assert_eq!(wings[0].wing, "ops");
    assert_eq!(wings[0].document_count, 1);
    assert_eq!(wings[1].wing, "research");
    assert_eq!(wings[1].document_count, 3);

    let rooms = store.list_rooms(Some("research")).unwrap();
    assert_eq!(rooms.len(), 2);
    assert!(rooms
        .iter()
        .any(|r| r.room == "rag" && r.document_count == 2));
    assert!(rooms
        .iter()
        .any(|r| r.room == "eval" && r.document_count == 1));

    let tax = store.get_taxonomy().unwrap();
    assert_eq!(tax.total_documents, 5);
    assert_eq!(tax.unscoped_count, 1);
    assert_eq!(tax.wings.len(), 2);
    let research = tax.wings.iter().find(|w| w.wing == "research").unwrap();
    assert_eq!(research.document_count, 3);
    assert_eq!(research.rooms.len(), 2);

    let filtered = store
        .list_documents_filtered(&DocumentFilter {
            wing: Some("research".into()),
            room: Some("rag".into()),
            status: Some("active".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(filtered.len(), 2);
    assert_eq!(filtered[0].id, "a"); // pinned first
    assert_eq!(filtered[1].id, "b");

    let archived = store
        .list_documents_filtered(&DocumentFilter {
            status: Some("archived".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].id, "c");

    // Default list excludes archived (include_archived unset / false).
    let default_list = store
        .list_documents_filtered(&DocumentFilter {
            wing: Some("research".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(default_list.len(), 2);
    assert!(default_list.iter().all(|d| d.id == "a" || d.id == "b"));

    let with_arch = store
        .list_documents_filtered(&DocumentFilter {
            wing: Some("research".into()),
            include_archived: Some(true),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(with_arch.len(), 3);

    let updated = store
        .update_document_placement(
            "b",
            &PlacementUpdate {
                wing: Some("ops".into()),
                room: Some("runbooks".into()),
                status: Some("draft".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .expect("updated");
    assert_eq!(updated.wing.as_deref(), Some("ops"));
    assert_eq!(updated.room.as_deref(), Some("runbooks"));
    assert_eq!(updated.status, "draft");
    assert_eq!(updated.content, "body b"); // body preserved

    let cleared = store
        .update_document_placement(
            "b",
            &PlacementUpdate {
                wing: Some(String::new()),
                room: Some(String::new()),
                ..Default::default()
            },
        )
        .unwrap()
        .expect("cleared");
    assert!(cleared.wing.is_none());
    assert!(cleared.room.is_none());
    assert_eq!(cleared.status, "draft");

    assert!(store
        .update_document_placement("missing", &PlacementUpdate::default())
        .unwrap()
        .is_none());

    // Meta-only update: title / metadata / pin / boost — no content_changed.
    let meta = store
        .update_document_meta(
            "a",
            &DocumentMetaUpdate {
                title: Some("Pinned research note".into()),
                metadata_json: Some(r#"{"topic":"rag"}"#.into()),
                pinned: Some(true),
                boost: Some(3.5),
                status: Some("active".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .expect("meta");
    assert!(!meta.content_changed);
    assert!(meta.title_changed);
    assert_eq!(meta.document.title, "Pinned research note");
    assert_eq!(meta.document.metadata_json, r#"{"topic":"rag"}"#);
    assert!(meta.document.pinned);
    assert!((meta.document.boost - 3.5).abs() < f64::EPSILON);
    assert_eq!(meta.document.content, "body a");

    // Content change on non-raw (status draft from earlier was "b"; use "d" wiki-like).
    store
        .upsert_document(&Document {
            id: "wiki1".into(),
            uri: "wiki://w1".into(),
            title: "W".into(),
            content: "old wiki body".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            layer: "wiki".into(),
            kind: "wiki".into(),
            ..Default::default()
        })
        .unwrap();
    let body_upd = store
        .update_document_meta(
            "wiki1",
            &DocumentMetaUpdate {
                content: Some("new wiki body".into()),
                title: Some("W2".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .expect("body");
    assert!(body_upd.content_changed);
    assert!(body_upd.title_changed);
    assert_eq!(body_upd.document.content, "new wiki body");
    assert_eq!(body_upd.document.title, "W2");
    assert!(body_upd.document.content_hash.is_some());

    // layer=raw refuses content rewrite.
    let err = store
        .update_document_meta(
            "a",
            &DocumentMetaUpdate {
                content: Some("mutated raw".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("immutable") || err.to_string().contains("layer=raw"),
        "unexpected err: {err}"
    );

    let bad_boost = store.update_document_meta(
        "a",
        &DocumentMetaUpdate {
            boost: Some(0.0),
            ..Default::default()
        },
    );
    assert!(bad_boost.is_err());
}

#[test]
fn raw_meta_update_cannot_escape_immutability_by_relabelling() {
    let store = open_temp();
    let mut raw = sample_doc("raw-policy", "file://raw-policy.md");
    raw.source_file = Some("/vault/raw-policy.md".into());
    store.upsert_document(&raw).unwrap();
    let before = store.get_document(&raw.id).unwrap().unwrap();

    let combined = store
        .update_document_meta(
            &raw.id,
            &DocumentMetaUpdate {
                layer: Some("wiki".into()),
                content: Some("rewritten in the same patch".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(combined, AppError::Conflict(_)));

    let transition = store
        .update_document_meta(
            &raw.id,
            &DocumentMetaUpdate {
                layer: Some("wiki".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(transition, AppError::Conflict(_)));
    let after = store.get_document(&raw.id).unwrap().unwrap();
    assert_eq!(after.layer, "raw");
    assert_eq!(after.content, before.content);
    assert_eq!(after.revision, before.revision);

    let mut wiki = sample_doc("freeze-policy", "wiki://freeze-policy");
    wiki.layer = "wiki".into();
    wiki.kind = "wiki".into();
    store.upsert_document(&wiki).unwrap();
    let frozen = store
        .update_document_meta(
            &wiki.id,
            &DocumentMetaUpdate {
                layer: Some("RAW".into()),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(frozen.document.layer, "raw");
    assert!(store
        .update_document_meta(
            &wiki.id,
            &DocumentMetaUpdate {
                content: Some("cannot rewrite after freezing".into()),
                ..Default::default()
            },
        )
        .is_err());
}

#[test]
fn embedding_manifest_get_set() {
    let store = open_temp();
    assert!(store.get_embedding_manifest().unwrap().is_none());

    let m = EmbeddingManifest {
        id: "default".into(),
        provider: "mock".into(),
        model: "mock-embed".into(),
        dims: 8,
        base_url: Some("http://localhost".into()),
        content_fingerprint: Some("fp1".into()),
        updated_at: Utc::now(),
    };
    store.set_embedding_manifest(&m).unwrap();

    let got = store.get_embedding_manifest().unwrap().expect("manifest");
    assert_eq!(got.provider, "mock");
    assert_eq!(got.model, "mock-embed");
    assert_eq!(got.dims, 8);
    assert_eq!(got.base_url.as_deref(), Some("http://localhost"));
    assert_eq!(got.content_fingerprint.as_deref(), Some("fp1"));

    let m2 = EmbeddingManifest {
        id: "default".into(),
        provider: "openai".into(),
        model: "text-embedding-3-small".into(),
        dims: 1536,
        base_url: None,
        content_fingerprint: None,
        updated_at: Utc::now(),
    };
    store.set_embedding_manifest(&m2).unwrap();
    let got2 = store.get_embedding_manifest().unwrap().expect("manifest");
    assert_eq!(got2.provider, "openai");
    assert_eq!(got2.dims, 1536);
    assert!(got2.base_url.is_none());
}

#[test]
fn schema_version_and_fts_probe_do_not_fail() {
    let store = open_temp();
    let ver = store.schema_version().expect("schema_version");
    assert_eq!(ver, Some(schema::SCHEMA_VERSION));
    // May be false when FTS extension is unavailable offline; must not error.
    let _ = store.fts_ready().expect("fts_ready");
}

fn sample_config(dims: usize) -> Config {
    Config {
        db_path: PathBuf::from("./rag.duckdb"),
        embedding_provider: crate::config::EmbeddingProviderKind::Mock,
        embedding_base_url: "https://api.openai.com/v1".to_string(),
        embedding_api_key: String::new(),
        embedding_model: "mock-embed".to_string(),
        embedding_dims: dims,
        chunk_size: 800,
        chunk_overlap: 120,
        default_top_k: 5,
        ingest_roots: Vec::new(),
        max_context_tokens: 4096,
        max_chunks_per_doc: 3,
        fts_stemmer: "porter".to_string(),
        default_search_mode: crate::models::SearchMode::Vec,
        llm_base_url: "http://127.0.0.1:11434/v1".to_string(),
        llm_provider: crate::llm::LlmProviderKind::Ollama,
        llm_model: "llama3.2".to_string(),
        llm_api_key: "ollama".to_string(),
        llm_enabled: false,
        llm_timeout_secs: 120,
        llm_max_tokens: 4096,
        maint_max_docs: 50,
        maint_near_dup_threshold: 0.92,
        tool_surface: crate::mcp::ToolSurface::Full,
        http_bind: None,
        wiki_require_if_match: false,
    }
}

#[test]
fn ensure_embedding_manifest_records_once() {
    let store = open_temp();
    let cfg = sample_config(32);
    assert!(store.get_embedding_manifest().unwrap().is_none());

    let m1 = store.ensure_embedding_manifest(&cfg).unwrap();
    assert_eq!(m1.dims, 32);
    assert_eq!(m1.provider, "mock");

    // Second call must not overwrite when config dims change.
    let cfg2 = sample_config(64);
    let m2 = store.ensure_embedding_manifest(&cfg2).unwrap();
    assert_eq!(m2.dims, 32);

    store.require_embedding_dims_match(32).unwrap();
    let err = store.require_embedding_dims_match(64).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("dimension mismatch") || msg.contains("dims="),
        "unexpected error: {msg}"
    );
}

#[test]
fn ensure_embedding_manifest_refuses_to_self_certify_existing_vectors() {
    let store = open_temp();
    let now = Utc::now();
    store
        .upsert_document(&Document {
            id: "legacy-document".into(),
            uri: "legacy://document".into(),
            title: "Legacy".into(),
            content: "unknown embedding identity".into(),
            created_at: now,
            updated_at: now,
            ..Document::default()
        })
        .unwrap();
    store
        .insert_chunks(&[Chunk {
            id: "legacy-chunk".into(),
            document_id: "legacy-document".into(),
            chunk_index: 0,
            content: "unknown embedding identity".into(),
            embedding: vec![0.5; 32],
            char_start: 0,
            char_end: 26,
            metadata_json: "{}".into(),
        }])
        .unwrap();

    let error = store
        .ensure_embedding_manifest(&sample_config(32))
        .expect_err("unknown vectors must remain fail-closed");
    assert!(error.to_string().contains("non-empty corpus"));
    assert!(error.to_string().contains("uncapped reembed_all"));
    assert!(store.get_embedding_manifest().unwrap().is_none());
}

#[test]
fn canonical_embedding_identity_refuses_legacy_rows_without_fingerprint() {
    let store = open_temp();
    let config = sample_config(32);
    let exact = embedding_manifest_from_config(&config);
    assert!(embedding_manifest_matches_config(&exact, &config));
    store.set_embedding_manifest(&exact).unwrap();
    store.require_embedding_manifest_match(&config).unwrap();

    let mut different_model = config.clone();
    different_model.embedding_model = "different-model-same-dims".into();
    assert!(!embedding_manifest_matches_config(&exact, &different_model));

    let mut stale_fingerprint = exact.clone();
    stale_fingerprint.content_fingerprint = Some("stale-fingerprint".into());
    assert!(!embedding_manifest_matches_config(
        &stale_fingerprint,
        &config
    ));
    store.set_embedding_manifest(&stale_fingerprint).unwrap();
    let mismatch = store
        .require_embedding_manifest_match(&config)
        .expect_err("persisted fingerprint is authoritative");
    assert!(mismatch.to_string().contains("stored fingerprint"));

    let mut forged_fields = exact.clone();
    forged_fields.dims = 999;
    assert!(!embedding_manifest_matches_config(&forged_fields, &config));
    store.set_embedding_manifest(&forged_fields).unwrap();
    store
        .require_embedding_manifest_match(&config)
        .expect_err("matching hash must not authorize forged manifest fields");

    let mut empty_fingerprint = exact.clone();
    empty_fingerprint.content_fingerprint = Some(String::new());
    assert!(!embedding_manifest_matches_config(
        &empty_fingerprint,
        &config
    ));

    let mut legacy = exact;
    legacy.content_fingerprint = None;
    assert!(!embedding_manifest_matches_config(&legacy, &config));
    assert!(!embedding_manifest_matches_config(
        &legacy,
        &different_model
    ));
}

#[test]
fn write_manifest_and_replace_chunks() {
    let store = open_temp();
    let cfg = sample_config(4);
    let m = store.write_embedding_manifest_from_config(&cfg).unwrap();
    assert_eq!(m.dims, 4);

    let now = Utc::now();
    let doc = Document {
        id: "d1".into(),
        uri: "u1".into(),
        title: "t".into(),
        content: "hello world".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        ..Default::default()
    };
    store.upsert_document(&doc).unwrap();
    store
        .insert_chunks(&[Chunk {
            id: "c1".into(),
            document_id: "d1".into(),
            chunk_index: 0,
            content: "hello world".into(),
            embedding: vec![1.0, 0.0, 0.0, 0.0],
            char_start: 0,
            char_end: 11,
            metadata_json: "{}".into(),
        }])
        .unwrap();

    store
        .replace_chunks_for_document(
            "d1",
            &[Chunk {
                id: "c1".into(),
                document_id: "d1".into(),
                chunk_index: 0,
                content: "hello world".into(),
                embedding: vec![0.0, 1.0, 0.0, 0.0],
                char_start: 0,
                char_end: 11,
                metadata_json: "{}".into(),
            }],
        )
        .unwrap();
    let chunks = store.list_chunks_for_document("d1").unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].embedding, vec![0.0, 1.0, 0.0, 0.0]);
}

#[test]
fn embedding_only_update_preserves_rows_and_clean_fts_without_rebuild() {
    let store = open_temp();
    let doc = sample_doc("embedding-only", "wiki://embedding-only");
    store.upsert_document(&doc).unwrap();
    store
        .insert_chunks(&[
            Chunk {
                id: "embedding-only-0".into(),
                document_id: doc.id.clone(),
                chunk_index: 0,
                content: "unchanged first text".into(),
                embedding: vec![1.0, 0.0],
                char_start: 0,
                char_end: 20,
                metadata_json: r#"{"section":"first"}"#.into(),
            },
            Chunk {
                id: "embedding-only-1".into(),
                document_id: doc.id.clone(),
                chunk_index: 1,
                content: "unchanged second text".into(),
                embedding: vec![0.0, 1.0],
                char_start: 21,
                char_end: 42,
                metadata_json: r#"{"section":"second"}"#.into(),
            },
        ])
        .unwrap();
    store.ensure_fts("porter").unwrap();

    let persisted_doc = store.get_document(&doc.id).unwrap().unwrap();
    let expected = store.list_chunks_for_document(&doc.id).unwrap();
    let created_before: Vec<String> = {
        let conn = store.lock().unwrap();
        let mut stmt = conn
                .prepare(
                    "SELECT CAST(created_at AS VARCHAR) FROM chunks WHERE document_id = ? ORDER BY chunk_index, id",
                )
                .unwrap();
        stmt.query_map(params![doc.id], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    };
    let fts_before = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert!(!fts_before.dirty);

    let manifest = EmbeddingManifest {
        id: "default".into(),
        provider: "mock".into(),
        model: "replacement-model".into(),
        dims: 2,
        base_url: None,
        content_fingerprint: Some("replacement-fingerprint".into()),
        updated_at: Utc::now(),
    };
    let updated = store
        .update_chunk_embeddings_atomic(
            &doc.id,
            persisted_doc.revision,
            &expected,
            &[vec![0.25, 0.75], vec![0.75, 0.25]],
            Some(&manifest),
        )
        .unwrap();
    assert_eq!(updated, 2);

    let current_doc = store.get_document(&doc.id).unwrap().unwrap();
    assert_eq!(current_doc.revision, persisted_doc.revision);
    let current = store.list_chunks_for_document(&doc.id).unwrap();
    assert_eq!(current[0].id, expected[0].id);
    assert_eq!(current[0].content, expected[0].content);
    assert_eq!(current[0].char_start, expected[0].char_start);
    assert_eq!(current[0].char_end, expected[0].char_end);
    assert_eq!(current[0].metadata_json, expected[0].metadata_json);
    assert_eq!(current[0].embedding, vec![0.25, 0.75]);
    assert_eq!(current[1].id, expected[1].id);
    assert_eq!(current[1].content, expected[1].content);
    assert_eq!(current[1].embedding, vec![0.75, 0.25]);
    let created_after: Vec<String> = {
        let conn = store.lock().unwrap();
        let mut stmt = conn
                .prepare(
                    "SELECT CAST(created_at AS VARCHAR) FROM chunks WHERE document_id = ? ORDER BY chunk_index, id",
                )
                .unwrap();
        stmt.query_map(params![doc.id], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(created_after, created_before);
    let stored_manifest = store.get_embedding_manifest().unwrap().unwrap();
    assert_eq!(stored_manifest.model, manifest.model);
    assert_eq!(
        stored_manifest.content_fingerprint,
        manifest.content_fingerprint
    );

    let fts_after = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert!(!fts_after.dirty);
    assert_eq!(
        fts_after.chunks_generation,
        fts_before.chunks_generation + 1
    );
    assert_eq!(
        fts_after.index_generation,
        Some(fts_after.chunks_generation)
    );
    assert_eq!(fts_after.rebuild_count, fts_before.rebuild_count);
}

#[test]
fn embedding_only_update_keeps_dirty_fts_dirty_and_rejects_stale_snapshots() {
    let store = open_temp();
    let doc = sample_doc("embedding-cas", "wiki://embedding-cas");
    store.upsert_document(&doc).unwrap();
    store
        .insert_chunks(&[Chunk {
            id: "embedding-cas-0".into(),
            document_id: doc.id.clone(),
            chunk_index: 0,
            content: "stable text".into(),
            embedding: vec![1.0, 0.0],
            char_start: 0,
            char_end: 11,
            metadata_json: "{}".into(),
        }])
        .unwrap();
    store.ensure_fts("porter").unwrap();
    let persisted = store.get_document(&doc.id).unwrap().unwrap();
    let original = store.list_chunks_for_document(&doc.id).unwrap();

    {
        let conn = store.lock().unwrap();
        crate::db::fts::mark_fts_dirty(&conn).unwrap();
    }
    let dirty_before = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert!(dirty_before.dirty);
    store
        .update_chunk_embeddings_atomic(
            &doc.id,
            persisted.revision,
            &original,
            &[vec![0.0, 1.0]],
            None,
        )
        .unwrap();
    let dirty_after = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert!(dirty_after.dirty);
    assert_eq!(
        dirty_after.chunks_generation,
        dirty_before.chunks_generation + 1
    );
    assert_eq!(dirty_after.index_generation, dirty_before.index_generation);
    assert_eq!(dirty_after.rebuild_count, dirty_before.rebuild_count);

    let manifest_before = EmbeddingManifest {
        id: "default".into(),
        provider: "mock".into(),
        model: "before-conflict".into(),
        dims: 2,
        base_url: None,
        content_fingerprint: None,
        updated_at: Utc::now(),
    };
    store.set_embedding_manifest(&manifest_before).unwrap();
    let rejected_manifest = EmbeddingManifest {
        model: "must-not-commit".into(),
        ..manifest_before.clone()
    };
    let generation_before_conflict = dirty_after.chunks_generation;
    let conflict = store
        .update_chunk_embeddings_atomic(
            &doc.id,
            persisted.revision,
            &original,
            &[vec![0.5, 0.5]],
            Some(&rejected_manifest),
        )
        .unwrap_err();
    assert!(matches!(conflict, AppError::Conflict(_)));
    assert_eq!(
        store.list_chunks_for_document(&doc.id).unwrap()[0].embedding,
        vec![0.0, 1.0]
    );
    assert_eq!(
        store.get_embedding_manifest().unwrap().unwrap().model,
        manifest_before.model
    );
    let generation_after_conflict = {
        let conn = store.lock().unwrap();
        crate::db::fts::chunks_generation(&conn).unwrap()
    };
    assert_eq!(generation_after_conflict, generation_before_conflict);

    let fresh_chunks = store.list_chunks_for_document(&doc.id).unwrap();
    let mut revised = store.get_document(&doc.id).unwrap().unwrap();
    revised.title = "concurrent metadata update".into();
    revised.updated_at = Utc::now();
    store
        .upsert_document_cas(&revised, Some(persisted.revision))
        .unwrap();
    let revision_conflict = store
        .update_chunk_embeddings_atomic(
            &doc.id,
            persisted.revision,
            &fresh_chunks,
            &[vec![0.75, 0.25]],
            Some(&rejected_manifest),
        )
        .unwrap_err();
    assert!(matches!(revision_conflict, AppError::Conflict(_)));
    assert_eq!(
        store.list_chunks_for_document(&doc.id).unwrap()[0].embedding,
        vec![0.0, 1.0]
    );
    assert_eq!(
        store.get_embedding_manifest().unwrap().unwrap().model,
        manifest_before.model
    );
}

#[test]
fn embedding_only_update_rolls_back_partial_vectors_generation_and_manifest() {
    let store = open_temp();
    let doc = sample_doc("embedding-rollback", "wiki://embedding-rollback");
    store.upsert_document(&doc).unwrap();
    store
        .insert_chunks(&[
            Chunk {
                id: "embedding-rollback-0".into(),
                document_id: doc.id.clone(),
                chunk_index: 0,
                content: "first stable text".into(),
                embedding: vec![1.0, 0.0],
                char_start: 0,
                char_end: 17,
                metadata_json: "{}".into(),
            },
            Chunk {
                id: "embedding-rollback-1".into(),
                document_id: doc.id.clone(),
                chunk_index: 1,
                content: "second stable text".into(),
                embedding: vec![0.0, 1.0],
                char_start: 18,
                char_end: 36,
                metadata_json: "{}".into(),
            },
        ])
        .unwrap();
    store.ensure_fts("porter").unwrap();
    let original_manifest = EmbeddingManifest {
        id: "default".into(),
        provider: "mock".into(),
        model: "original-model".into(),
        dims: 2,
        base_url: None,
        content_fingerprint: None,
        updated_at: Utc::now(),
    };
    store.set_embedding_manifest(&original_manifest).unwrap();
    let replacement_manifest = EmbeddingManifest {
        model: "replacement-model".into(),
        ..original_manifest.clone()
    };
    let persisted = store.get_document(&doc.id).unwrap().unwrap();
    let original_chunks = store.list_chunks_for_document(&doc.id).unwrap();
    let generation_before = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };

    let error = store
        .update_chunk_embeddings_atomic_with(
            &doc.id,
            persisted.revision,
            &original_chunks,
            &[vec![0.25, 0.75], vec![0.75, 0.25]],
            Some(&replacement_manifest),
            |updated| {
                if updated == 1 {
                    Err(AppError::db("injected embedding update failure"))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("injected embedding update failure"));
    let after = store.list_chunks_for_document(&doc.id).unwrap();
    assert_eq!(after[0].embedding, original_chunks[0].embedding);
    assert_eq!(after[1].embedding, original_chunks[1].embedding);
    assert_eq!(
        store.get_embedding_manifest().unwrap().unwrap().model,
        original_manifest.model
    );
    let generation_after = {
        let conn = store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert_eq!(generation_after, generation_before);
}

#[test]
fn ops_log_append_read_list_recent() {
    let store = open_temp();

    let empty_op = OpsLogEntry {
        id: String::new(),
        seq: 0,
        ts: Utc::now(),
        op: "  ".into(),
        prefix: None,
        message: "x".into(),
        entity_id: None,
        entity_kind: None,
        payload_json: "{}".into(),
        agent_name: None,
    };
    assert!(store.append_ops_log(&empty_op).is_err());

    let e1 = store
        .append_ops_log(&OpsLogEntry {
            id: String::new(),
            seq: 0,
            ts: Utc::now(),
            op: "ingest".into(),
            prefix: Some("INGEST".into()),
            message: "ingested doc".into(),
            entity_id: Some("d1".into()),
            entity_kind: Some("document".into()),
            payload_json: r#"{"chunks":3}"#.into(),
            agent_name: Some("tester".into()),
        })
        .unwrap();
    assert!(!e1.id.is_empty());
    assert_eq!(e1.seq, 1);
    assert_eq!(e1.op, "ingest");
    assert_eq!(e1.prefix.as_deref(), Some("INGEST"));

    let e2 = store
        .append_ops_log(&OpsLogEntry {
            id: String::new(),
            seq: 0,
            ts: Utc::now(),
            op: "wiki_write".into(),
            prefix: Some("WIKI".into()),
            message: "wrote page".into(),
            entity_id: Some("w1".into()),
            entity_kind: Some("wiki".into()),
            payload_json: String::new(),
            agent_name: None,
        })
        .unwrap();
    assert_eq!(e2.seq, 2);
    assert_eq!(e2.payload_json, "{}");

    let by_id = store.get_ops_log(&e1.id).unwrap().expect("by id");
    assert_eq!(by_id.seq, 1);
    assert_eq!(by_id.message, "ingested doc");
    assert_eq!(by_id.entity_id.as_deref(), Some("d1"));
    assert_eq!(by_id.agent_name.as_deref(), Some("tester"));

    let by_seq = store.get_ops_log_by_seq(2).unwrap().expect("by seq");
    assert_eq!(by_seq.id, e2.id);
    assert_eq!(by_seq.op, "wiki_write");

    let recent = store.list_recent_ops(10).unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].seq, 2);
    assert_eq!(recent[1].seq, 1);

    let limited = store.list_ops_log(1).unwrap();
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].seq, 2);

    let read_one = store.read_ops_log(Some(&e1.id), None, None).unwrap();
    assert_eq!(read_one.len(), 1);
    assert_eq!(read_one[0].id, e1.id);

    let read_seq = store.read_ops_log(None, Some(2), None).unwrap();
    assert_eq!(read_seq.len(), 1);
    assert_eq!(read_seq[0].id, e2.id);

    let read_list = store.read_ops_log(None, None, Some(10)).unwrap();
    assert_eq!(read_list.len(), 2);

    assert!(store.get_ops_log("missing").unwrap().is_none());
    assert!(store.get_ops_log_by_seq(99).unwrap().is_none());

    assert!(store.ops_log_exists().unwrap());

    // Non-filing ops are excluded; ingest / wiki_write match the filter.
    let filed = store.memories_filed_away(10).unwrap();
    assert_eq!(filed.len(), 2);
    assert_eq!(filed[0].op, "wiki_write");
    assert_eq!(filed[1].op, "ingest");

    store
        .append_ops_log(&OpsLogEntry {
            id: String::new(),
            seq: 0,
            ts: Utc::now(),
            op: "analyze_corpus".into(),
            prefix: Some("MAINT".into()),
            message: "not a memory filing".into(),
            entity_id: None,
            entity_kind: None,
            payload_json: "{}".into(),
            agent_name: None,
        })
        .unwrap();
    let filed2 = store.memories_filed_away(10).unwrap();
    assert_eq!(filed2.len(), 2, "non-filing ops must not appear");
    assert!(filed2.iter().all(|e| e.op != "analyze_corpus"));
}

#[test]
fn vacuum_store_checkpoint_and_size_stats() {
    let store = open_temp();
    store
        .upsert_document(&sample_doc("d1", "file://vac.md"))
        .unwrap();

    let report = store.vacuum_store().expect("vacuum_store");
    assert!(report.checkpointed);
    assert_eq!(report.db_path, store.path().display().to_string());
    assert!(report.bytes_before.is_some());
    assert!(report.bytes_after.is_some());
    assert!(report.bytes_delta.is_some());
    assert!(report.bytes_after.unwrap() > 0);

    let ops = store.list_ops_log(5).unwrap();
    assert!(
        ops.iter().any(|e| e.op == "vacuum_store"),
        "expected ops_log vacuum_store entry, got: {:?}",
        ops.iter().map(|e| e.op.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn document_cas_rejects_stale_revision() {
    let store = open_temp();
    let now = Utc::now();
    let mut doc = Document {
        id: "d1".into(),
        uri: "wiki://t".into(),
        title: "T".into(),
        content: "v1".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        layer: "wiki".into(),
        kind: "wiki".into(),
        ..Default::default()
    };
    let r1 = store.upsert_document_cas(&doc, None).unwrap();
    assert_eq!(r1, 1);
    doc.content = "v2".into();
    let r2 = store.upsert_document_cas(&doc, Some(1)).unwrap();
    assert_eq!(r2, 2);
    doc.content = "v3".into();
    let err = store.upsert_document_cas(&doc, Some(1)).unwrap_err();
    assert!(
        matches!(err, AppError::Conflict(_)),
        "expected conflict, got {err:?}"
    );
    let ok = store.upsert_document_cas(&doc, Some(2)).unwrap();
    assert_eq!(ok, 3);
    let loaded = store.get_document("d1").unwrap().unwrap();
    assert_eq!(loaded.revision, 3);
    assert_eq!(loaded.content, "v3");
    let history = store.list_document_revisions("d1").unwrap();
    assert_eq!(
        history.iter().map(|doc| doc.revision).collect::<Vec<_>>(),
        vec![2, 1]
    );
    assert_eq!(history[0].content, "v2");
    assert_eq!(history[1].content, "v1");
    let first_page = store.list_document_revision_summaries("d1", 1, 0).unwrap();
    assert_eq!(first_page.total, 2);
    assert_eq!(first_page.items.len(), 1);
    assert_eq!(first_page.items[0].revision, 2);
    assert_eq!(first_page.items[0].content_chars, 2);
    assert_eq!(first_page.items[0].content_lines, 1);
    let serialized = serde_json::to_value(&first_page.items[0]).unwrap();
    assert!(serialized.get("content").is_none());
    assert!(serialized.get("metadata_json").is_none());
    let second_page = store.list_document_revision_summaries("d1", 1, 1).unwrap();
    assert_eq!(second_page.items[0].revision, 1);
}

#[test]
fn query_wiki_index_term_match_and_limit() {
    let store = open_temp();
    let now = Utc::now();
    store
        .upsert_wiki_index_entry(&WikiIndexEntry {
            id: "idx-1".into(),
            slug: "rust-ownership".into(),
            title: "Rust Ownership".into(),
            kind: "concept".into(),
            category: Some("languages".into()),
            summary: Some("How ownership and borrowing work in Rust".into()),
            page_id: Some("page-1".into()),
            updated_at: now,
        })
        .unwrap();
    store
        .upsert_wiki_index_entry(&WikiIndexEntry {
            id: "idx-2".into(),
            slug: "python-gil".into(),
            title: "Python GIL".into(),
            kind: "concept".into(),
            category: Some("languages".into()),
            summary: Some("Global Interpreter Lock notes".into()),
            page_id: Some("page-2".into()),
            updated_at: now,
        })
        .unwrap();

    let hits = store.query_wiki_index("rust ownership", 10).unwrap();
    assert!(!hits.is_empty());
    assert_eq!(hits[0].entry.slug, "rust-ownership");
    assert!(hits[0].score > 0.0);
    assert!(!hits.iter().any(|h| h.entry.slug == "python-gil"));

    // Slug/title hits outrank weak summary overlap (discrimination).
    store
        .upsert_wiki_index_entry(&WikiIndexEntry {
            id: "idx-3".into(),
            slug: "architecture-spine".into(),
            title: "Architecture and Spine Tools".into(),
            kind: "concept".into(),
            category: Some("system".into()),
            summary: Some("Layers L0-L4 and index-first cascade".into()),
            page_id: Some("page-3".into()),
            updated_at: now,
        })
        .unwrap();
    store
        .upsert_wiki_index_entry(&WikiIndexEntry {
            id: "idx-4".into(),
            slug: "connect-clients".into(),
            title: "Connect Clients".into(),
            kind: "howto".into(),
            category: Some("ops".into()),
            summary: Some("Mentions architecture only in passing".into()),
            page_id: Some("page-4".into()),
            updated_at: now,
        })
        .unwrap();
    let ranked = store.query_wiki_index("architecture spine", 10).unwrap();
    assert!(ranked.len() >= 2);
    assert_eq!(ranked[0].entry.slug, "architecture-spine");
    assert!(
        ranked[0].score > ranked[1].score,
        "expected discrimination: {} vs {}",
        ranked[0].score,
        ranked[1].score
    );

    let limited = store.query_wiki_index("", 1).unwrap();
    assert_eq!(limited.len(), 1);

    let none = store.query_wiki_index("zzzzqqqxxxyyy", 5).unwrap();
    assert!(none.is_empty());
}

#[test]
fn wiki_index_rebuild_read_update_entry() {
    let store = open_temp();
    let now = Utc::now();

    let w1 = Document {
        id: "w1".into(),
        uri: "wiki://alpha".into(),
        title: "Alpha".into(),
        content: "# Alpha\n\nFirst line about alpha systems.\n\nMore body.".into(),
        metadata_json: r#"{"category":"entities","summary":"Alpha entity summary"}"#.into(),
        created_at: now,
        updated_at: now,
        layer: "wiki".into(),
        kind: "entity".into(),
        ..Default::default()
    };
    let w2 = Document {
        id: "w2".into(),
        uri: "wiki://beta-page".into(),
        title: "Beta Page".into(),
        content: "Beta intro without heading.\nSecond.".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        layer: "wiki".into(),
        kind: "wiki".into(),
        ..Default::default()
    };
    let raw = Document {
        id: "r1".into(),
        uri: "raw://src".into(),
        title: "Raw".into(),
        content: "raw only".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        layer: "raw".into(),
        kind: "document".into(),
        ..Default::default()
    };
    store.upsert_document(&w1).unwrap();
    store.upsert_document(&w2).unwrap();
    store.upsert_document(&raw).unwrap();

    let n = store.rebuild_wiki_index_from_docs().unwrap();
    assert_eq!(n, 2);
    assert_eq!(store.list_wiki_index().unwrap().len(), 2);

    let alpha = store
        .get_wiki_index_by_slug("alpha")
        .unwrap()
        .expect("alpha");
    assert_eq!(alpha.title, "Alpha");
    assert_eq!(alpha.kind, "entity");
    assert_eq!(alpha.category.as_deref(), Some("entities"));
    assert_eq!(alpha.summary.as_deref(), Some("Alpha entity summary"));
    assert_eq!(alpha.page_id.as_deref(), Some("w1"));

    let beta = store
        .get_wiki_index_by_slug("beta-page")
        .unwrap()
        .expect("beta");
    assert_eq!(beta.title, "Beta Page");
    assert_eq!(beta.summary.as_deref(), Some("Beta intro without heading."));

    let updated = store
        .update_wiki_index_entry_fields(
            "beta-page",
            None,
            Some("Updated one-line summary".into()),
            None,
            Some("concepts".into()),
            None,
        )
        .unwrap();
    assert_eq!(updated.title, "Beta Page");
    assert_eq!(updated.summary.as_deref(), Some("Updated one-line summary"));
    assert_eq!(updated.category.as_deref(), Some("concepts"));

    let md = store.render_wiki_index_markdown().unwrap();
    assert!(md.contains("# Wiki index"));
    assert!(md.contains("[[Alpha]]"));
    assert!(md.contains("Updated one-line summary"));

    let manual = store
        .update_wiki_index_entry_fields(
            "manual-note",
            Some("Manual Note".into()),
            Some("Hand-written catalog row".into()),
            Some("concept".into()),
            Some("notes".into()),
            None,
        )
        .unwrap();
    assert_eq!(manual.slug, "manual-note");
    assert_eq!(store.list_wiki_index().unwrap().len(), 3);

    let n2 = store.rebuild_wiki_index_from_docs().unwrap();
    assert_eq!(n2, 2);
    assert!(store
        .get_wiki_index_by_slug("manual-note")
        .unwrap()
        .is_none());
    assert!(store.get_wiki_index_by_slug("alpha").unwrap().is_some());
}

#[test]
fn diary_entries_and_pinned_list_for_wake_up() {
    let store = open_temp();
    let now = Utc::now();
    let earlier = now - chrono::Duration::hours(2);

    let d1 = Document {
        id: "diary-1".into(),
        uri: "diary://agents/alice/1".into(),
        title: "Alice note 1".into(),
        content: "first diary body".into(),
        metadata_json: r#"{"agent_name":"alice"}"#.into(),
        created_at: earlier,
        updated_at: earlier,
        wing: Some("agents/alice".into()),
        room: Some("alice".into()),
        layer: "diary".into(),
        kind: "diary".into(),
        ..Default::default()
    };
    let d2 = Document {
        id: "diary-2".into(),
        uri: "diary://agents/alice/2".into(),
        title: "Alice note 2".into(),
        content: "second diary body".into(),
        metadata_json: r#"{"agent_name":"alice"}"#.into(),
        created_at: now,
        updated_at: now,
        wing: Some("agents/alice".into()),
        room: Some("alice".into()),
        layer: "diary".into(),
        kind: "diary".into(),
        ..Default::default()
    };
    let d_bob = Document {
        id: "diary-bob".into(),
        uri: "diary://agents/bob/1".into(),
        title: "Bob note".into(),
        content: "bob body".into(),
        metadata_json: r#"{"agent_name":"bob"}"#.into(),
        created_at: now,
        updated_at: now,
        wing: Some("agents/bob".into()),
        room: Some("bob".into()),
        layer: "diary".into(),
        kind: "diary".into(),
        ..Default::default()
    };
    let pinned = Document {
        id: "pin-1".into(),
        uri: "wiki://evergreen".into(),
        title: "Evergreen".into(),
        content: "pinned page".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        layer: "wiki".into(),
        kind: "wiki".into(),
        pinned: true,
        boost: 2.0,
        ..Default::default()
    };
    let unpinned = Document {
        id: "plain-1".into(),
        uri: "raw://plain".into(),
        title: "Plain".into(),
        content: "not pinned".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        layer: "raw".into(),
        kind: "document".into(),
        pinned: false,
        ..Default::default()
    };
    for d in [&d1, &d2, &d_bob, &pinned, &unpinned] {
        store.upsert_document(d).unwrap();
    }

    let alice = store
        .list_diary_entries(Some("alice"), 10)
        .expect("alice diary");
    assert_eq!(alice.len(), 2);
    assert_eq!(alice[0].id, "diary-2"); // newest first
    assert_eq!(alice[0].agent_name, "alice");
    assert_eq!(alice[0].content, "second diary body");
    assert_eq!(alice[0].wing, "agents/alice");
    assert_eq!(alice[1].id, "diary-1");

    let limited = store.list_diary_entries(Some("alice"), 1).expect("limit 1");
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].id, "diary-2");

    let all = store.list_diary_entries(None, 10).expect("all diary");
    assert_eq!(all.len(), 3);

    let pins = store.list_pinned_documents(10).expect("pinned");
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].id, "pin-1");
    assert!(pins[0].pinned);
}

#[test]
fn list_wiki_page_metas_filters_order_metadata_and_revision() {
    let store = open_temp();
    let now = Utc::now();

    let alpha = Document {
        id: "w-alpha".into(),
        uri: "wiki://alpha".into(),
        title: "Alpha".into(),
        content: "# Alpha\n\nFirst line of body content.\n".into(),
        metadata_json: r#"{"summary":"Alpha summary","category":"entities"}"#.into(),
        created_at: now,
        updated_at: now,
        layer: "wiki".into(),
        kind: "entity".into(),
        ..Default::default()
    };
    let zeta = Document {
        id: "w-zeta".into(),
        uri: "wiki://zeta-page".into(),
        title: "Zeta".into(),
        content: "Body only; no summary in metadata.".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        layer: "wiki".into(),
        kind: "wiki".into(),
        ..Default::default()
    };
    // Kind will be cleared in-DB after upsert so metas empty/null kind -> "wiki".
    let beta = Document {
        id: "w-beta".into(),
        uri: "wiki://beta".into(),
        title: "Beta".into(),
        content: "beta body".into(),
        metadata_json: r#"{"category":"  concepts  "}"#.into(),
        created_at: now,
        updated_at: now,
        layer: "wiki".into(),
        kind: "concept".into(),
        ..Default::default()
    };
    let raw = Document {
        id: "r1".into(),
        uri: "file://raw.md".into(),
        title: "Raw Should Not Appear".into(),
        content: "raw".into(),
        metadata_json: "{}".into(),
        created_at: now,
        updated_at: now,
        layer: "raw".into(),
        kind: "document".into(),
        ..Default::default()
    };
    for d in [&alpha, &zeta, &beta, &raw] {
        store.upsert_document(d).unwrap();
    }
    // upsert_document coerces empty kind to "document"; force empty for fallback path.
    {
        let conn = store.lock().unwrap();
        conn.execute(
            "UPDATE documents SET kind = '' WHERE id = ?",
            params!["w-beta"],
        )
        .unwrap();
    }

    // Bump revision on alpha so metas surface revision/etag > 1.
    let mut alpha_v2 = alpha.clone();
    alpha_v2.content = "# Alpha\n\nRevised body.\n".into();
    let rev = store.upsert_document_cas(&alpha_v2, Some(1)).unwrap();
    assert_eq!(rev, 2);

    let empty = open_temp().list_wiki_page_metas().unwrap();
    assert!(empty.is_empty());

    let metas = store.list_wiki_page_metas().unwrap();
    assert_eq!(metas.len(), 3, "raw layer excluded");
    // Ordered by title ASC: Alpha, Beta, Zeta
    assert_eq!(
        metas.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(),
        vec!["Alpha", "Beta", "Zeta"]
    );

    let a = &metas[0];
    assert_eq!(a.id, "w-alpha");
    assert_eq!(a.uri, "wiki://alpha");
    assert_eq!(a.slug, "alpha");
    assert_eq!(a.kind, "entity");
    assert_eq!(a.summary.as_deref(), Some("Alpha summary"));
    assert_eq!(a.category.as_deref(), Some("entities"));
    assert_eq!(a.revision, 2);
    assert_eq!(a.etag, crate::models::format_document_etag(2));
    assert!(!a.updated_at.is_empty());

    let b = &metas[1];
    assert_eq!(b.id, "w-beta");
    assert_eq!(b.slug, "beta");
    assert_eq!(b.kind, "wiki", "empty kind defaults to wiki");
    assert!(b.summary.is_none());
    assert_eq!(b.category.as_deref(), Some("concepts"));
    assert_eq!(b.revision, 1);
    assert_eq!(b.etag, crate::models::format_document_etag(1));

    let z = &metas[2];
    assert_eq!(z.id, "w-zeta");
    assert_eq!(z.slug, "zeta-page");
    // Unlike rebuild_wiki_index, metas do not fall back to first content line.
    assert!(
        z.summary.is_none(),
        "summary must come from metadata only, not content"
    );
    assert!(z.category.is_none());

    // q / kind / category / limit / offset filters
    let (by_q, total_q) = store
        .list_wiki_page_metas_filtered(&WikiPageMetaFilter {
            q: Some("alp".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(total_q, 1);
    assert_eq!(by_q.len(), 1);
    assert_eq!(by_q[0].id, "w-alpha");

    let (by_kind, total_kind) = store
        .list_wiki_page_metas_filtered(&WikiPageMetaFilter {
            kind: Some("wiki".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(total_kind, 2, "empty kind + explicit wiki");
    assert_eq!(
        by_kind.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["w-beta", "w-zeta"]
    );

    let (by_cat, total_cat) = store
        .list_wiki_page_metas_filtered(&WikiPageMetaFilter {
            category: Some("ENTITIES".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(total_cat, 1);
    assert_eq!(by_cat[0].id, "w-alpha");

    let (page, total_all) = store
        .list_wiki_page_metas_filtered(&WikiPageMetaFilter {
            limit: Some(1),
            offset: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(total_all, 3);
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].title, "Beta");

    let (none, total_none) = store
        .list_wiki_page_metas_filtered(&WikiPageMetaFilter {
            q: Some("zzzz-no-match".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(total_none, 0);
    assert!(none.is_empty());
}

#[test]
fn list_wiki_page_metas_never_leaks_body_content() {
    let store = open_temp();
    let now = Utc::now();
    let secret = "SECRET_BODY_MARKER_xyz_do_not_list";
    store
        .upsert_document(&Document {
            id: "w-body".into(),
            uri: "wiki://body-page".into(),
            title: "Body Page".into(),
            content: format!("# Body\n\n{secret}\n"),
            metadata_json: r#"{"summary":"catalog blurb only"}"#.into(),
            created_at: now,
            updated_at: now,
            layer: "wiki".into(),
            kind: "wiki".into(),
            ..Default::default()
        })
        .unwrap();

    let items = store.list_wiki_page_metas().unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item.summary.as_deref(), Some("catalog blurb only"));
    let wire = serde_json::to_string(item).unwrap();
    assert!(
        !wire.contains(secret),
        "list DTO must not include document body: {wire}"
    );
    assert!(
        !wire.contains("\"content\""),
        "list DTO JSON must not have content key: {wire}"
    );
}

#[test]
fn list_wiki_page_metas_filtered_wing_room() {
    let store = open_temp();
    let now = Utc::now();
    for (id, title, wing, room) in [
        ("w1", "One", Some("projects"), Some("rag")),
        ("w2", "Two", Some("projects"), Some("ui")),
        ("w3", "Three", Some("personal"), None),
    ] {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("wiki://{id}"),
                title: title.into(),
                content: title.into(),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                layer: "wiki".into(),
                kind: "wiki".into(),
                wing: wing.map(str::to_string),
                room: room.map(str::to_string),
                ..Default::default()
            })
            .unwrap();
    }

    let (rows, total) = store
        .list_wiki_page_metas_filtered(&WikiPageMetaFilter {
            wing: Some("projects".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(total, 2);
    assert_eq!(
        rows.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec!["w1", "w2"]
    );

    let (room_rows, room_total) = store
        .list_wiki_page_metas_filtered(&WikiPageMetaFilter {
            wing: Some("projects".into()),
            room: Some("rag".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(room_total, 1);
    assert_eq!(room_rows[0].id, "w1");
}

#[test]
fn wiki_backlinks_for_document_wikilink_only_sorted_and_deduped() {
    let store = open_temp();
    let now = Utc::now();

    // Missing document / no graph node -> empty, not an error.
    assert!(store
        .wiki_backlinks_for_document("missing-doc")
        .unwrap()
        .is_empty());

    for (id, uri, title) in [
        ("doc-target", "wiki://target", "Target"),
        ("doc-src-a", "wiki://src-a", "Source A"),
        ("doc-src-b", "wiki://src-b", "Source B"),
    ] {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: uri.into(),
                title: title.into(),
                content: title.into(),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                layer: "wiki".into(),
                kind: "wiki".into(),
                ..Default::default()
            })
            .unwrap();
    }

    let gnode = |id: &str, label: &str, doc_id: Option<&str>| crate::models::GraphNode {
        id: id.into(),
        kind: if doc_id.is_some() {
            "document".into()
        } else {
            "stub".into()
        },
        label: label.into(),
        document_id: doc_id.map(str::to_string),
        uri: None,
        resolved: doc_id.is_some(),
        metadata_json: "{}".into(),
    };

    store
        .upsert_graph_node(&gnode("n-target", "Target", Some("doc-target")))
        .unwrap();
    store
        .upsert_graph_node(&gnode("n-src-a", "Source A", Some("doc-src-a")))
        .unwrap();
    store
        .upsert_graph_node(&gnode("n-src-b", "Source B", Some("doc-src-b")))
        .unwrap();
    // Stub without document_id: key falls back to node id.
    store
        .upsert_graph_node(&gnode("n-stub", "Stub Link", None))
        .unwrap();

    store
        .link_nodes("n-src-b", "n-target", "wikilink", 1.0)
        .unwrap();
    store
        .link_nodes("n-src-a", "n-target", "wikilink", 1.0)
        .unwrap();
    store
        .link_nodes("n-stub", "n-target", "wikilink", 1.0)
        .unwrap();
    // Non-wikilink inbound edges must be ignored.
    store
        .link_nodes("n-src-a", "n-target", "related", 1.0)
        .unwrap();
    store
        .link_nodes("n-src-a", "n-target", "tagged", 1.0)
        .unwrap();
    // Duplicate wikilink from same source (same label + key) should dedupe.
    store
        .link_nodes("n-src-a", "n-target", "wikilink", 0.5)
        .unwrap();

    let bl = store.wiki_backlinks_for_document("doc-target").unwrap();
    assert_eq!(
        bl.len(),
        3,
        "expected 3 unique wikilink sources, got {bl:?}"
    );
    // Sorted by label ASC.
    assert_eq!(
        bl.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(),
        vec!["Source A", "Source B", "Stub Link"]
    );
    assert_eq!(bl[0].1, "doc-src-a");
    assert_eq!(bl[1].1, "doc-src-b");
    assert_eq!(bl[2].1, "n-stub", "stub without document_id uses node id");

    // Document with a node but no inbound wikilinks.
    assert!(store
        .wiki_backlinks_for_document("doc-src-a")
        .unwrap()
        .is_empty());
}

#[test]
fn wiki_backlinks_project_scope_excludes_foreign_documents_and_stubs() {
    let store = open_temp();
    for (id, wing) in [
        ("target", "alpha"),
        ("source-alpha", "alpha"),
        ("source-beta", "beta"),
    ] {
        let mut document = sample_doc(id, &format!("wiki://{id}"));
        document.wing = Some(wing.into());
        store.upsert_document(&document).unwrap();
    }
    let graph_node = |id: &str, document_id: Option<&str>| crate::models::GraphNode {
        id: format!("node-{id}"),
        kind: if document_id.is_some() {
            "document".into()
        } else {
            "stub".into()
        },
        label: id.into(),
        document_id: document_id.map(str::to_string),
        uri: None,
        resolved: document_id.is_some(),
        metadata_json: "{}".into(),
    };
    for node in [
        graph_node("target", Some("target")),
        graph_node("source-alpha", Some("source-alpha")),
        graph_node("source-beta", Some("source-beta")),
        graph_node("unowned", None),
    ] {
        store.upsert_graph_node(&node).unwrap();
    }
    for source in ["source-alpha", "source-beta", "unowned"] {
        store
            .link_nodes(&format!("node-{source}"), "node-target", "wikilink", 1.0)
            .unwrap();
    }

    let scoped = store
        .wiki_backlinks_for_document_in_wing("target", "alpha")
        .unwrap();
    assert_eq!(scoped, vec![("source-alpha".into(), "source-alpha".into())]);
    assert!(store
        .wiki_backlinks_for_document_in_wing("target", "beta")
        .unwrap()
        .is_empty());
    assert_eq!(
        store.wiki_backlinks_for_document("target").unwrap().len(),
        3
    );
}
