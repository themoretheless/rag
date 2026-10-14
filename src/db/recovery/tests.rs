use super::*;
use crate::models::{GraphEdge, GraphFilter, GraphNode};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[test]
fn bounded_recovery_reader_rejects_sparse_oversized_file() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("oversized.json");
    let file = fs::File::create(&path).unwrap();
    file.set_len(PORTABLE_RECOVERY_MAX_BYTES + 1).unwrap();

    let error = read_recovery_bundle_file(&path).unwrap_err();

    assert!(error.to_string().contains("serialized to"));
    assert!(error.to_string().contains("verified DuckDB backup"));
}

#[test]
fn store_preflight_refuses_document_count_before_materialization() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("large.duckdb")).unwrap();
    {
        let conn = store.lock().unwrap();
        conn.execute(
            r#"
                INSERT INTO documents (
                    id, uri, title, content, metadata_json, created_at, updated_at
                )
                SELECT
                    'portable-limit-' || CAST(i AS VARCHAR),
                    'recovery://portable-limit/' || CAST(i AS VARCHAR),
                    'limit', '', '{}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
                FROM range(0, ?) AS generated(i)
                "#,
            params![i64::try_from(PORTABLE_RECOVERY_MAX_DOCUMENTS + 1).unwrap()],
        )
        .unwrap();
    }

    let error = store.recovery_bundle().unwrap_err();

    assert!(error.to_string().contains("contains 10001 documents"));
    assert!(error.to_string().contains("verified DuckDB backup"));
}

#[test]
fn bounded_encoder_preserves_small_json_and_jsonl_semantics() {
    let bundle = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: None,
        documents: vec![BundleDocument {
            document: recovery_document("small", "recovery://small", "small body"),
            chunks: Vec::new(),
        }],
    };

    for format in ["json", "jsonl"] {
        let encoded = encode_recovery_bundle(&bundle, format).unwrap();
        let decoded =
            decode_recovery_bundle(std::str::from_utf8(&encoded).unwrap(), format).unwrap();
        assert_eq!(decoded.format, bundle.format);
        assert_eq!(decoded.version, bundle.version);
        assert_eq!(decoded.documents.len(), 1);
        assert_eq!(decoded.documents[0].document.id, "small");
    }
}

#[test]
fn backup_writes_verified_sidecars_and_inventory_protects_final() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("live.duckdb");
    let backups = root.path().join("backups");
    fs::create_dir(&backups).unwrap();
    let backup = backups.join("rag-final.duckdb");
    let store = Store::open(&source).unwrap();

    let report = store.backup_database(&backup, false, false).unwrap();
    let verification = report.verification.expect("verification");
    assert!(verification.ok);
    assert_eq!(
        verification.schema_version,
        crate::db::schema::SCHEMA_VERSION
    );
    assert!(PathBuf::from(format!("{}.sha256", backup.display())).is_file());
    let metadata_path = PathBuf::from(format!("{}.metadata.json", backup.display()));
    let sidecar: BackupSidecar = serde_json::from_slice(&fs::read(metadata_path).unwrap()).unwrap();
    assert_eq!(sidecar.required_free_bytes, verification.bytes * 2);
    assert_eq!(sidecar.verification.sha256, verification.sha256);

    let inventory = backup_inventory(&backups).unwrap();
    assert_eq!(inventory.len(), 1);
    let final_backup = inventory
        .iter()
        .find(|item| item.path.ends_with("rag-final.duckdb"))
        .unwrap();
    assert!(final_backup.protected);
    assert!(retention_preview(&backups, 1).unwrap().is_empty());
}

#[test]
fn verify_backup_never_migrates_or_mutates_the_inspected_file() {
    let root = tempfile::tempdir().unwrap();
    let backup = root.path().join("legacy-empty.duckdb");
    drop(duckdb::Connection::open(&backup).unwrap());
    let before = fs::read(&backup).unwrap();
    let before_hash = sha256_file(&backup).unwrap();

    let verification = verify_backup(&backup).unwrap();

    assert!(verification.ok);
    assert_eq!(
        verification.schema_version,
        crate::db::schema::SCHEMA_VERSION
    );
    assert_eq!(verification.sha256, before_hash);
    assert_eq!(fs::read(&backup).unwrap(), before);
    assert_eq!(sha256_file(&backup).unwrap(), before_hash);
    assert_no_backup_work_files(root.path());
}

