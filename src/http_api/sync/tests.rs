use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use std::sync::Arc;
use tower::ServiceExt;

fn state(root: &std::path::Path, name: &str) -> HttpState {
    let config = crate::Config {
        db_path: root.join(format!("{name}.duckdb")),
        embedding_dims: 16,
        ..crate::Config::for_tests()
    };
    HttpState::new(
        Arc::new(crate::Store::open(&config.db_path).unwrap()),
        false,
        config,
        Arc::new(crate::embeddings::MockEmbedder::new(16)),
    )
}

fn command(body: &str) -> WikiWriteCommand {
    WikiWriteCommand {
        slug: "shared".into(),
        title: "Shared".into(),
        content: body.into(),
        wing: None,
        room: None,
        kind: "wiki".into(),
        category: None,
        summary: None,
        agent: Some("sync:untrusted-user".into()),
        options: WriteWikiOpts::default(),
    }
}

async fn local_write(st: &HttpState, node_id: &str, replica: bool, body: &str) {
    wiki::write_wiki_page_synced_command(
        &st.store,
        &st.embedder,
        &st.config,
        command(body),
        WikiSyncWrite::Local(SyncIdentity {
            node_id: node_id.into(),
            primary_url: replica.then(|| "http://primary".into()),
        }),
        false,
    )
    .await
    .unwrap();
}

fn body(st: &HttpState) -> String {
    st.store
        .documents()
        .find_by_uri("wiki://shared")
        .unwrap()
        .unwrap()
        .content
}
fn revision(st: &HttpState) -> i64 {
    st.store
        .documents()
        .find_by_uri("wiki://shared")
        .unwrap()
        .unwrap()
        .revision
}
fn cursor(st: &HttpState, node: &str) -> i64 {
    let listed = st
        .store
        .sync()
        .sync_status("replica", node, None)
        .unwrap()
        .nodes
        .into_iter()
        .find(|item| item.node_id == node)
        .map(|item| item.pull_cursor)
        .unwrap_or(0);
    assert_eq!(st.store.sync().sync_pull_cursor(node).unwrap(), listed);
    listed
}

fn event(seq: i64, body: &str) -> SyncChangeInput {
    SyncChangeInput {
        origin_seq: seq,
        entity_kind: "wiki".into(),
        entity_id: "wiki://shared".into(),
        operation: "upsert".into(),
        payload_json: json!({"slug":"shared","title":"Shared","content":body}).to_string(),
        content_hash: None,
        created_at: "2026-09-05T00:00:00.123456789Z".into(),
    }
}

async fn primary_server(st: HttpState) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, routes().with_state(st))
            .await
            .unwrap();
    });
    (base, task)
}

#[tokio::test]
async fn sync_http_validates_all_events_before_writing_and_rejects_identity_reuse() {
    let root = tempfile::tempdir().unwrap();
    let primary = state(root.path(), "primary");
    let app = routes().with_state(primary.clone());
    let first = event(1, "first");
    let mut invalid = event(2, "bad");
    invalid.created_at = "2026-09-05 00:00:00".into();
    for (events, expected) in [
        (vec![first.clone(), invalid], StatusCode::BAD_REQUEST),
        (vec![first.clone()], StatusCode::OK),
        (vec![event(1, "different")], StatusCode::CONFLICT),
    ] {
        let request = Request::builder()
            .method("POST")
            .uri("/v1/sync/push")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"node_id":"replica","changes":events}).to_string(),
            ))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            expected,
            "{}",
            String::from_utf8_lossy(&to_bytes(response.into_body(), 65536).await.unwrap())
        );
        if expected == StatusCode::BAD_REQUEST {
            assert!(primary
                .store
                .documents()
                .find_by_uri("wiki://shared")
                .unwrap()
                .is_none());
            assert!(!primary.store.sync().has_sync_change("replica", 1).unwrap());
        }
    }
    assert_eq!(body(&primary), "first");
    assert_eq!(revision(&primary), 1);
}

