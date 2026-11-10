//! MCP-vs-HTTP parity: the same operation through the MCP tool and through the
//! HTTP handler, on one seeded store.
//!
//! These tests PIN TODAY'S BEHAVIOUR before Stage 4 of docs/refactor/PLAN.md
//! moves both transports onto shared use cases. Where the transports agree,
//! the fields are compared with each other. Where they differ today, each
//! side's current value is asserted and the difference carries a `// DRIFT:`
//! comment; the same list lives in PLAN.md under "Known MCP/HTTP drifts (to
//! align in Stage 4)". When Stage 4 aligns a drift on purpose, the matching
//! assertion changes in the same commit.
//!
//! The MCP side goes through the real `RagServer` dispatch (`tools/call` over
//! the stateless streamable-HTTP service, so surface, role and audit checks
//! run). The HTTP side goes through the API router with `tower::oneshot`.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use chrono::Utc;
use rag_mcp::access::AccessRole;
use rag_mcp::context::AppContext;
use rag_mcp::embeddings::{EmbeddingProvider, MockEmbedder};
use rag_mcp::http_api::{self, HttpState};
use rag_mcp::models::{Chunk, Document};
use rag_mcp::testkit::{open_temp_store, test_config};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const DIMS: usize = 32;

/// JSON-RPC `invalid_params`; rmcp maps most `AppError`s onto it.
const JSONRPC_INVALID_PARAMS: i64 = -32602;
/// JSON-RPC code rmcp uses for `resource_not_found`.
const JSONRPC_RESOURCE_NOT_FOUND: i64 = -32002;

struct Harness {
    _dir: tempfile::TempDir,
    context: AppContext,
    http: axum::Router,
}

/// Outcome of one MCP `tools/call`.
#[derive(Debug)]
enum McpReply {
    /// Tool result: the JSON carried in the first text content block.
    Ok(Value),
    /// JSON-RPC error object (`code`, `message`, optional `data`).
    Err(Value),
}

impl McpReply {
    fn ok(self) -> Value {
        match self {
            McpReply::Ok(value) => value,
            McpReply::Err(error) => panic!("expected MCP tool result, got error {error}"),
        }
    }

    fn err(self) -> Value {
        match self {
            McpReply::Err(error) => error,
            McpReply::Ok(value) => panic!("expected MCP error, got result {value}"),
        }
    }
}

impl Harness {
    fn new() -> Self {
        let (dir, store) = open_temp_store();
        let config = test_config(store.path().to_path_buf(), DIMS);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(DIMS));
        let context = AppContext::new(store, embedder, config);
        let http = http_api::router_for_tests(HttpState::from_context(context.clone(), true));
        Self {
            _dir: dir,
            context,
            http,
        }
    }

    /// One raw document with a single mock-embedded chunk.
    async fn seed_document(&self, id: &str, title: &str, text: &str) {
        let now = Utc::now();
        self.context
            .store
            .documents()
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("text://{id}"),
                title: title.into(),
                content: text.into(),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                ..Default::default()
            })
            .expect("upsert document");
        let embedding = self
            .context
            .embedder
            .embed(&[text.to_string()])
            .await
            .expect("embed")
            .remove(0);
        self.context
            .store
            .chunks()
            .insert_chunks(&[Chunk {
                id: format!("{id}-c0"),
                document_id: id.into(),
                chunk_index: 0,
                content: text.into(),
                embedding,
                char_start: 0,
                char_end: text.len() as i32,
                metadata_json: "{}".into(),
            }])
            .expect("insert chunk");
    }

    /// Eight documents about distinct topics; more than `default_top_k` (5).
    async fn seed_corpus(&self) {
        for (index, topic) in [
            "harbor", "orchard", "glacier", "lantern", "meadow", "quarry", "saddle", "tundra",
        ]
        .iter()
        .enumerate()
        {
            self.seed_document(
                &format!("doc-{index}"),
                &format!("Notes on the {topic}"),
                &format!("The {topic} report describes parity retrieval for the {topic} survey."),
            )
            .await;
        }
    }

    async fn mcp(&self, tool: &str, arguments: Value) -> McpReply {
        let service =
            http_api::mcp_http_service_for(self.context.clone(), CancellationToken::new());
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": tool, "arguments": arguments },
        });
        let mut request = Request::builder()
            .method("POST")
            .uri("/mcp")
            .header(header::HOST, "localhost")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .body(Body::from(body.to_string()))
            .expect("mcp request");
        // The gateway's auth layer normally attaches the caller's role.
        request.extensions_mut().insert(AccessRole::Admin);
        let response = tokio::time::timeout(std::time::Duration::from_secs(60), async {
            let response = service.handle(request).await;
            let status = response.status();
            let bytes = axum::body::to_bytes(Body::new(response.into_body()), usize::MAX)
                .await
                .expect("mcp body");
            (
                status,
                String::from_utf8(bytes.to_vec()).expect("utf-8 body"),
            )
        })
        .await
        .expect("mcp call finished");
        let (status, text) = response;
        assert_eq!(status, StatusCode::OK, "mcp transport status; body: {text}");
        // Stateless streamable HTTP frames the JSON-RPC response as SSE.
        let message = text
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
            .find(|message| message.get("id") == Some(&json!(1)))
            .unwrap_or_else(|| panic!("no JSON-RPC response in: {text}"));
        if let Some(error) = message.get("error") {
            return McpReply::Err(error.clone());
        }
        let result = &message["result"];
        assert_ne!(
            result["isError"],
            json!(true),
            "tool reported isError: {result}"
        );
        let payload = result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("tool result without text content: {result}"));
        McpReply::Ok(serde_json::from_str(payload).expect("tool result is JSON"))
    }

    async fn http(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        let body = match body {
            Some(value) => {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
                Body::from(value.to_string())
            }
            None => Body::empty(),
        };
        let response = self
            .http
            .clone()
            .oneshot(builder.body(body).expect("http request"))
            .await
            .expect("http response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("http body");
        let value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, value)
    }
}

fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("expected object, got {value}"))
        .keys()
        .cloned()
        .collect()
}

fn strings(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| value.to_string()).collect()
}

/// Ranking-relevant projection of a hit list (timings in `explanation` differ per call).
///
/// DRIFT: float formatting. Scores are `f32`. MCP serializes the hit structs
/// directly (shortest f32 form, `0.03226646`); HTTP goes through
/// `serde_json::json!`, which widens to f64 (`0.03226646035909653`). The values
/// are equal as f32, so they are compared as f32 here.
fn ranking(hits: &Value) -> Vec<(String, String, f32)> {
    hits.as_array()
        .unwrap_or_else(|| panic!("expected hit array, got {hits}"))
        .iter()
        .map(|hit| {
            (
                hit["chunk_id"].as_str().expect("chunk_id").to_string(),
                hit["document_id"]
                    .as_str()
                    .expect("document_id")
                    .to_string(),
                hit["score"].as_f64().expect("score") as f32,
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// search
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_returns_the_same_ranking_in_different_envelopes() {
    let harness = Harness::new();
    harness.seed_corpus().await;
    let arguments = json!({ "query": "glacier survey report", "mode": "hybrid", "top_k": 3 });

    let mcp = harness.mcp("search", arguments.clone()).await.ok();
    let (status, http) = harness.http("POST", "/v1/search", Some(arguments)).await;
    assert_eq!(status, StatusCode::OK);

    // DRIFT: envelope. MCP returns the bare hit array; HTTP wraps it as
    // `{ok, mode, count, timings, items}`.
    assert!(mcp.is_array());
    assert_eq!(
        keys(&http),
        strings(&["ok", "mode", "count", "timings", "items"])
    );
    assert_eq!(http["ok"], json!(true));
    assert_eq!(http["mode"], json!("hybrid"));
    assert_eq!(http["count"], json!(3));

    // Agreement: same hits, same order, same scores, same hit fields.
    assert_eq!(ranking(&mcp).len(), 3);
    assert_eq!(ranking(&mcp), ranking(&http["items"]));
    assert_eq!(keys(&mcp[0]), keys(&http["items"][0]));
    // The float-formatting drift, pinned on the raw JSON text of one score.
    assert_ne!(
        mcp[0]["score"].to_string(),
        http["items"][0]["score"].to_string()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_defaults_and_top_k_bounds_agree() {
    let harness = Harness::new();
    harness.seed_corpus().await;

    // Default mode (config: vec) and default top_k (config: 5) on both sides.
    let arguments = json!({ "query": "orchard survey report" });
    let mcp = harness.mcp("search", arguments.clone()).await.ok();
    let (status, http) = harness.http("POST", "/v1/search", Some(arguments)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(http["mode"], json!("vec"));
    assert_eq!(ranking(&mcp).len(), harness.context.config.default_top_k);
    assert_eq!(ranking(&mcp), ranking(&http["items"]));

    // top_k = 0 returns no hits on both (it is not raised to 1 on this path).
    let arguments = json!({ "query": "orchard survey report", "mode": "lex", "top_k": 0 });
    let mcp = harness.mcp("search", arguments.clone()).await.ok();
    let (status, http) = harness.http("POST", "/v1/search", Some(arguments)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ranking(&mcp).len(), 0);
    assert_eq!(http["count"], json!(0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_errors_carry_the_same_code_in_different_shapes() {
    let harness = Harness::new();
    harness.seed_corpus().await;

    // Unknown mode: the same domain error on both sides.
    let arguments = json!({ "query": "harbor", "mode": "bogus" });
    let mcp = harness.mcp("search", arguments.clone()).await.err();
    let (status, http) = harness.http("POST", "/v1/search", Some(arguments)).await;
    // DRIFT: error shape. MCP: JSON-RPC error -32602 with `data.code`;
    // HTTP: status 400 with `{ok:false, code, error}`.
    assert_eq!(mcp["code"], json!(JSONRPC_INVALID_PARAMS));
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(keys(&http), strings(&["ok", "code", "error"]));
    assert_eq!(http["ok"], json!(false));
    // Agreement: the wire code.
    assert_eq!(mcp["data"]["code"], json!("INVALID_REQUEST"));
    assert_eq!(mcp["data"]["code"], http["code"]);
    // DRIFT: message text. MCP sends the bare message; HTTP sends the
    // `AppError` display form, which prefixes the variant ("config error: ").
    let message = "invalid search mode 'bogus': expected 'vec', 'lex', or 'hybrid'";
    assert_eq!(mcp["message"], json!(message));
    assert_eq!(http["error"], json!(format!("config error: {message}")));

    // Wrong parameter type is rejected by each transport's own decoder.
    let arguments = json!({ "query": "harbor", "top_k": "three" });
    let mcp = harness.mcp("search", arguments.clone()).await.err();
    let (status, http) = harness.http("POST", "/v1/search", Some(arguments)).await;
    // DRIFT: decode errors. MCP: -32602 without `data.code`; HTTP: axum's 422
    // with a plain-text body (no JSON envelope).
    assert_eq!(mcp["code"], json!(JSONRPC_INVALID_PARAMS));
    assert!(mcp.get("data").is_none_or(Value::is_null));
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(http.is_string());
}

// ---------------------------------------------------------------------------
// pack_context
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pack_context_packs_identically_with_an_extra_ok_on_http() {
    let harness = Harness::new();
    harness.seed_corpus().await;
    let (_, found) = harness
        .http(
            "POST",
            "/v1/search",
            Some(json!({ "query": "meadow survey report", "mode": "lex", "top_k": 4 })),
        )
        .await;
    // Only the fields `pack_context` accepts (same shape on both transports).
    let hits: Vec<Value> = found["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|hit| {
            json!({
                "chunk_id": hit["chunk_id"],
                "document_id": hit["document_id"],
                "document_title": hit["document_title"],
                "document_uri": hit["document_uri"],
                "chunk_index": hit["chunk_index"],
                "content": hit["content"],
                "score": hit["score"],
            })
        })
        .collect();
    assert!(!hits.is_empty());

    for max_tokens in [None, Some(40), Some(0)] {
        let mut arguments = json!({ "hits": hits });
        if let Some(limit) = max_tokens {
            arguments["max_tokens"] = json!(limit);
        }
        let mcp = harness.mcp("pack_context", arguments.clone()).await.ok();
        let (status, http) = harness
            .http("POST", "/v1/pack-context", Some(arguments))
            .await;
        assert_eq!(status, StatusCode::OK);

        // Transport envelope (not drift): HTTP adds `ok: true`; the MCP
        // result is bare. `trace` appears on both only when requested.
        let shared = [
            "hits",
            "total_tokens",
            "max_tokens",
            "omitted_count",
            "context_text",
        ];
        assert_eq!(keys(&mcp), strings(&shared));
        let mut with_ok = shared.to_vec();
        with_ok.push("ok");
        assert_eq!(keys(&http), strings(&with_ok));

        // Agreement: every shared field, including the default budget.
        for field in [
            "total_tokens",
            "max_tokens",
            "omitted_count",
            "context_text",
        ] {
            assert_eq!(
                mcp[field], http[field],
                "{field} with max_tokens={max_tokens:?}"
            );
        }
        // Packed hits: same chunks and scores (f32 formatting drift as in search).
        assert_eq!(ranking(&mcp["hits"]), ranking(&http["hits"]));
        let expected_budget = max_tokens.unwrap_or(harness.context.config.max_context_tokens);
        assert_eq!(mcp["max_tokens"], json!(expected_budget));
        if max_tokens == Some(0) {
            assert_eq!(mcp["context_text"], json!(""));
            assert_eq!(mcp["hits"], json!([]));
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pack_context_returns_a_trace_on_request_on_both_transports() {
    let harness = Harness::new();
    harness.seed_corpus().await;
    let (_, found) = harness
        .http(
            "POST",
            "/v1/search",
            Some(json!({ "query": "orchard survey", "mode": "lex", "top_k": 2 })),
        )
        .await;
    let hits: Vec<Value> = found["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|hit| {
            json!({
                "chunk_id": hit["chunk_id"],
                "document_id": hit["document_id"],
                "document_title": hit["document_title"],
                "document_uri": hit["document_uri"],
                "chunk_index": hit["chunk_index"],
                "content": hit["content"],
                "score": hit["score"],
            })
        })
        .collect();
    let arguments = json!({ "hits": hits, "include_trace": true });
    // Stage 4 aligned: `include_trace` / `persist_trace` / `prior_trace` on both.
    let mcp = harness.mcp("pack_context", arguments.clone()).await.ok();
    let (status, http) = harness
        .http("POST", "/v1/pack-context", Some(arguments))
        .await;
    assert_eq!(status, StatusCode::OK);
    for reply in [&mcp, &http] {
        assert_eq!(reply["trace"]["path"], json!("search_pack"), "{reply}");
        assert_eq!(reply["trace"]["spans"][0]["name"], json!("pack_context"));
    }
}

// ---------------------------------------------------------------------------
// multi_get
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_get_agrees_on_order_missing_and_document_shape() {
    let harness = Harness::new();
    harness.seed_corpus().await;
    let arguments = json!({
        "document_ids": ["doc-3", "no-such-document", "doc-1"],
        "include_chunks": true,
    });

    let mcp = harness.mcp("multi_get", arguments.clone()).await.ok();
    let (status, http) = harness.http("POST", "/v1/multi-get", Some(arguments)).await;
    assert_eq!(status, StatusCode::OK);

    // Transport envelope (not drift): MCP `{documents, missing}`; HTTP
    // `{ok, items, missing}`.
    assert_eq!(keys(&mcp), strings(&["documents", "missing"]));
    assert_eq!(keys(&http), strings(&["ok", "items", "missing"]));

    // Stage 4 aligned: one item shape on both transports. The full row sits
    // under `document`; the earlier flat MCP fields stay beside it.
    let (mcp_item, http_item) = (&mcp["documents"][0], &http["items"][0]);
    let item_keys = strings(&[
        "document",
        "id",
        "uri",
        "title",
        "content",
        "metadata_json",
        "created_at",
        "updated_at",
        "chunks",
        "chunks_total",
        "chunks_truncated",
    ]);
    assert_eq!(keys(mcp_item), item_keys);
    assert_eq!(keys(http_item), item_keys);
    assert_eq!(mcp_item, http_item);
    let http_doc = &http_item["document"];
    for full_row in ["layer", "kind", "status", "revision"] {
        assert!(http_doc.get(full_row).is_some(), "document has {full_row}");
    }

    // Agreement: request order, missing ids, document text, chunk paging.
    let mcp_ids: Vec<&Value> = mcp["documents"]
        .as_array()
        .expect("documents")
        .iter()
        .map(|item| &item["document"]["id"])
        .collect();
    let http_ids: Vec<&Value> = http["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| &item["document"]["id"])
        .collect();
    assert_eq!(mcp_ids, vec![&json!("doc-3"), &json!("doc-1")]);
    assert_eq!(mcp_ids, http_ids);
    assert_eq!(mcp["missing"], json!(["no-such-document"]));
    assert_eq!(mcp["missing"], http["missing"]);
    for field in ["id", "uri", "title", "content", "metadata_json"] {
        assert_eq!(mcp_item[field], http_doc[field], "{field}");
    }
    for field in ["chunks", "chunks_total", "chunks_truncated"] {
        assert_eq!(mcp_item[field], http_item[field], "{field}");
    }
    assert_eq!(mcp_item["chunks_total"], json!(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_get_rejects_an_empty_id_list_with_the_same_code() {
    let harness = Harness::new();
    let arguments = json!({ "document_ids": [] });

    let mcp = harness.mcp("multi_get", arguments.clone()).await.err();
    let (status, http) = harness.http("POST", "/v1/multi-get", Some(arguments)).await;

    // DRIFT: error shape (JSON-RPC -32602 vs HTTP 400), same as `search`.
    assert_eq!(mcp["code"], json!(JSONRPC_INVALID_PARAMS));
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Agreement: the wire code.
    assert_eq!(mcp["data"]["code"], json!("INVALID_REQUEST"));
    assert_eq!(mcp["data"]["code"], http["code"]);
    // DRIFT: message text (bare on MCP, variant-prefixed on HTTP).
    let message = "document_ids must contain 1..=100 ids";
    assert_eq!(mcp["message"], json!(message));
    assert_eq!(http["error"], json!(format!("invalid request: {message}")));
}

// ---------------------------------------------------------------------------
// wiki write: create, CAS update, stale write
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wiki_create_returns_the_same_fields_on_both_transports() {
    let harness = Harness::new();
    let page = |slug: &str| json!({ "slug": slug, "title": "Parity page", "content": "Body of the parity page." });

    let mcp = harness
        .mcp("write_wiki_page", page("parity-mcp"))
        .await
        .ok();
    let (status, http) = harness
        .http("PUT", "/v1/wiki", Some(page("parity-http")))
        .await;
    assert_eq!(status, StatusCode::OK);

    // DRIFT: envelope. HTTP adds `ok: true` to the same nine result fields.
    let fields = [
        "document_id",
        "uri",
        "slug",
        "chunk_count",
        "node_id",
        "edge_count",
        "index_id",
        "revision",
        "etag",
    ];
    assert_eq!(keys(&mcp), strings(&fields));
    let mut with_ok = fields.to_vec();
    with_ok.push("ok");
    assert_eq!(keys(&http), strings(&with_ok));

    // Agreement: a first write is revision 1 with a weak etag, same chunking.
    for (reply, slug) in [(&mcp, "parity-mcp"), (&http, "parity-http")] {
        assert_eq!(reply["slug"], json!(slug));
        assert_eq!(reply["uri"], json!(format!("wiki://{slug}")));
        assert_eq!(reply["revision"], json!(1));
        assert_eq!(reply["etag"], json!("W/\"1\""));
    }
    assert_eq!(mcp["chunk_count"], http["chunk_count"]);

    // Agreement: both pages are stored as layer=wiki, kind=wiki.
    for reply in [&mcp, &http] {
        let id = reply["document_id"].as_str().expect("document_id");
        let stored = harness
            .context
            .store
            .documents()
            .get_document(id)
            .unwrap()
            .expect("stored page");
        assert_eq!(stored.layer, "wiki");
        assert_eq!(stored.kind.to_string(), "wiki");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wiki_create_on_an_existing_slug_differs_by_entrypoint() {
    let harness = Harness::new();
    let page = |slug: &str, content: &str| json!({ "slug": slug, "title": "Occupied", "content": content });
    harness
        .mcp("write_wiki_page", page("occupied-mcp", "first"))
        .await
        .ok();
    let (status, _) = harness
        .http("POST", "/v1/wiki", Some(page("occupied-http", "first")))
        .await;
    assert_eq!(status, StatusCode::OK);

    // DRIFT: create semantics. MCP has no create-only tool: `write_wiki_page`
    // on an existing slug overwrites it (revision 2). HTTP `POST /v1/wiki` is
    // create-only and answers 409 CONFLICT; HTTP `PUT` is the upsert.
    let mcp = harness
        .mcp("write_wiki_page", page("occupied-mcp", "second"))
        .await
        .ok();
    assert_eq!(mcp["revision"], json!(2));
    let (status, http) = harness
        .http("POST", "/v1/wiki", Some(page("occupied-http", "second")))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(http["code"], json!("CONFLICT"));
    let (status, http) = harness
        .http("PUT", "/v1/wiki", Some(page("occupied-http", "second")))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(http["revision"], json!(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wiki_cas_update_and_stale_write_agree_on_revision_and_conflict_code() {
    let harness = Harness::new();
    let page = |slug: &str, content: &str, if_match: Option<i64>| {
        let mut body = json!({ "slug": slug, "title": "CAS page", "content": content });
        if let Some(revision) = if_match {
            body["if_match_revision"] = json!(revision);
        }
        body
    };
    harness
        .mcp("write_wiki_page", page("cas-mcp", "v1", None))
        .await
        .ok();
    harness
        .http("PUT", "/v1/wiki", Some(page("cas-http", "v1", None)))
        .await;

    // CAS update with the current revision succeeds on both.
    // DRIFT: entrypoint. MCP updates through `update_wiki_page` (preserves
    // omitted kind/category/summary); HTTP uses the same `PUT /v1/wiki` as
    // create, with `if_match_revision` in the body.
    let mcp = harness
        .mcp("update_wiki_page", page("cas-mcp", "v2", Some(1)))
        .await
        .ok();
    let (status, http) = harness
        .http("PUT", "/v1/wiki", Some(page("cas-http", "v2", Some(1))))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mcp["revision"], json!(2));
    assert_eq!(mcp["revision"], http["revision"]);
    assert_eq!(mcp["etag"], json!("W/\"2\""));
    assert_eq!(mcp["etag"], http["etag"]);

    // Stale write (revision 1 again) is refused on both and changes nothing.
    let mcp = harness
        .mcp("update_wiki_page", page("cas-mcp", "v3", Some(1)))
        .await
        .err();
    let (status, http) = harness
        .http("PUT", "/v1/wiki", Some(page("cas-http", "v3", Some(1))))
        .await;
    // DRIFT: conflict signalling. MCP has no distinct conflict class: JSON-RPC
    // -32602 (invalid_params) with `data.code = "CONFLICT"`. HTTP: 409.
    assert_eq!(mcp["code"], json!(JSONRPC_INVALID_PARAMS));
    assert_eq!(status, StatusCode::CONFLICT);
    // Agreement: the wire code.
    assert_eq!(mcp["data"]["code"], json!("CONFLICT"));
    assert_eq!(mcp["data"]["code"], http["code"]);
    for slug in ["cas-mcp", "cas-http"] {
        let stored = harness
            .context
            .store
            .documents()
            .find_by_uri(&format!("wiki://{slug}"))
            .unwrap()
            .expect("page");
        assert_eq!(stored.revision, 2);
        assert_eq!(stored.content, "v2");
    }

    // The etag form of If-Match is accepted by both.
    let mut mcp_body = page("cas-mcp", "v3", None);
    mcp_body["if_match_etag"] = json!("W/\"2\"");
    let mut http_body = page("cas-http", "v3", None);
    http_body["if_match_etag"] = json!("W/\"2\"");
    let mcp = harness.mcp("update_wiki_page", mcp_body).await.ok();
    let (status, http) = harness.http("PUT", "/v1/wiki", Some(http_body)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(mcp["revision"], json!(3));
    assert_eq!(mcp["revision"], http["revision"]);
}

// ---------------------------------------------------------------------------
// get_neighbors
// ---------------------------------------------------------------------------

/// A wikilink chain `chain-0 → chain-1 → … → chain-6`; returns the node ids.
async fn seed_chain(harness: &Harness) -> Vec<String> {
    let mut node_ids = Vec::new();
    for index in (0..7).rev() {
        let content = if index < 6 {
            format!("Step {index} links to [[Chain {}]].", index + 1)
        } else {
            "Last step.".to_string()
        };
        let reply = harness
            .mcp(
                "write_wiki_page",
                json!({ "slug": format!("chain-{index}"), "title": format!("Chain {index}"), "content": content }),
            )
            .await
            .ok();
        node_ids.push(reply["node_id"].as_str().expect("node_id").to_string());
    }
    node_ids.reverse();
    node_ids
}

fn node_ids(view: &Value) -> BTreeSet<String> {
    view["nodes"]
        .as_array()
        .unwrap_or_else(|| panic!("expected graph view, got {view}"))
        .iter()
        .map(|node| node["id"].as_str().expect("node id").to_string())
        .collect()
}

fn edge_triples(view: &Value) -> BTreeSet<(String, String, String)> {
    view["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .map(|edge| {
            (
                edge["source_id"].as_str().expect("source").to_string(),
                edge["target_id"].as_str().expect("target").to_string(),
                edge["rel_type"].as_str().expect("rel_type").to_string(),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn neighbors_agree_on_the_default_local_graph() {
    let harness = Harness::new();
    let chain = seed_chain(&harness).await;
    let seed = &chain[3];

    // Stage 4 aligned: `node_id` and `seed` are aliases on both transports.
    for argument in ["node_id", "seed"] {
        let mcp = harness
            .mcp("get_neighbors", json!({ argument: seed }))
            .await
            .ok();
        let (status, http) = harness
            .http("GET", &format!("/v1/neighbors?{argument}={seed}"), None)
            .await;
        assert_eq!(status, StatusCode::OK);

        // Agreement: default depth 1 gives the seed and both wikilink
        // neighbours; the same bare `{nodes, edges}` view on both.
        assert_eq!(keys(&mcp), strings(&["nodes", "edges"]));
        assert_eq!(keys(&http), strings(&["nodes", "edges"]));
        let expected: BTreeSet<String> = chain[2..=4].iter().cloned().collect();
        assert_eq!(node_ids(&mcp), expected);
        assert_eq!(node_ids(&mcp), node_ids(&http));
        assert_eq!(edge_triples(&mcp), edge_triples(&http));
        assert_eq!(edge_triples(&mcp).len(), 2);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn neighbors_share_depth_cap_seed_resolution_and_unknown_seed_rules() {
    let harness = Harness::new();
    let chain = seed_chain(&harness).await;
    let seed = &chain[0];

    // Stage 4 aligned: one depth cap (3) on both.
    let mcp = harness
        .mcp("get_neighbors", json!({ "node_id": seed, "depth": 9 }))
        .await
        .ok();
    let (status, http) = harness
        .http("GET", &format!("/v1/neighbors?seed={seed}&depth=9"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let capped: BTreeSet<String> = chain[..=3].iter().cloned().collect();
    assert_eq!(node_ids(&mcp), capped);
    assert_eq!(node_ids(&http), capped);

    // Stage 4 aligned: both resolve a node id, document id, URI or exact label.
    let (status, http) = harness
        .http("GET", "/v1/neighbors?seed=Chain%200", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(node_ids(&http).contains(seed));
    let mcp = harness
        .mcp("get_neighbors", json!({ "node_id": "Chain 0" }))
        .await
        .ok();
    assert_eq!(node_ids(&mcp), node_ids(&http));

    // Stage 4 aligned: an unknown seed is NOT_FOUND on both.
    let mcp = harness
        .mcp("get_neighbors", json!({ "node_id": "no-such-node" }))
        .await
        .err();
    assert_eq!(mcp["code"], json!(JSONRPC_RESOURCE_NOT_FOUND));
    assert_eq!(mcp["data"]["code"], json!("NOT_FOUND"));
    let (status, http) = harness
        .http("GET", "/v1/neighbors?seed=no-such-node", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(http["code"], json!("NOT_FOUND"));

    // Stage 4 aligned: a blank seed is INVALID_REQUEST on both.
    let (status, http) = harness.http("GET", "/v1/neighbors?seed=%20", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(http["code"], json!("INVALID_REQUEST"));
    let mcp = harness
        .mcp("get_neighbors", json!({ "node_id": " " }))
        .await
        .err();
    assert_eq!(mcp["code"], json!(JSONRPC_INVALID_PARAMS));
    assert_eq!(mcp["data"]["code"], json!("INVALID_REQUEST"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_wiki_page_is_not_found_on_both_transports() {
    let harness = Harness::new();

    let mcp = harness
        .mcp("get_wiki_page", json!({ "id_or_slug": "absent" }))
        .await
        .err();
    let (status, http) = harness
        .http("GET", "/v1/document?uri=wiki://absent", None)
        .await;

    // DRIFT: not-found signalling. MCP: JSON-RPC -32002 (resource_not_found);
    // HTTP: 404. Agreement: `NOT_FOUND` wire code.
    assert_eq!(mcp["code"], json!(JSONRPC_RESOURCE_NOT_FOUND));
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(mcp["data"]["code"], json!("NOT_FOUND"));
    assert_eq!(mcp["data"]["code"], http["code"]);
}
