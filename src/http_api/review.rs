//! Review proposals shared by the UI and API clients; synthesis stays client-side.
use super::{
    error::{api_err, api_ok},
    HttpState,
};
use crate::{
    error::AppError,
    review,
    wiki::{self, WikiWriteCommand, WriteWikiOpts},
};
use axum::{extract::State, response::Response, routing::get, Json, Router};
use serde::Deserialize;
use serde_json::json;

pub(super) fn routes() -> Router<HttpState> {
    Router::new().route("/v1/wiki-proposals", get(list).post(change))
}
async fn list(State(st): State<HttpState>) -> Response {
    match super::run_blocking("review proposals", move || review::list(&st.store)).await {
        Ok(items) => api_ok(json!({"items":items})),
        Err(e) => api_err(e),
    }
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Change {
    Create {
        document_id: String,
    },
    Save {
        id: String,
        revision: i64,
        content: String,
    },
    Reject {
        id: String,
        revision: i64,
    },
    Accept {
        id: String,
        revision: i64,
    },
}
async fn change(State(st): State<HttpState>, Json(body): Json<Change>) -> Response {
    let result = match body {
        Change::Accept { id, revision } => accept(&st, &id, revision).await,
        other => {
            super::run_blocking("change review proposal", move || match other {
                Change::Create { document_id } => review::create(&st.store, &document_id),
                Change::Save {
                    id,
                    revision,
                    content,
                } => review::update(&st.store, &id, revision, content, false),
                Change::Reject { id, revision } => {
                    let p = review::get(&st.store, &id)?;
                    review::update(&st.store, &id, revision, p.content, true)
                }
                Change::Accept { .. } => unreachable!(),
            })
            .await
        }
    };
    match result {
        Ok(proposal) => api_ok(proposal),
        Err(e) => api_err(e),
    }
}
async fn accept(st: &HttpState, id: &str, revision: i64) -> crate::error::Result<review::Proposal> {
    let p = review::get(&st.store, id)?;
    if p.revision != revision || p.status != "pending" {
        return Err(AppError::conflict("proposal changed; reload it"));
    }
    let versions: Vec<_> = p
        .sources
        .iter()
        .filter_map(|source| {
            source.content_hash.as_ref().map(|hash|
        json!({"document_id":source.document_id,"uri":source.uri,"content_hash":hash}))
        })
        .collect();
    let slug = crate::util::wiki_slug_from_uri(&p.base.uri)
        .ok_or_else(|| AppError::config("proposal requires a wiki URI"))?;
    wiki::write_wiki_page_command(
        &st.store,
        &st.embedder,
        &st.config,
        WikiWriteCommand {
            slug,
            title: p.base.title.clone(),
            content: p.content.clone(),
            wing: p.base.wing.clone(),
            room: p.base.room.clone(),
            kind: p.base.kind.clone(),
            category: None,
            summary: None,
            agent: Some("knowledge-review".into()),
            options: WriteWikiOpts {
                op: Some("wiki_review_accept".into()),
                if_match_revision: Some(p.base.revision),
                extra_metadata: Some(json!({"source_versions":versions})),
                extra_payload: Some(json!({"proposal_id":p.id,"proposal_revision":p.revision})),
                ..Default::default()
            },
        },
    )
    .await?;
    review::get(&st.store, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config, db::Store, embeddings::MockEmbedder, models::Document, util::content_hash,
    };
    use std::sync::Arc;

    async fn fixture() -> (HttpState, review::Proposal) {
        let root = tempfile::tempdir().unwrap().keep();
        let config = Config {
            db_path: root.join("review.duckdb"),
            embedding_dims: 16,
            ..Config::for_tests()
        };
        let store = Arc::new(Store::open(&config.db_path).unwrap());
        let st = HttpState::new(store, false, config, Arc::new(MockEmbedder::new(16)));
        st.store
            .upsert_document(&Document {
                id: "source".into(),
                uri: "raw://source".into(),
                content: "source version one".into(),
                layer: "raw".into(),
                ..Default::default()
            })
            .unwrap();
        let page = wiki::write_wiki_page_command(&st.store, &st.embedder, &st.config, WikiWriteCommand {
            slug:"review-test".into(), title:"Review".into(), content:"Before".into(), kind:"wiki".into(), wing:None, room:None, category:None, summary:None, agent:None,
            options: WriteWikiOpts { extra_metadata:Some(json!({"source_versions":[{"document_id":"source","uri":"raw://source","content_hash":content_hash("source version one")}]})), ..Default::default() },
        }).await.unwrap();
        let p = review::create(&st.store, &page.document_id).unwrap();
        let p = review::update(&st.store, &p.id, p.revision, "After".into(), false).unwrap();
        (st, p)
    }
    #[tokio::test]
    async fn proposal_accepts_article_and_provenance_once() {
        let (st, p) = fixture().await;
        let accepted = accept(&st, &p.id, p.revision).await.unwrap();
        assert_eq!(accepted.status, "accepted");
        let page = st.store.get_document(&p.base.id).unwrap().unwrap();
        assert_eq!(page.content, "After");
        assert_eq!(page.revision, p.base.revision + 1);
        let metadata: serde_json::Value = serde_json::from_str(&page.metadata_json).unwrap();
        assert_eq!(
            metadata["source_versions"][0]["content_hash"],
            content_hash("source version one")
        );
        assert!(matches!(
            accept(&st, &p.id, p.revision).await,
            Err(AppError::Conflict(_))
        ));
    }
    #[tokio::test]
    async fn proposal_rejects_changed_source_and_retains_draft() {
        let (st, p) = fixture().await;
        let mut source = st.store.get_document("source").unwrap().unwrap();
        source.content = "new source".into();
        st.store.upsert_document(&source).unwrap();
        assert!(matches!(
            accept(&st, &p.id, p.revision).await,
            Err(AppError::Conflict(_))
        ));
        assert_eq!(review::get(&st.store, &p.id).unwrap().status, "pending");
        assert_eq!(
            st.store.get_document(&p.base.id).unwrap().unwrap().content,
            "Before"
        );
    }
    #[tokio::test]
    async fn proposal_rejects_changed_article_and_draft_revision() {
        let (st, p) = fixture().await;
        let next =
            review::update(&st.store, &p.id, p.revision, "New proposal".into(), false).unwrap();
        assert!(matches!(
            accept(&st, &p.id, p.revision).await,
            Err(AppError::Conflict(_))
        ));
        let mut page = st.store.get_document(&p.base.id).unwrap().unwrap();
        page.content = "Concurrent author".into();
        st.store.upsert_document(&page).unwrap();
        assert!(matches!(
            accept(&st, &p.id, next.revision).await,
            Err(AppError::Conflict(_))
        ));
        assert_eq!(
            review::get(&st.store, &p.id).unwrap().content,
            "New proposal"
        );
    }
    #[tokio::test]
    async fn missing_source_reappearing_with_new_id_blocks_acceptance() {
        let (st, p) = fixture().await;
        st.store.delete_document("source").unwrap();
        let p = review::create(&st.store, &p.base.id).unwrap();
        assert_eq!(p.sources[0].uri, "raw://source");
        assert!(p.sources[0].content_hash.is_none());
        st.store
            .upsert_document(&Document {
                id: "replacement".into(),
                uri: "raw://source".into(),
                content: "replacement".into(),
                layer: "raw".into(),
                ..Default::default()
            })
            .unwrap();
        assert!(matches!(
            accept(&st, &p.id, p.revision).await,
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn acceptance_marker_rolls_back_with_transaction() {
        let (st, p) = fixture().await;
        let mut doc = p.base.clone();
        doc.content = p.content.clone();
        let audit = crate::models::OpsLogEntry {
            id: "test-audit".into(),
            seq: 0,
            ts: chrono::Utc::now(),
            op: "wiki_review_accept".into(),
            prefix: None,
            message: "test".into(),
            entity_id: Some(doc.id.clone()),
            entity_kind: Some("wiki".into()),
            payload_json: json!({"proposal_id":p.id,"proposal_revision":p.revision}).to_string(),
            agent_name: None,
        };
        {
            let mut conn = st.store.lock().unwrap();
            let tx = conn.transaction().unwrap();
            review::accept_locked(&tx, &doc, &audit).unwrap();
            // Simulate a later write failure by dropping without commit.
        }
        assert_eq!(review::get(&st.store, &p.id).unwrap().status, "pending");
        assert_eq!(
            st.store.get_document(&doc.id).unwrap().unwrap().content,
            "Before"
        );
    }

    #[tokio::test]
    async fn proposal_survives_database_reopen() {
        let (st, p) = fixture().await;
        let path = st.config.db_path.clone();
        drop(st);
        let reopened = Store::open(&path).unwrap();
        let restored = review::get(&reopened, &p.id).unwrap();
        assert_eq!(restored.content, "After");
        assert_eq!(restored.status, "pending");
        eprintln!("review fixture: {}", path.display());
    }

    #[tokio::test]
    async fn rejected_proposal_cannot_publish() {
        let (st, p) = fixture().await;
        let rejected =
            review::update(&st.store, &p.id, p.revision, p.content.clone(), true).unwrap();
        assert!(matches!(
            accept(&st, &p.id, rejected.revision).await,
            Err(AppError::Conflict(_))
        ));
        assert_eq!(
            st.store.get_document(&p.base.id).unwrap().unwrap().content,
            "Before"
        );
        assert_eq!(review::list(&st.store).unwrap().len(), 1);
    }
}