#[tokio::test]
async fn sync_page_journal_and_pull_cursor_rollback_together_and_retry_once() {
    let root = tempfile::tempdir().unwrap();
    let replica = state(root.path(), "replica");
    let input = event(1, "canonical");
    let canonical = SyncChange {
        primary_seq: 1,
        origin_node: "primary".into(),
        change: input,
    };
    let mut failing = command("canonical");
    failing.options.op = Some("__test_fail_wiki_after_sync_journal__".into());
    let result = wiki::write_wiki_page_synced_command(
        &replica.store,
        &replica.embedder,
        &replica.config,
        failing,
        WikiSyncWrite::Pull {
            node_id: "replica",
            event: &canonical,
        },
        false,
    )
    .await;
    assert!(result.is_err());
    assert!(replica
        .store
        .documents()
        .find_by_uri("wiki://shared")
        .unwrap()
        .is_none());
    assert!(!replica.store.sync().has_sync_change("primary", 1).unwrap());
    assert_eq!(cursor(&replica, "replica"), 0);
    apply_change(
        &replica,
        WikiSyncWrite::Pull {
            node_id: "replica",
            event: &canonical,
        },
    )
    .await
    .unwrap();
    let committed = revision(&replica);
    apply_change(
        &replica,
        WikiSyncWrite::Pull {
            node_id: "replica",
            event: &canonical,
        },
    )
    .await
    .unwrap();
    assert_eq!(revision(&replica), committed);
    assert_eq!(cursor(&replica, "replica"), 1);
    assert!(replica
        .store
        .sync()
        .pending_sync_outbox(10)
        .unwrap()
        .is_empty());

    let local = state(root.path(), "local");
    let mut failing = command("local");
    failing.options.op = Some("__test_fail_wiki_after_sync_journal__".into());
    let result = wiki::write_wiki_page_synced_command(
        &local.store,
        &local.embedder,
        &local.config,
        failing,
        WikiSyncWrite::Local(SyncIdentity {
            node_id: "local".into(),
            primary_url: Some("http://primary".into()),
        }),
        false,
    )
    .await;
    assert!(result.is_err());
    assert!(local
        .store
        .sync()
        .pending_sync_outbox(10)
        .unwrap()
        .is_empty());
    assert!(local
        .store
        .documents()
        .find_by_uri("wiki://shared")
        .unwrap()
        .is_none());
    // Public agent strings cannot opt out of journalling.
    wiki::write_wiki_page_command(
        &local.store,
        &local.embedder,
        &local.config,
        command("public"),
    )
    .await
    .unwrap();
    assert_eq!(
        local
            .store
            .sync()
            .sync_status("primary", "local", None)
            .unwrap()
            .latest_primary_seq,
        1
    );
}

