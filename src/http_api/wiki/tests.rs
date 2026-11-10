use std::sync::Arc;

use axum::body::to_bytes;
use tower::ServiceExt;

use super::*;
use crate::config::Config;
use crate::embeddings::MockEmbedder;
use crate::models::{Document, GraphNode};

fn test_state(name: &str) -> HttpState {
    let root = crate::testkit::scratch_dir();
    let config = Config {
        db_path: root.join(format!("{name}.duckdb")),
        embedding_dims: 16,
        ..Config::for_tests()
    };
    let store = Arc::new(crate::db::Store::open(&config.db_path).unwrap());
    HttpState::new(store, false, config, Arc::new(MockEmbedder::new(16)))
}

#[tokio::test]
async fn review_queue_reports_missing_source_without_mutating_page() {
    let state = test_state("review-missing");
    let doc = Document {
            id: "review-page".into(), uri: "wiki://review-page".into(),
            title: "Review me".into(), content: "Keep original".into(),
            layer: "wiki".into(), kind: "wiki".into(),
            metadata_json: json!({"source_versions": [{"document_id":"gone", "uri":"raw://gone", "content_hash":"a".repeat(64)}]}).to_string(),
            ..Document::default()
        };
    state.store.documents().upsert_document(&doc).unwrap();
    let before = state
        .store
        .documents()
        .get_document(&doc.id)
        .unwrap()
        .unwrap();
    let response = wiki_review(State(state.clone())).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["items"][0]["link_kind"], "source_missing");
    assert_eq!(value["items"][0]["wiki_id"], doc.id);
    let after = state
        .store
        .documents()
        .get_document(&doc.id)
        .unwrap()
        .unwrap();
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.metadata_json, after.metadata_json);
    assert_eq!(before.content, after.content);
}

#[tokio::test]
async fn review_queue_empty_corpus_is_success() {
    let state = test_state("review-empty");
    let response = wiki_review(State(state)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["items"], json!([]));
}

#[tokio::test]
async fn wiki_http_routes_reject_mismatched_embedding_identity_on_create_and_update() {
    let mut state = test_state("wiki-http-manifest-mismatch");
    let created = wiki::write_wiki_page(
        &state.store,
        &state.embedder,
        &state.config,
        wiki::WikiPageWrite {
            slug: "original",
            title: "Original",
            content: "original body",
            kind: "wiki",
            category: None,
            summary: None,
            agent: None,
        },
    )
    .await
    .unwrap();
    let original_manifest = serde_json::to_value(
        state
            .store
            .embedding_manifest()
            .get_embedding_manifest()
            .unwrap(),
    )
    .unwrap();
    let original_chunks = serde_json::to_value(
        state
            .store
            .chunks()
            .list_chunks_for_document(&created.document_id)
            .unwrap(),
    )
    .unwrap();
    state.config.embedding_dims = 8;
    state.config.embedding_model = "changed-model".into();
    state.embedder = Arc::new(MockEmbedder::new(8));
    let router = super::super::api_router(state.clone());
    for payload in [
        json!({"slug":"new-page","title":"New","content":"new body"}),
        json!({"slug":"original","title":"Replacement","content":"replacement body","if_match_revision":created.revision}),
        json!({"id":created.document_id,"uri":"wiki://original","title":"Replacement","content":"replacement body","if_match_revision":created.revision}),
    ] {
        let response = router
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("PUT")
                    .uri("/v1/wiki")
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(axum::body::Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            "application/json"
        );
        let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["ok"], false);
        assert_eq!(body["code"], "EMBEDDINGS_ERROR");
        assert!(body["error"]
            .as_str()
            .unwrap()
            .contains("embedding manifest mismatch"));
    }
    assert!(state
        .store
        .documents()
        .find_by_uri("wiki://new-page")
        .unwrap()
        .is_none());
    let original = state
        .store
        .documents()
        .get_document(&created.document_id)
        .unwrap()
        .unwrap();
    assert_eq!(original.content, "original body");
    assert_eq!(original.revision, created.revision);
    assert_eq!(
        serde_json::to_value(
            state
                .store
                .embedding_manifest()
                .get_embedding_manifest()
                .unwrap()
        )
        .unwrap(),
        original_manifest
    );
    assert_eq!(
        serde_json::to_value(
            state
                .store
                .chunks()
                .list_chunks_for_document(&created.document_id)
                .unwrap()
        )
        .unwrap(),
        original_chunks
    );
}