#[test]
fn verify_backup_fails_closed_for_invalid_embedding_contracts() {
    for case in ["missing", "forged", "incomplete", "dims"] {
        let root = tempfile::tempdir().unwrap();
        let backup = root.path().join(format!("{case}.duckdb"));
        let store = Store::open(&backup).unwrap();
        let document = recovery_document("document", "recovery://document", "body");
        store.upsert_document(&document).unwrap();
        let mut chunk = recovery_chunk("chunk", "document", "body");
        if case == "dims" {
            chunk.embedding = vec![1.0];
        }
        store.insert_chunks(&[chunk]).unwrap();
        if case != "missing" {
            let mut manifest = recovery_manifest();
            match case {
                "forged" => {
                    manifest.content_fingerprint = Some("forged".into());
                }
                "incomplete" => {
                    manifest.content_fingerprint = Some("migration-incomplete:test-fixture".into());
                }
                "dims" => {}
                _ => unreachable!(),
            }
            store.set_embedding_manifest(&manifest).unwrap();
        }
        drop(store);
        let before = fs::read(&backup).unwrap();
        let before_hash = sha256_file(&backup).unwrap();

        let verification = verify_backup(&backup).unwrap();

        assert!(!verification.ok, "case {case} must fail closed");
        assert!(!verification.embedding_contract_ok, "case {case}");
        assert_eq!(verification.sha256, before_hash);
        assert_eq!(fs::read(&backup).unwrap(), before);
        assert_no_backup_work_files(root.path());
    }
}

#[test]
fn backup_reports_destination_errors() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("live.duckdb")).unwrap();
    let missing_parent = root.path().join("missing/backup.duckdb");
    assert!(store
        .backup_database(&missing_parent, false, false)
        .is_err());
}

#[test]
fn concurrent_overwrite_publications_are_serialized_as_complete_generations() {
    let root = tempfile::tempdir().unwrap();
    let source_a = root.path().join("source-a.duckdb");
    let source_b = root.path().join("source-b.duckdb");
    let backup = root.path().join("shared-backup.duckdb");
    let store_a = Store::open(&source_a).unwrap();
    let store_b = Store::open(&source_b).unwrap();
    store_a
        .upsert_document(&recovery_document("a", "recovery://a", "a"))
        .unwrap();
    for id in ["b1", "b2"] {
        store_b
            .upsert_document(&recovery_document(id, &format!("recovery://{id}"), id))
            .unwrap();
    }

    let (first_staged_tx, first_staged_rx) = mpsc::channel();
    let (release_first_tx, release_first_rx) = mpsc::channel();
    let first_backup = backup.clone();
    let first = thread::spawn(move || {
        store_a.backup_database_with_publish_hook(&first_backup, false, true, |stage| {
            if stage == BackupPublishStage::SidecarPublished {
                first_staged_tx.send(()).unwrap();
                release_first_rx.recv().unwrap();
            }
            Ok(())
        })
    });
    first_staged_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("first publisher reached its sidecar stage");

    let (second_staged_tx, second_staged_rx) = mpsc::channel();
    let second_backup = backup.clone();
    let second_source = source_b.clone();
    let second = thread::spawn(move || {
        store_b.backup_database_with_publish_hook(&second_backup, false, true, |stage| {
            if stage == BackupPublishStage::SidecarPublished {
                second_staged_tx.send(()).unwrap();
            }
            Ok(())
        })
    });
    assert!(
        second_staged_rx
            .recv_timeout(Duration::from_millis(250))
            .is_err(),
        "second publisher must wait for the destination generation lock"
    );
    release_first_tx.send(()).unwrap();
    first.join().unwrap().unwrap();
    second_staged_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("second publisher proceeds after the first commits");
    let second_report = second.join().unwrap().unwrap();

    let final_verification = verify_published_backup_group(&backup, &second_source).unwrap();
    assert_eq!(final_verification.documents, 2);
    assert_eq!(
        second_report.verification.unwrap().sha256,
        final_verification.sha256
    );
    assert_no_backup_work_files(root.path());
}

#[test]
fn online_snapshot_copy_does_not_hold_store_mutex_and_preserves_semantics() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("live.duckdb");
    let backup = root.path().join("online.duckdb");
    let store = Store::open(&source).unwrap();
    store
        .upsert_document(&recovery_document(
            "online-document",
            "recovery://online-document",
            "online body",
        ))
        .unwrap();
    store
        .insert_chunks(&[recovery_chunk(
            "online-chunk",
            "online-document",
            "online body",
        )])
        .unwrap();
    store.set_embedding_manifest(&recovery_manifest()).unwrap();
    let source_counts = store.stats().unwrap();
    let source_manifest = store.get_embedding_manifest().unwrap();

    let (copy_ready_tx, copy_ready_rx) = mpsc::channel();
    let (release_copy_tx, release_copy_rx) = mpsc::channel();
    let backup_store = store.clone();
    let backup_path = backup.clone();
    let backup_thread = thread::spawn(move || {
        backup_store.backup_database_with_publish_hook(&backup_path, false, false, |stage| {
            if stage == BackupPublishStage::SnapshotConnectionReady {
                copy_ready_tx.send(()).unwrap();
                release_copy_rx.recv().unwrap();
            }
            Ok(())
        })
    });
    copy_ready_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("snapshot connection reached copy stage");

    let (stats_tx, stats_rx) = mpsc::channel();
    let query_store = store.clone();
    let stats_thread = thread::spawn(move || stats_tx.send(query_store.stats()).unwrap());
    let concurrent_counts = stats_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("stats must not wait for snapshot copy hook")
        .unwrap();
    assert_eq!(concurrent_counts, source_counts);
    stats_thread.join().unwrap();

    store
        .upsert_document(&recovery_document(
            "after-snapshot",
            "recovery://after-snapshot",
            "committed while backup snapshot is pinned",
        ))
        .expect("source write remains available during snapshot copy");
    let source_counts_after = store.stats().unwrap();
    assert_eq!(source_counts_after.0, source_counts.0 + 1);

    release_copy_tx.send(()).unwrap();
    let report = backup_thread.join().unwrap().unwrap();
    let verification = report.verification.unwrap();
    assert_eq!(
        (
            verification.documents,
            verification.chunks,
            verification.nodes,
            verification.edges,
        ),
        source_counts
    );
    assert_eq!(
        serde_json::to_value(verification.embedding_manifest).unwrap(),
        serde_json::to_value(source_manifest).unwrap()
    );
    assert_complete_backup_group(&backup);
    assert_no_backup_work_files(root.path());
}