#[tokio::test]
async fn sync_real_cycles_converge_offline_conflicts_and_retries_without_duplicate_revisions() {
    let root = tempfile::tempdir().unwrap();
    let primary = state(root.path(), "primary");
    let first = state(root.path(), "first");
    let second = state(root.path(), "second");
    local_write(&first, "first", true, "offline first").await;
    local_write(&second, "second", true, "offline second").await;
    let (base, server) = primary_server(primary.clone()).await;
    let client = reqwest::Client::new();
    sync_once(&first, &client, &base, "first", "first.local")
        .await
        .unwrap();
    sync_once(&second, &client, &base, "second", "second.local")
        .await
        .unwrap();
    sync_once(&first, &client, &base, "first", "first.local")
        .await
        .unwrap();
    assert_eq!(body(&primary), "offline second");
    assert_eq!(body(&first), body(&primary));
    assert_eq!(body(&second), body(&primary));
    let revisions = (revision(&first), revision(&second), revision(&primary));
    sync_once(&first, &client, &base, "first", "first.local")
        .await
        .unwrap();
    sync_once(&second, &client, &base, "second", "second.local")
        .await
        .unwrap();
    assert_eq!(
        (revision(&first), revision(&second), revision(&primary)),
        revisions
    );
    assert_eq!(cursor(&first, "first"), 2);
    assert_eq!(cursor(&second, "second"), 2);
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn sync_replays_more_than_one_page_of_events_without_losing_pending_drafts() {
    let root = tempfile::tempdir().unwrap();
    let primary = state(root.path(), "primary");
    let replica = state(root.path(), "replica");
    for i in 1..=105 {
        local_write(&replica, "replica", true, &format!("offline {i}")).await;
    }
    let (base, server) = primary_server(primary.clone()).await;
    let client = reqwest::Client::new();
    sync_once(&replica, &client, &base, "replica", "replica.local")
        .await
        .unwrap();
    assert_eq!(body(&replica), "offline 105"); // The first 100 older own events cannot erase five unsent edits.
    assert_eq!(
        replica.store.sync().pending_sync_outbox(100).unwrap().len(),
        5
    );
    assert_eq!(cursor(&replica, "replica"), 100);
    sync_once(&replica, &client, &base, "replica", "replica.local")
        .await
        .unwrap();
    assert_eq!(body(&replica), "offline 105");
    assert_eq!(body(&primary), body(&replica));
    assert_eq!(cursor(&replica, "replica"), 105);
    assert!(replica
        .store
        .sync()
        .pending_sync_outbox(100)
        .unwrap()
        .is_empty());
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn sync_edit_between_push_and_pull_survives_and_a_lost_push_reply_is_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let primary = state(root.path(), "primary");
    let replica = state(root.path(), "replica");
    local_write(&replica, "replica", true, "submitted").await;
    let submitted = replica
        .store
        .sync()
        .pending_sync_outbox(10)
        .unwrap()
        .remove(0);
    let sequence = apply_change(
        &primary,
        WikiSyncWrite::Push {
            origin_node: "replica",
            change: &submitted,
        },
    )
    .await
    .unwrap();
    let primary_revision = revision(&primary);
    local_write(&replica, "replica", true, "new draft").await;
    // Simulate a lost push response: retry the same event before marking sent.
    assert_eq!(
        apply_change(
            &primary,
            WikiSyncWrite::Push {
                origin_node: "replica",
                change: &submitted
            }
        )
        .await
        .unwrap(),
        sequence
    );
    assert_eq!(revision(&primary), primary_revision);
    replica
        .store
        .sync()
        .mark_sync_outbox_sent(&[(submitted.origin_seq, sequence)])
        .unwrap();
    let first = primary
        .store
        .sync()
        .pull_sync_changes("replica", 0, 10)
        .unwrap()
        .remove(0);
    apply_change(
        &replica,
        WikiSyncWrite::Pull {
            node_id: "replica",
            event: &first,
        },
    )
    .await
    .unwrap();
    assert_eq!(body(&replica), "new draft");
    assert_eq!(cursor(&replica, "replica"), 1);
    let (base, server) = primary_server(primary.clone()).await;
    let client = reqwest::Client::new();
    sync_once(&replica, &client, &base, "replica", "replica.local")
        .await
        .unwrap();
    assert_eq!(body(&primary), "new draft");
    assert_eq!(body(&replica), "new draft");
    assert_eq!(cursor(&replica, "replica"), 2);
    server.abort();
    let _ = server.await;
}

struct ConcurrentEmbedder {
    barrier: tokio::sync::Barrier,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl crate::embeddings::EmbeddingProvider for ConcurrentEmbedder {
    async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
        if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
            self.barrier.wait().await;
        }
        Ok(vec![vec![1.0; 16]; texts.len()])
    }
    fn dimensions(&self) -> usize {
        16
    }
}

