//! Web ingestion endpoints (`POST /v1/ingest/url`).

use axum::{extract::State, response::IntoResponse, routing::post, Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::ingest::{IngestService, IngestUrlCommand};

use super::{
    error::{api_err, api_ok},
    HttpState,
};

pub(super) fn routes() -> Router<HttpState> {
    Router::new().route("/v1/ingest/url", post(ingest_url))
}

#[derive(Deserialize)]
struct IngestUrlBody {
    url: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    metadata_json: Option<String>,
    #[serde(default)]
    wing: Option<String>,
    #[serde(default)]
    room: Option<String>,
}

async fn ingest_url(
    State(st): State<HttpState>,
    Json(body): Json<IngestUrlBody>,
) -> impl IntoResponse {
    let fetcher = crate::web_ingest::ReqwestFetcher::new(st.config.ingest_url_timeout_secs);
    match IngestService::new(&st.store, &st.embedder, &st.config)
        .ingest_url(
            &fetcher,
            IngestUrlCommand {
                url: body.url,
                title: body.title,
                uri: body.uri,
                metadata_json: body.metadata_json,
                wing: body.wing,
                room: body.room,
            },
        )
        .await
    {
        Ok(result) => api_ok(json!({"ok": true, "result": result})),
        Err(error) => api_err(error),
    }
}