#[test]
fn backup_preflights_every_sidecar_before_publishing_the_database() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("live.duckdb");
    let backup = root.path().join("backup.duckdb");
    let sha_path = PathBuf::from(format!("{}.sha256", backup.display()));
    let store = Store::open(&source).unwrap();
    fs::write(&sha_path, "keep-existing-sidecar").unwrap();

    let error = store.backup_database(&backup, false, false).unwrap_err();

    assert!(matches!(error, AppError::Conflict(_)));
    assert!(!backup.exists());
    assert_eq!(
        fs::read_to_string(sha_path).unwrap(),
        "keep-existing-sidecar"
    );
}

#[test]
fn atomic_no_overwrite_publish_rejects_a_racing_destination() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("backup.duckdb.sha256");
    let (temporary_path, mut temporary) = create_temporary_artifact(&destination).unwrap();
    temporary.write_all(b"new").unwrap();
    temporary.sync_all().unwrap();
    drop(temporary);
    fs::write(&destination, "racing-writer").unwrap();

    let error = publish_temporary_artifact(&temporary_path, &destination, false).unwrap_err();

    assert!(matches!(error, AppError::Conflict(_)));
    assert_eq!(fs::read_to_string(destination).unwrap(), "racing-writer");
    fs::remove_file(temporary_path).unwrap();
}

#[test]
fn recovery_artifact_publish_is_atomic_and_no_clobber_under_race() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("bundle.json");

    let error =
        publish_recovery_artifact_with_hook(&destination, b"new bundle", false, |destination| {
            fs::write(destination, "racing writer")?;
            Ok(())
        })
        .unwrap_err();

    assert!(matches!(error, AppError::Conflict(_)));
    assert_eq!(fs::read_to_string(&destination).unwrap(), "racing writer");
    assert_no_backup_work_files(root.path());

    assert!(publish_recovery_artifact(&destination, b"replacement", true).unwrap());
    assert_eq!(fs::read(&destination).unwrap(), b"replacement");
    assert_no_backup_work_files(root.path());
}

#[test]
fn recovery_artifact_publish_cleans_stage_after_injected_failure() {
    let root = tempfile::tempdir().unwrap();
    let destination = root.path().join("bundle.jsonl");

    let error =
        publish_recovery_artifact_with_hook(&destination, b"complete staged bytes", false, |_| {
            Err(AppError::db("injected failure before publish"))
        })
        .unwrap_err();

    assert!(error.to_string().contains("injected failure"));
    assert!(!destination.exists());
    assert_no_backup_work_files(root.path());
}