#[tokio::test]
async fn sync_concurrent_first_writes_follow_the_committed_canonical_order() {
    let root = tempfile::tempdir().unwrap();
    let mut primary = state(root.path(), "primary");
    primary.embedder = Arc::new(ConcurrentEmbedder {
        barrier: tokio::sync::Barrier::new(2),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let left = event(1, "left");
    let right = event(1, "right");
    let (a, b) = tokio::join!(
        apply_change(
            &primary,
            WikiSyncWrite::Push {
                origin_node: "left",
                change: &left
            }
        ),
        apply_change(
            &primary,
            WikiSyncWrite::Push {
                origin_node: "right",
                change: &right
            }
        ),
    );
    let mut assignments = vec![a.unwrap(), b.unwrap()];
    assignments.sort_unstable();
    assert_eq!(assignments, vec![1, 2]);
    let canonical = primary
        .store
        .sync()
        .pull_sync_changes("observer", 0, 10)
        .unwrap();
    assert_eq!(
        body(&primary),
        validate_wiki_change(&canonical.last().unwrap().change)
            .unwrap()
            .content
    );
    let replica = state(root.path(), "replica");
    for event in &canonical {
        apply_change(
            &replica,
            WikiSyncWrite::Pull {
                node_id: "replica",
                event,
            },
        )
        .await
        .unwrap();
    }
    assert_eq!(body(&replica), body(&primary));
    assert_eq!(cursor(&replica, "replica"), 2);
}

#[tokio::test]
async fn sync_metadata_snapshot_preserves_source_versions_across_nodes() {
    let root = tempfile::tempdir().unwrap();
    let first = state(root.path(), "first");
    let primary = state(root.path(), "primary");
    let second = state(root.path(), "second");
    let metadata = json!({"source_versions":[{"document_id":"source","content_hash":"captured-hash"}],"custom":{"preserved":true}});
    let mut page = command("compiled body");
    page.options.extra_metadata = Some(metadata.clone());
    wiki::write_wiki_page_synced_command(
        &first.store,
        &first.embedder,
        &first.config,
        page,
        WikiSyncWrite::Local(SyncIdentity {
            node_id: "first".into(),
            primary_url: Some("http://primary".into()),
        }),
        false,
    )
    .await
    .unwrap();
    let outgoing = first.store.sync().pending_sync_outbox(1).unwrap().remove(0);
    let payload = validate_wiki_change(&outgoing).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&payload.metadata_json.unwrap()).unwrap(),
        metadata
    );
    apply_change(
        &primary,
        WikiSyncWrite::Push {
            origin_node: "first",
            change: &outgoing,
        },
    )
    .await
    .unwrap();
    let incoming = primary
        .store
        .sync()
        .pull_sync_changes("second", 0, 1)
        .unwrap()
        .remove(0);
    apply_change(
        &second,
        WikiSyncWrite::Pull {
            node_id: "second",
            event: &incoming,
        },
    )
    .await
    .unwrap();
    for state in [&primary, &second] {
        let doc = state
            .store
            .documents()
            .find_by_uri("wiki://shared")
            .unwrap()
            .unwrap();
        let received: serde_json::Value = serde_json::from_str(&doc.metadata_json).unwrap();
        assert_eq!(received, metadata);
    }
}

#[test]
fn sync_batches_fit_the_gateway_body_limit_and_do_not_skip_oversized_entries() {
    let medium = event(1, &"x".repeat(500_000));
    let batch = bounded_sync_batch("replica", &[medium.clone(), medium.clone(), medium]).unwrap();
    assert_eq!(batch.len(), 2);
    assert!(
        serde_json::to_vec(&json!({"node_id":"replica","changes":batch}))
            .unwrap()
            .len()
            <= super::super::MAX_HTTP_BODY_BYTES
    );
    let large = event(1, &"x".repeat(super::super::MAX_HTTP_BODY_BYTES));
    assert!(bounded_sync_batch("replica", &[large]).is_err());
}