#[tokio::test]
async fn wiki_put_with_document_id_uses_preserving_update_semantics() {
    let state = test_state("wiki-put-preserve");
    let created = wiki::write_wiki_page(
        &state.store,
        &state.embedder,
        &state.config,
        wiki::WikiPageWrite {
            slug: "project-page",
            title: "Project page",
            content: "original body",
            kind: "concept",
            category: Some("architecture"),
            summary: Some("original summary"),
            agent: None,
        },
    )
    .await
    .expect("create page");
    let mut document = state
        .store
        .documents()
        .get_document(&created.document_id)
        .unwrap()
        .expect("created document");
    document.wing = Some("alpha".into());
    document.room = Some("design".into());
    document.source_file = Some("/sources/alpha/page.md".into());
    document.status = "archived".into();
    document.pinned = true;
    document.boost = 3.0;
    document.metadata_json =
        r#"{"category":"architecture","summary":"original summary","custom":"keep"}"#.into();
    let revision = document.revision;
    document.revision = state
        .store
        .documents()
        .upsert_document_cas(&document, Some(revision))
        .expect("seed project fields");

    let response = wiki_put(
        State(state.clone()),
        Json(WikiPutBody {
            slug: Some("project-page".into()),
            id: Some(created.document_id.clone()),
            uri: Some("wiki://project-page".into()),
            title: "Edited title".into(),
            content: "edited body".into(),
            wing: None,
            room: None,
            kind: None,
            category: None,
            summary: None,
            agent: None,
            if_match_revision: Some(document.revision),
            if_match_etag: None,
        }),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);

    let updated = state
        .store
        .documents()
        .get_document(&created.document_id)
        .unwrap()
        .expect("updated document");
    assert_eq!(updated.wing.as_deref(), Some("alpha"));
    assert_eq!(updated.room.as_deref(), Some("design"));
    assert_eq!(
        updated.source_file.as_deref(),
        Some("/sources/alpha/page.md")
    );
    assert_eq!(updated.status, "archived");
    assert!(updated.pinned);
    assert_eq!(updated.boost, 3.0);
    assert_eq!(updated.kind, "concept");
    assert_eq!(updated.metadata_json, document.metadata_json);
    let index = state
        .store
        .wiki_index()
        .get_wiki_index_by_slug("project-page")
        .unwrap()
        .expect("updated index");
    assert_eq!(index.category.as_deref(), Some("architecture"));
    assert_eq!(index.summary.as_deref(), Some("original summary"));
}

#[tokio::test]
async fn wiki_put_places_new_page_in_requested_project() {
    let state = test_state("wiki-put-placement");
    let response = wiki_put(
        State(state.clone()),
        Json(WikiPutBody {
            slug: Some("alpha-overview".into()),
            id: None,
            uri: None,
            title: "Alpha overview".into(),
            content: "Project navigation".into(),
            wing: Some("alpha".into()),
            room: Some("overview".into()),
            kind: Some("source_summary".into()),
            category: Some("projects".into()),
            summary: None,
            agent: None,
            if_match_revision: None,
            if_match_etag: None,
        }),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);

    let document = state
        .store
        .documents()
        .find_by_uri("wiki://alpha-overview")
        .unwrap()
        .expect("created page");
    assert_eq!(document.wing.as_deref(), Some("alpha"));
    assert_eq!(document.room.as_deref(), Some("overview"));
}

#[tokio::test]
async fn wiki_post_never_overwrites_an_existing_archived_page() {
    let state = test_state("wiki-post-create-only");
    let app = routes().with_state(state.clone());
    let request = || {
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/wiki")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"slug":"occupied","title":"Original","content":"keep"}"#,
            ))
            .unwrap()
    };
    assert_eq!(
        app.clone().oneshot(request()).await.unwrap().status(),
        StatusCode::OK
    );
    let mut doc = state
        .store
        .documents()
        .find_by_uri("wiki://occupied")
        .unwrap()
        .unwrap();
    doc.status = "archived".into();
    state.store.documents().upsert_document(&doc).unwrap();
    let before = state
        .store
        .documents()
        .get_document(&doc.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        app.oneshot(request()).await.unwrap().status(),
        StatusCode::CONFLICT
    );
    let after = state
        .store
        .documents()
        .get_document(&doc.id)
        .unwrap()
        .unwrap();
    assert_eq!(after.content, before.content);
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.status, "archived");
}

#[tokio::test]
async fn backlinks_http_query_enforces_wing_scope() {
    let state = test_state("backlinks");
    let store = state.store.clone();
    for (id, wing) in [
        ("target", "alpha"),
        ("source-alpha", "alpha"),
        ("source-beta", "beta"),
    ] {
        store
            .documents()
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("wiki://{id}"),
                title: id.into(),
                content: id.into(),
                wing: Some(wing.into()),
                layer: "wiki".into(),
                kind: "wiki".into(),
                ..Document::default()
            })
            .unwrap();
        store
            .graph()
            .upsert_graph_node(&GraphNode {
                id: format!("node-{id}"),
                kind: "document".into(),
                label: id.into(),
                document_id: Some(id.into()),
                uri: Some(format!("wiki://{id}")),
                resolved: true,
                metadata_json: "{}".into(),
            })
            .unwrap();
    }
    for source in ["source-alpha", "source-beta"] {
        store
            .graph()
            .link_nodes(&format!("node-{source}"), "node-target", "wikilink", 1.0)
            .unwrap();
    }
    let response = wiki_backlinks(
        State(state),
        Query(BacklinksQuery {
            id: "target".into(),
            wing: Some("alpha".into()),
        }),
    )
    .await
    .into_response();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["count"], 1);
    assert_eq!(body["backlinks"][0]["id"], "source-alpha");
}