#[test]
fn recovery_overwrite_cross_collision_conflicts_and_rolls_back_every_item() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("live.duckdb")).unwrap();
    store.set_embedding_manifest(&recovery_manifest()).unwrap();
    let original_a = recovery_document("document-a", "recovery://a", "original a");
    let original_b = recovery_document("document-b", "recovery://b", "original b");
    store.upsert_document(&original_a).unwrap();
    store.upsert_document(&original_b).unwrap();
    store
        .insert_chunks(&[
            recovery_chunk("chunk-a", "document-a", "original chunk a"),
            recovery_chunk("chunk-b", "document-b", "original chunk b"),
        ])
        .unwrap();
    for node in [
        recovery_node("node-a", "document-a", "recovery://a"),
        recovery_node("node-b", "document-b", "recovery://b"),
    ] {
        store.upsert_graph_node(&node).unwrap();
    }
    store
        .insert_graph_edges(&[GraphEdge {
            id: "edge-a-b".into(),
            source_id: "node-a".into(),
            target_id: "node-b".into(),
            rel_type: "related".into(),
            weight: 1.0,
            context: Some("preserve me".into()),
        }])
        .unwrap();

    let before_documents = serde_json::to_vec(&store.recovery_bundle().unwrap().documents)
        .expect("serialize documents before import");
    let before_graph = serde_json::to_vec(
        &store
            .get_graph_view(GraphFilter {
                max_nodes: Some(100),
                ..GraphFilter::default()
            })
            .unwrap(),
    )
    .expect("serialize graph before import");
    let bundle = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: Some(recovery_manifest()),
        documents: vec![
            BundleDocument {
                document: recovery_document("new-document", "recovery://new", "must roll back"),
                chunks: vec![recovery_chunk(
                    "new-chunk",
                    "new-document",
                    "must roll back",
                )],
            },
            BundleDocument {
                document: recovery_document("document-a", "recovery://b", "cross collision"),
                chunks: vec![recovery_chunk(
                    "collision-chunk",
                    "document-a",
                    "cross collision",
                )],
            },
        ],
    };

    let error = store
        .import_recovery_bundle(
            &bundle,
            ConflictPolicy::Overwrite,
            false,
            Path::new("fixture.json"),
            "json",
        )
        .unwrap_err();

    assert!(matches!(error, AppError::Conflict(_)));
    assert!(error.to_string().contains("document-a"));
    assert!(error.to_string().contains("document-b"));
    assert!(store.get_document("new-document").unwrap().is_none());
    let after_documents = serde_json::to_vec(&store.recovery_bundle().unwrap().documents)
        .expect("serialize documents after import");
    let after_graph = serde_json::to_vec(
        &store
            .get_graph_view(GraphFilter {
                max_nodes: Some(100),
                ..GraphFilter::default()
            })
            .unwrap(),
    )
    .expect("serialize graph after import");
    assert_eq!(after_documents, before_documents);
    assert_eq!(after_graph, before_graph);
}

#[test]
fn recovery_import_preserves_zero_and_single_match_overwrite_semantics() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("live.duckdb")).unwrap();
    store.set_embedding_manifest(&recovery_manifest()).unwrap();
    let source_path = "/vault/replace.md";
    let mut old_document = recovery_document("old-document", "recovery://replace", "old body");
    old_document.source_file = Some(source_path.into());
    store.upsert_document(&old_document).unwrap();
    old_document.content = "old body latest".into();
    old_document.content_hash = None;
    store.upsert_document(&old_document).unwrap();
    store
        .upsert_source_manifest(crate::db::SourceManifestWrite {
            canonical_path: source_path,
            canonical_root: "/vault",
            size_bytes: 15,
            mtime_ns: 1,
            content_hash: "old-source-hash",
            document_id: "old-document",
        })
        .unwrap();
    assert_eq!(
        store.list_document_revisions("old-document").unwrap().len(),
        1
    );
    assert!(store
        .load_source_manifest_root(Path::new("/vault"))
        .unwrap()
        .contains_key(source_path));
    store
        .insert_chunks(&[recovery_chunk("old-chunk", "old-document", "old chunk")])
        .unwrap();
    store
        .upsert_graph_node(&recovery_node(
            "old-node",
            "old-document",
            "recovery://replace",
        ))
        .unwrap();
    let bundle = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: Some(recovery_manifest()),
        documents: vec![
            BundleDocument {
                document: recovery_document(
                    "replacement-document",
                    "recovery://replace",
                    "replacement body",
                ),
                chunks: vec![recovery_chunk(
                    "replacement-chunk",
                    "replacement-document",
                    "replacement chunk",
                )],
            },
            BundleDocument {
                document: recovery_document(
                    "inserted-document",
                    "recovery://inserted",
                    "inserted body",
                ),
                chunks: vec![recovery_chunk(
                    "inserted-chunk",
                    "inserted-document",
                    "inserted chunk",
                )],
            },
        ],
    };

    let report = store
        .import_recovery_bundle(
            &bundle,
            ConflictPolicy::Overwrite,
            false,
            Path::new("fixture.json"),
            "json",
        )
        .unwrap();

    assert!(report.success);
    assert!(report.durable_mutation_committed);
    assert_eq!(report.documents_overwritten, 1);
    assert_eq!(report.documents_inserted, 1);
    assert_eq!(report.conflicts, 1);
    assert_eq!(report.chunks_inserted, 2);
    assert!(store.get_document("old-document").unwrap().is_none());
    // §6.3: the replaced document's node is demoted, not deleted. It keeps the
    // uri, so the next rebuild binds it to `replacement-document` and any note
    // that linked the old one keeps a real backlink instead of a silently
    // dropped edge.
    let demoted = store
        .find_node_by_id("old-node")
        .unwrap()
        .expect("demoted stub");
    assert_eq!(demoted.kind, "stub");
    assert!(!demoted.resolved);
    assert_eq!(demoted.document_id, None);
    assert_eq!(demoted.uri.as_deref(), Some("recovery://replace"));
    assert!(store
        .list_document_revisions("old-document")
        .unwrap()
        .is_empty());
    assert!(!store
        .load_source_manifest_root(Path::new("/vault"))
        .unwrap()
        .contains_key(source_path));
    assert_eq!(
        store
            .get_document("replacement-document")
            .unwrap()
            .unwrap()
            .content,
        "replacement body"
    );
    assert_eq!(
        store
            .list_chunks_for_document("replacement-document")
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .get_document("inserted-document")
            .unwrap()
            .unwrap()
            .content,
        "inserted body"
    );
}