#[tokio::test]
async fn sync_rejects_untransportable_new_wiki_without_committing_page_or_outbox() {
    let root = tempfile::tempdir().unwrap();
    let replica = state(root.path(), "replica");
    let mut page = command("small body");
    page.options.extra_metadata =
        Some(json!({"oversized":"x".repeat(crate::db::sync::MAX_SYNC_EVENT_BYTES)}));
    let result = wiki::write_wiki_page_synced_command(
        &replica.store,
        &replica.embedder,
        &replica.config,
        page,
        WikiSyncWrite::Local(SyncIdentity {
            node_id: "replica".into(),
            primary_url: Some("http://primary".into()),
        }),
        false,
    )
    .await;
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("sync event exceeds"));
    assert!(replica
        .store
        .documents()
        .find_by_uri("wiki://shared")
        .unwrap()
        .is_none());
    assert!(replica
        .store
        .sync()
        .pending_sync_outbox(10)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn sync_event_size_limit_uses_the_allocated_outbox_sequence_and_rolls_back() {
    let root = tempfile::tempdir().unwrap();
    let replica = state(root.path(), "replica");
    let mode = WikiSyncWrite::Local(SyncIdentity {
        node_id: "replica".into(),
        primary_url: Some("http://primary".into()),
    });
    let mut page = command("small body");
    page.options.extra_metadata = Some(json!({"padding":""}));
    for _ in 0..8 {
        wiki::write_wiki_page_synced_command(
            &replica.store,
            &replica.embedder,
            &replica.config,
            page.clone(),
            mode.clone(),
            false,
        )
        .await
        .unwrap();
    }
    let pending = replica.store.sync().pending_sync_outbox(10).unwrap();
    let padding = crate::db::sync::MAX_SYNC_EVENT_BYTES
        - serde_json::to_vec(pending.last().unwrap()).unwrap().len();
    page.options.extra_metadata = Some(json!({"padding":"x".repeat(padding)}));
    wiki::write_wiki_page_synced_command(
        &replica.store,
        &replica.embedder,
        &replica.config,
        page.clone(),
        mode.clone(),
        false,
    )
    .await
    .unwrap();
    let pending = replica.store.sync().pending_sync_outbox(10).unwrap();
    assert_eq!(pending.last().unwrap().origin_seq, 9);
    assert_eq!(
        serde_json::to_vec(pending.last().unwrap()).unwrap().len(),
        crate::db::sync::MAX_SYNC_EVENT_BYTES
    );
    let before = replica
        .store
        .documents()
        .find_by_uri("wiki://shared")
        .unwrap()
        .unwrap();
    let error = wiki::write_wiki_page_synced_command(
        &replica.store,
        &replica.embedder,
        &replica.config,
        page,
        mode,
        false,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("sync event exceeds"));
    let after = replica
        .store
        .documents()
        .find_by_uri("wiki://shared")
        .unwrap()
        .unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.content, before.content);
    assert_eq!(after.metadata_json, before.metadata_json);
    assert_eq!(
        replica.store.sync().pending_sync_outbox(10).unwrap(),
        pending
    );
}

fn replica_metadata_edit(st: &HttpState, metadata: serde_json::Value) {
    let document = st
        .store
        .documents()
        .find_by_uri("wiki://shared")
        .unwrap()
        .unwrap();
    st.store
        .documents()
        .update_document_meta_for_sync_test(
            &document.id,
            &crate::models::DocumentMetaUpdate {
                metadata_json: Some(metadata.to_string()),
                ..Default::default()
            },
            &SyncIdentity {
                node_id: "replica".into(),
                primary_url: Some("http://primary".into()),
            },
        )
        .unwrap()
        .unwrap();
}

fn metadata(st: &HttpState) -> serde_json::Value {
    serde_json::from_str(
        &st.store
            .documents()
            .find_by_uri("wiki://shared")
            .unwrap()
            .unwrap()
            .metadata_json,
    )
    .unwrap()
}

#[tokio::test]
async fn sync_metadata_edits_around_push_ack_and_pull_converge() {
    let root = tempfile::tempdir().unwrap();
    let primary = state(root.path(), "primary");
    let replica = state(root.path(), "replica");
    let observer = state(root.path(), "observer");
    let mut page = command("body");
    page.options.extra_metadata = Some(json!({"version":"M1"}));
    wiki::write_wiki_page_synced_command(
        &replica.store,
        &replica.embedder,
        &replica.config,
        page,
        WikiSyncWrite::Local(SyncIdentity {
            node_id: "replica".into(),
            primary_url: Some("http://primary".into()),
        }),
        false,
    )
    .await
    .unwrap();
    replica_metadata_edit(&replica, json!({"version":"M2"}));
    let outgoing = replica.store.sync().pending_sync_outbox(10).unwrap();
    assert_eq!(outgoing.len(), 2);
    let (base, server) = primary_server(primary.clone()).await;
    let client = reqwest::Client::new();
    let response: serde_json::Value = client
        .post(format!("{base}/v1/sync/push"))
        .json(&json!({"node_id":"replica", "changes":outgoing}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["primary_sequences"], json!([1, 2]));
    // An edit while the push reply is in flight, then another after ack.
    replica_metadata_edit(&replica, json!({"version":"M3"}));
    replica
        .store
        .sync()
        .mark_sync_outbox_sent(&[(1, 1), (2, 2)])
        .unwrap();
    replica_metadata_edit(&replica, json!({"version":"M4"}));
    for event in primary
        .store
        .sync()
        .pull_sync_changes("replica", 0, 10)
        .unwrap()
    {
        apply_change(
            &replica,
            WikiSyncWrite::Pull {
                node_id: "replica",
                event: &event,
            },
        )
        .await
        .unwrap();
        assert_eq!(metadata(&replica), json!({"version":"M4"}));
    }
    sync_once(&replica, &client, &base, "replica", "replica.local")
        .await
        .unwrap();
    sync_once(&observer, &client, &base, "observer", "observer.local")
        .await
        .unwrap();
    for node in [&primary, &replica, &observer] {
        assert_eq!(metadata(node), json!({"version":"M4"}));
    }
    // A further metadata-only edit after the previous pull must also publish.
    replica_metadata_edit(&replica, json!({"version":"M5", "source_versions":[]}));
    for _ in 0..2 {
        sync_once(&replica, &client, &base, "replica", "replica.local")
            .await
            .unwrap();
        sync_once(&observer, &client, &base, "observer", "observer.local")
            .await
            .unwrap();
    }
    for node in [&primary, &replica, &observer] {
        assert_eq!(
            metadata(node),
            json!({"version":"M5", "source_versions":[]})
        );
    }
    assert_eq!(
        primary
            .store
            .sync()
            .pull_sync_changes("observer", 0, 10)
            .unwrap()
            .len(),
        5
    );
    assert!(replica
        .store
        .sync()
        .pending_sync_outbox(10)
        .unwrap()
        .is_empty());
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn sync_direct_metadata_update_rolls_back_when_journal_fails() {
    let root = tempfile::tempdir().unwrap();
    let replica = state(root.path(), "replica");
    local_write(&replica, "replica", true, "body").await;
    let before = replica
        .store
        .documents()
        .find_by_uri("wiki://shared")
        .unwrap()
        .unwrap();
    let catalog = replica
        .store
        .wiki_index()
        .get_wiki_index_by_slug("shared")
        .unwrap()
        .unwrap();
    let pending = replica.store.sync().pending_sync_outbox(10).unwrap();
    replica
        .store
        .lock()
        .unwrap()
        .execute_batch("DROP TABLE sync_nodes")
        .unwrap();
    let result = replica
        .store
        .documents()
        .update_document_meta_for_sync_test(
            &before.id,
            &crate::models::DocumentMetaUpdate {
                title: Some("should roll back".into()),
                metadata_json: Some(json!({"version":"M2", "category":"changed"}).to_string()),
                ..Default::default()
            },
            &SyncIdentity {
                node_id: "replica".into(),
                primary_url: Some("http://primary".into()),
            },
        );
    assert!(result.is_err());
    let after = replica
        .store
        .documents()
        .get_document(&before.id)
        .unwrap()
        .unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.title, before.title);
    assert_eq!(after.metadata_json, before.metadata_json);
    let after_catalog = replica
        .store
        .wiki_index()
        .get_wiki_index_by_slug("shared")
        .unwrap()
        .unwrap();
    assert_eq!(after_catalog.title, catalog.title);
    assert_eq!(after_catalog.category, catalog.category);
    assert_eq!(
        replica.store.sync().pending_sync_outbox(10).unwrap(),
        pending
    );
}

#[tokio::test]
async fn sync_generic_atomic_wiki_write_journals_only_replicated_fields() {
    let root = tempfile::tempdir().unwrap();
    let primary = state(root.path(), "primary");
    local_write(&primary, "primary", false, "body").await;
    let mut doc = primary
        .store
        .documents()
        .find_by_uri("wiki://shared")
        .unwrap()
        .unwrap();
    doc.title = "Changed title".into();
    doc.wing = Some("project".into());
    doc.room = Some("architecture".into());
    doc.kind = "reference".into();
    doc.metadata_json = json!({"version":"M2", "category":"design"}).to_string();
    primary
        .store
        .documents()
        .write_document_atomic(
            &doc,
            Some(doc.revision),
            crate::db::store::DocumentDerivedWrite::Preserve,
        )
        .unwrap();
    let events = primary
        .store
        .sync()
        .pull_sync_changes("observer", 0, 10)
        .unwrap();
    assert_eq!(events.len(), 2);
    let payload = validate_wiki_change(&events[1].change).unwrap();
    assert_eq!(payload.title, doc.title);
    assert_eq!(payload.wing, doc.wing);
    assert_eq!(payload.room, doc.room);
    assert_eq!(payload.kind.as_deref(), Some("reference"));
    assert_eq!(
        payload.metadata_json.as_deref(),
        Some(doc.metadata_json.as_str())
    );
    // Generic metadata-only lifecycle changes remain local and do not add events.
    let update = crate::models::DocumentMetaUpdate {
        status: Some("archived".into()),
        pinned: Some(true),
        boost: Some(2.0),
        source_file: Some("local/path.md".into()),
        ..Default::default()
    };
    primary
        .store
        .documents()
        .update_document_meta(&doc.id, &update)
        .unwrap()
        .unwrap();
    let mut local = primary
        .store
        .documents()
        .get_document(&doc.id)
        .unwrap()
        .unwrap();
    local.status = "active".into();
    local.pinned = false;
    local.boost = 3.0;
    local.source_file = None;
    primary
        .store
        .documents()
        .write_document_atomic(
            &local,
            Some(local.revision),
            crate::db::store::DocumentDerivedWrite::Preserve,
        )
        .unwrap();
    let raw = crate::models::Document {
        id: "raw-with-wiki-uri".into(),
        uri: "wiki://raw-with-wiki-uri".into(),
        title: "Raw source".into(),
        content: "Immutable source text".into(),
        layer: "raw".into(),
        ..Default::default()
    };
    primary
        .store
        .documents()
        .write_document_atomic(&raw, None, crate::db::store::DocumentDerivedWrite::Preserve)
        .unwrap();
    primary
        .store
        .documents()
        .update_document_meta(
            &raw.id,
            &crate::models::DocumentMetaUpdate {
                metadata_json: Some(json!({"raw-only":true}).to_string()),
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        primary
            .store
            .sync()
            .pull_sync_changes("observer", 0, 10)
            .unwrap(),
        events
    );
}