#[test]
fn recovery_import_refuses_missing_or_mismatched_embedding_identity() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("target.duckdb")).unwrap();
    let target_manifest = recovery_manifest();
    store.set_embedding_manifest(&target_manifest).unwrap();
    let item = BundleDocument {
        document: recovery_document("imported", "recovery://imported", "body"),
        chunks: vec![recovery_chunk("imported-chunk", "imported", "body")],
    };

    let missing = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: None,
        documents: vec![item.clone()],
    };
    let missing_error = store
        .import_recovery_bundle(
            &missing,
            ConflictPolicy::Error,
            false,
            Path::new("missing.json"),
            "json",
        )
        .unwrap_err();
    assert!(missing_error.to_string().contains("no embedding_manifest"));

    let mut foreign_manifest = target_manifest;
    foreign_manifest.model = "foreign-model".into();
    foreign_manifest.content_fingerprint = Some(
        canonical_embedding_manifest_fingerprint(&foreign_manifest)
            .expect("canonical foreign manifest fingerprint"),
    );
    let mismatched = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: Some(foreign_manifest),
        documents: vec![item],
    };
    let mismatch_error = store
        .import_recovery_bundle(
            &mismatched,
            ConflictPolicy::Error,
            false,
            Path::new("mismatch.json"),
            "json",
        )
        .unwrap_err();
    assert!(mismatch_error
        .to_string()
        .contains("does not match target corpus"));
    assert!(store.get_document("imported").unwrap().is_none());
}

#[test]
fn recovery_import_refuses_forged_fingerprint_for_empty_target() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("empty-target.duckdb")).unwrap();
    let item = BundleDocument {
        document: recovery_document("imported", "recovery://imported", "body"),
        chunks: vec![recovery_chunk("imported-chunk", "imported", "body")],
    };
    let mut forged_manifest = recovery_manifest();
    forged_manifest.content_fingerprint = Some("attacker-controlled-fingerprint".into());
    let forged = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: Some(forged_manifest),
        documents: vec![item],
    };

    let error = store
        .import_recovery_bundle(
            &forged,
            ConflictPolicy::Error,
            false,
            Path::new("forged.json"),
            "json",
        )
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("content_fingerprint does not match"));
    assert!(store.get_embedding_manifest().unwrap().is_none());
    assert!(store.get_document("imported").unwrap().is_none());
}

#[test]
fn recovery_decoder_is_strict_but_preserves_v1_metadata_only_records() {
    let document = recovery_document("document", "recovery://document", "body");
    let legacy = serde_json::json!({
        "format": "rag-recovery-bundle",
        "version": 1,
        "exported_at": Utc::now(),
        "documents": [{"document": document}],
    });
    let decoded = decode_recovery_bundle(&legacy.to_string(), "json").unwrap();
    assert_eq!(decoded.version, 1);
    assert!(decoded.documents[0].chunks.is_empty());

    let mut v2_without_chunks = legacy.clone();
    v2_without_chunks["version"] = serde_json::json!(BUNDLE_VERSION);
    let error = decode_recovery_bundle(&v2_without_chunks.to_string(), "json").unwrap_err();
    assert!(error
        .to_string()
        .contains("requires explicit field 'chunks'"));

    let mut typo = legacy;
    typo["documents"][0]["chuncks"] = serde_json::json!([]);
    let error = decode_recovery_bundle(&typo.to_string(), "json").unwrap_err();
    assert!(error.to_string().contains("unknown field 'chuncks'"));

    let bundle = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: Some(recovery_manifest()),
        documents: vec![BundleDocument {
            document: recovery_document("nested", "recovery://nested", "body"),
            chunks: vec![recovery_chunk("nested-chunk", "nested", "body")],
        }],
    };
    for (pointer, field) in [
        ("/documents/0/document", "unknown_document_field"),
        ("/documents/0/chunks/0", "unknown_chunk_field"),
        ("/embedding_manifest", "unknown_manifest_field"),
    ] {
        let mut value = serde_json::to_value(&bundle).unwrap();
        value
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(field.into(), serde_json::json!(true));
        let error = decode_recovery_bundle(&value.to_string(), "json").unwrap_err();
        assert!(
            error.to_string().contains(field),
            "expected {field} in {error}"
        );
    }
}

#[test]
fn recovery_import_rejects_empty_and_misparented_ids_without_writes() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("target.duckdb")).unwrap();
    let invalid = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: Some(recovery_manifest()),
        documents: vec![BundleDocument {
            document: recovery_document("parent", "recovery://parent", "body"),
            chunks: vec![recovery_chunk("chunk", "different-parent", "body")],
        }],
    };

    let error = store
        .import_recovery_bundle(
            &invalid,
            ConflictPolicy::Error,
            true,
            Path::new("invalid.json"),
            "json",
        )
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("expected parent document id 'parent'"));

    let empty_document_id = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: None,
        documents: vec![BundleDocument {
            document: recovery_document(" ", "recovery://empty-id", "body"),
            chunks: Vec::new(),
        }],
    };
    let error = store
        .import_recovery_bundle(
            &empty_document_id,
            ConflictPolicy::Error,
            true,
            Path::new("invalid.json"),
            "json",
        )
        .unwrap_err();
    assert!(error.to_string().contains("empty document id"));

    let empty_chunk_id = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: Some(recovery_manifest()),
        documents: vec![BundleDocument {
            document: recovery_document("parent", "recovery://parent", "body"),
            chunks: vec![recovery_chunk(" ", "parent", "body")],
        }],
    };
    let error = store
        .import_recovery_bundle(
            &empty_chunk_id,
            ConflictPolicy::Error,
            true,
            Path::new("invalid.json"),
            "json",
        )
        .unwrap_err();
    assert!(error.to_string().contains("has an empty id"));
    assert_eq!(store.stats().unwrap(), (0, 0, 0, 0));
    assert!(store.get_embedding_manifest().unwrap().is_none());
}

#[test]
fn recovery_dry_run_models_intra_bundle_document_and_uri_conflicts() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("target.duckdb")).unwrap();
    let bundle = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: None,
        documents: vec![
            BundleDocument {
                document: recovery_document("same-id", "recovery://first", "first"),
                chunks: Vec::new(),
            },
            BundleDocument {
                document: recovery_document("same-id", "recovery://second", "second"),
                chunks: Vec::new(),
            },
            BundleDocument {
                document: recovery_document("third", "recovery://shared", "third"),
                chunks: Vec::new(),
            },
            BundleDocument {
                document: recovery_document("fourth", "recovery://shared", "fourth"),
                chunks: Vec::new(),
            },
        ],
    };

    let preview = store
        .import_recovery_bundle(
            &bundle,
            ConflictPolicy::Error,
            true,
            Path::new("conflicts.json"),
            "json",
        )
        .unwrap();
    assert!(!preview.success);
    assert!(!preview.durable_mutation_committed);
    assert_eq!(preview.documents_inserted, 2);
    assert_eq!(preview.conflicts, 2);
    assert_eq!(store.stats().unwrap(), (0, 0, 0, 0));

    let applied = store
        .import_recovery_bundle(
            &bundle,
            ConflictPolicy::Error,
            false,
            Path::new("conflicts.json"),
            "json",
        )
        .unwrap();
    assert_eq!(applied.success, preview.success);
    assert_eq!(applied.documents_inserted, preview.documents_inserted);
    assert_eq!(applied.conflicts, preview.conflicts);
    assert_eq!(applied.errors, preview.errors);
    assert!(!applied.durable_mutation_committed);
    assert_eq!(store.stats().unwrap(), (0, 0, 0, 0));
}

#[test]
fn recovery_dry_run_models_intra_bundle_chunk_id_conflicts() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("target.duckdb")).unwrap();
    let bundle = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: Some(recovery_manifest()),
        documents: vec![
            BundleDocument {
                document: recovery_document("first", "recovery://first", "first"),
                chunks: vec![recovery_chunk("same-chunk", "first", "first")],
            },
            BundleDocument {
                document: recovery_document("second", "recovery://second", "second"),
                chunks: vec![recovery_chunk("same-chunk", "second", "second")],
            },
        ],
    };

    let preview_error = store
        .import_recovery_bundle(
            &bundle,
            ConflictPolicy::Error,
            true,
            Path::new("chunk-conflict.json"),
            "json",
        )
        .unwrap_err();
    assert_eq!(store.stats().unwrap(), (0, 0, 0, 0));
    assert!(store.get_embedding_manifest().unwrap().is_none());

    let apply_error = store
        .import_recovery_bundle(
            &bundle,
            ConflictPolicy::Error,
            false,
            Path::new("chunk-conflict.json"),
            "json",
        )
        .unwrap_err();
    assert_eq!(preview_error.to_string(), apply_error.to_string());
    assert_eq!(store.stats().unwrap(), (0, 0, 0, 0));
    assert!(store.get_embedding_manifest().unwrap().is_none());
}

#[test]
fn recovery_export_refuses_self_inconsistent_stored_manifest() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("source.duckdb")).unwrap();
    let mut forged_manifest = recovery_manifest();
    forged_manifest.content_fingerprint = Some("stale-or-forged".into());
    store.set_embedding_manifest(&forged_manifest).unwrap();

    let error = store.recovery_bundle().unwrap_err();

    assert!(error
        .to_string()
        .contains("content_fingerprint does not match"));
}

fn recovery_document(id: &str, uri: &str, content: &str) -> Document {
    Document {
        id: id.into(),
        uri: uri.into(),
        title: id.into(),
        content: content.into(),
        ..Document::default()
    }
}

fn recovery_chunk(id: &str, document_id: &str, content: &str) -> Chunk {
    Chunk {
        id: id.into(),
        document_id: document_id.into(),
        chunk_index: 0,
        content: content.into(),
        embedding: vec![1.0, 0.0],
        char_start: 0,
        char_end: content.chars().count() as i32,
        metadata_json: "{}".into(),
    }
}

fn recovery_manifest() -> EmbeddingManifest {
    let mut manifest = EmbeddingManifest {
        id: "default".into(),
        provider: "mock".into(),
        model: "recovery-test".into(),
        dims: 2,
        base_url: Some("https://embeddings.example/v1".into()),
        content_fingerprint: None,
        updated_at: Utc::now(),
    };
    manifest.content_fingerprint = Some(
        canonical_embedding_manifest_fingerprint(&manifest)
            .expect("canonical recovery manifest fingerprint"),
    );
    manifest
}

fn recovery_node(id: &str, document_id: &str, uri: &str) -> GraphNode {
    GraphNode {
        id: id.into(),
        kind: "document".into(),
        label: id.into(),
        document_id: Some(document_id.into()),
        uri: Some(uri.into()),
        resolved: true,
        metadata_json: "{}".into(),
    }
}

#[test]
fn backup_failure_after_sidecar_publish_removes_group_and_retry_succeeds() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("live.duckdb");
    let backup = root.path().join("backup.duckdb");
    let artifacts = backup_artifact_paths(&backup);
    let store = Store::open(&source).unwrap();

    let error = store
        .backup_database_with_publish_hook(&backup, false, false, |stage| match stage {
            BackupPublishStage::SnapshotConnectionReady => Ok(()),
            BackupPublishStage::SidecarPublished => {
                assert!(!artifacts[DATABASE_ARTIFACT].exists());
                assert!(artifacts[SHA256_ARTIFACT].is_file());
                assert!(!artifacts[METADATA_ARTIFACT].exists());
                Err(AppError::db("injected failure after sidecar publish"))
            }
            BackupPublishStage::MainPublishedAndVerified => Ok(()),
        })
        .unwrap_err();

    assert!(error.to_string().contains("injected failure"));
    assert_backup_group_absent(&backup);
    assert_no_backup_work_files(root.path());

    let report = store.backup_database(&backup, false, false).unwrap();
    assert!(report.success);
    assert_complete_backup_group(&backup);
}

#[test]
fn backup_failure_after_main_verification_removes_group_and_retry_succeeds() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("live.duckdb");
    let backup = root.path().join("backup.duckdb");
    let artifacts = backup_artifact_paths(&backup);
    let store = Store::open(&source).unwrap();

    let error = store
        .backup_database_with_publish_hook(&backup, false, false, |stage| match stage {
            BackupPublishStage::SnapshotConnectionReady => Ok(()),
            BackupPublishStage::SidecarPublished => Ok(()),
            BackupPublishStage::MainPublishedAndVerified => {
                assert!(artifacts.iter().all(|artifact| artifact.is_file()));
                Err(AppError::db("injected failure after main verification"))
            }
        })
        .unwrap_err();

    assert!(error.to_string().contains("injected failure"));
    assert_backup_group_absent(&backup);
    assert_no_backup_work_files(root.path());

    let report = store.backup_database(&backup, false, false).unwrap();
    assert!(report.success);
    assert_complete_backup_group(&backup);
}

#[cfg(unix)]
#[test]
fn backup_refuses_live_database_inode_alias() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("live.duckdb");
    let alias = root.path().join("alias.duckdb");
    let store = Store::open(&source).unwrap();
    fs::hard_link(&source, &alias).unwrap();
    let error = store.backup_database(&alias, false, true).unwrap_err();
    assert!(error.to_string().contains("inode"));
}

#[cfg(unix)]
#[test]
fn backup_refuses_a_sidecar_alias_to_the_live_database() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("live.duckdb");
    let backup = root.path().join("backup.duckdb");
    let metadata_path = PathBuf::from(format!("{}.metadata.json", backup.display()));
    let store = Store::open(&source).unwrap();
    fs::hard_link(&source, &metadata_path).unwrap();
    let original_len = fs::metadata(&source).unwrap().len();

    let error = store.backup_database(&backup, false, true).unwrap_err();

    assert!(matches!(error, AppError::Forbidden(_)));
    assert!(!backup.exists());
    assert_eq!(fs::metadata(source).unwrap().len(), original_len);
}

fn assert_backup_group_absent(path: &Path) {
    assert!(
        backup_artifact_paths(path)
            .iter()
            .all(|artifact| !artifact.exists()),
        "failed backup must not leave a visible partial group"
    );
}

fn assert_complete_backup_group(path: &Path) {
    assert!(
        backup_artifact_paths(path)
            .iter()
            .all(|artifact| artifact.is_file()),
        "successful retry must publish the complete backup group"
    );
    assert!(verify_backup(path).unwrap().ok);
}

fn assert_no_backup_work_files(dir: &Path) {
    let leaked = fs::read_dir(dir)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".rag-backup-"))
        .collect::<Vec<_>>();
    assert!(leaked.is_empty(), "leaked backup work files: {leaked:?}");
}

#[test]
fn import_preserves_document_timestamps_to_the_microsecond() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("timestamps.duckdb")).unwrap();
    let created = DateTime::parse_from_rfc3339("2024-03-05T10:11:12.123456Z")
        .unwrap()
        .with_timezone(&Utc);
    let updated = DateTime::parse_from_rfc3339("2025-01-02T03:04:05.654321Z")
        .unwrap()
        .with_timezone(&Utc);
    let mut document = recovery_document("timed", "recovery://timed", "timed body");
    document.created_at = created;
    document.updated_at = updated;
    let bundle = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: Some(recovery_manifest()),
        documents: vec![BundleDocument {
            document,
            chunks: vec![recovery_chunk("timed-chunk", "timed", "timed body")],
        }],
    };
    let report = store
        .import_recovery_bundle(
            &bundle,
            ConflictPolicy::Error,
            false,
            Path::new("fixture.json"),
            "json",
        )
        .unwrap();
    assert!(report.success, "{report:?}");
    let restored = store.get_document("timed").unwrap().unwrap();
    assert_eq!(restored.created_at, created);
    assert_eq!(restored.updated_at, updated);
    assert_eq!(store.list_chunks_for_document("timed").unwrap().len(), 1);
}

fn preflight_bundle(chunks_per_doc: &[usize]) -> RecoveryBundle {
    RecoveryBundle {
        format: "rag-mcp-recovery".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: Some(recovery_manifest()),
        documents: chunks_per_doc
            .iter()
            .enumerate()
            .map(|(d, count)| {
                let id = format!("doc-{d}");
                BundleDocument {
                    document: recovery_document(&id, &format!("recovery://{id}"), "body"),
                    chunks: (0..*count)
                        .map(|c| recovery_chunk(&format!("{id}-c{c}"), &id, "chunk"))
                        .collect(),
                }
            })
            .collect(),
    }
}

#[test]
fn preflight_recovery_bundle_counts_documents_chunks_and_vector_values() {
    let bundle = preflight_bundle(&[2, 0, 1]);
    let preflight = preflight_recovery_bundle(&bundle).unwrap();
    assert_eq!(preflight.documents, 3);
    assert_eq!(preflight.chunks, 3);
    // recovery_chunk carries two-dimensional embeddings.
    assert_eq!(preflight.embedding_values, 6);
    assert!(preflight.estimated_bytes > 0);

    let empty = preflight_recovery_bundle(&preflight_bundle(&[])).unwrap();
    assert_eq!(
        (empty.documents, empty.chunks, empty.embedding_values),
        (0, 0, 0)
    );
    assert!(empty.estimated_bytes < preflight.estimated_bytes);
}

#[test]
fn preflight_recovery_bundle_reembed_sizes_vectors_by_target_dimensions() {
    let bundle = preflight_bundle(&[2, 1]);
    let stored = preflight_recovery_bundle(&bundle).unwrap();
    let reembed = preflight_recovery_bundle_reembed(&bundle, 10).unwrap();
    assert_eq!(reembed.documents, stored.documents);
    assert_eq!(reembed.chunks, 3);
    assert_eq!(reembed.embedding_values, 30);
    // Only vector values differ; each is budgeted at the worst-case float width.
    assert_eq!(
        reembed.estimated_bytes - stored.estimated_bytes,
        (30 - 6) * JSON_FLOAT_WORST_CASE_BYTES
    );
}

#[test]
fn preflight_recovery_bundle_reembed_refuses_oversized_target_vectors() {
    let bundle = preflight_bundle(&[1]);
    assert!(preflight_recovery_bundle(&bundle).is_ok());
    let dims =
        usize::try_from(PORTABLE_RECOVERY_MAX_BYTES / JSON_FLOAT_WORST_CASE_BYTES).unwrap() + 1;
    let error = preflight_recovery_bundle_reembed(&bundle, dims).unwrap_err();
    assert!(
        error.to_string().contains("materialized-size estimate"),
        "{error}"
    );
}

#[test]
fn preflight_recovery_bundle_refuses_too_many_chunks() {
    let max = usize::try_from(PORTABLE_RECOVERY_MAX_CHUNKS).unwrap();
    let bundle = preflight_bundle(&[max + 1]);
    let error = preflight_recovery_bundle(&bundle).unwrap_err();
    assert!(
        error
            .to_string()
            .contains(&format!("contains {} chunks", max + 1)),
        "{error}"
    );
}
