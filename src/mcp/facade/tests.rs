//! Behavior tests for the composed MCP server: tool calls, guards, and policy.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::Utc;

use super::*;
use crate::db::recovery::{
    decode_recovery_bundle, encode_recovery_bundle, BundleDocument, RecoveryBundle, BUNDLE_VERSION,
};
use crate::db::schema::SCHEMA_VERSION;
use crate::db::search::SearchQuery;
use crate::diagnostics::DiagnosticsService;
use crate::diagnostics::{DoctorReport, StatusReport};
use crate::embeddings::MockEmbedder;
use crate::mcp::maintain::clamp_maintenance_max_docs;
use crate::mcp::recovery::{prepare_recovery_bundle_for_import, LEGACY_RECOVERY_BUNDLE_VERSION};
use crate::mcp::tools::*;
use crate::models::{Chunk, Document, SearchMode};
use crate::util::{check_path_allowlist, content_hash, parse_optional_timestamp};

impl RagServer {
    /// MemPalace-style `status` health payload (vision §5.5 layer health).
    fn status_report(&self) -> Result<StatusReport, AppError> {
        DiagnosticsService::new(&self.store, &self.config).status()
    }

    /// Minimal `doctor` integrity payload.
    fn doctor_report(&self) -> Result<DoctorReport, AppError> {
        DiagnosticsService::new(&self.store, &self.config).doctor()
    }
}

#[test]
fn every_registered_tool_has_an_explicit_access_policy() {
    for tool in RagServer::all_tools_router().list_all() {
        assert!(
            crate::access::required_tool_role(tool.name.as_ref()).is_some(),
            "missing access policy for {}",
            tool.name
        );
    }
}

#[test]
fn every_registered_tool_has_an_explicit_telemetry_group() {
    for tool in RagServer::all_tools_router().list_all() {
        assert!(
            crate::telemetry::explicit_tool_group(tool.name.as_ref()).is_some(),
            "missing telemetry group for {}",
            tool.name
        );
    }
}

#[test]
fn maintenance_max_docs_never_exceeds_configured_boundary() {
    assert_eq!(clamp_maintenance_max_docs(50, None), 50);
    assert_eq!(clamp_maintenance_max_docs(50, Some(0)), 1);
    assert_eq!(clamp_maintenance_max_docs(50, Some(12)), 12);
    assert_eq!(clamp_maintenance_max_docs(50, Some(500)), 50);
    assert_eq!(clamp_maintenance_max_docs(0, Some(500)), 1);
}

struct CountingEmbedder {
    calls: Arc<AtomicUsize>,
    dims: usize,
}

struct GatedEmbedder {
    started: Arc<tokio::sync::Semaphore>,
    release: Arc<tokio::sync::Semaphore>,
    dims: usize,
}

#[async_trait]
impl EmbeddingProvider for CountingEmbedder {
    async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(vec![vec![1.0; self.dims]; texts.len()])
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

#[async_trait]
impl EmbeddingProvider for GatedEmbedder {
    async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
        self.started.add_permits(1);
        let permit = self
            .release
            .acquire()
            .await
            .expect("release semaphore open");
        permit.forget();
        Ok(vec![vec![0.25; self.dims]; texts.len()])
    }

    fn dimensions(&self) -> usize {
        self.dims
    }
}

fn test_config(db_path: PathBuf, roots: Vec<PathBuf>, dims: usize) -> Config {
    Config {
        db_path,
        embedding_model: "mock".to_string(),
        embedding_dims: dims,
        ingest_roots: roots,
        llm_enabled: false,
        ..Config::for_tests()
    }
}

#[tokio::test]
async fn pack_context_mcp_enforces_complete_context_budget() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("pack-mcp.duckdb");
    let config = test_config(path.clone(), Vec::new(), 2);
    let store = Store::open(&path).unwrap();
    let server = RagServer::new(store, Arc::new(MockEmbedder::new(2)), config);
    for budget in [0, 1, 32] {
        let params: PackContextParams = serde_json::from_value(serde_json::json!({
            "max_tokens": budget,
            "hits": [{"chunk_id":"c", "document_id":"d", "document_title":"Doc", "document_uri":"doc://d", "chunk_index":0, "score":0.9, "content":"expanded body ".repeat(2_000), "snippet":"hidden source ".repeat(2_000)}],
        })).unwrap();
        let result = server.pack_context(Parameters(params)).await.unwrap();
        let text = &result.content[0].as_text().unwrap().text;
        let result: serde_json::Value = serde_json::from_str(text).unwrap();
        assert!(result["total_tokens"].as_u64().unwrap() <= budget);
        assert_eq!(
            crate::search_pack::estimate_tokens(result["context_text"].as_str().unwrap()),
            result["total_tokens"].as_u64().unwrap() as usize
        );
        assert!(!text.contains("hidden source"));
        if budget < 32 {
            assert!(result["hits"].as_array().unwrap().is_empty());
        } else {
            assert_eq!(result["hits"].as_array().unwrap().len(), 1);
        }
    }
}

#[test]
fn status_and_doctor_empty_corpus() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("status.duckdb");
    let store = Store::open(&path).expect("open");
    let dims = 32usize;
    let config = test_config(path.clone(), Vec::new(), dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);

    let status = server.status_report().expect("status");
    assert_eq!(status.backend, "duckdb");
    assert_eq!(status.schema_version, SCHEMA_VERSION);
    assert_eq!(status.document_count, 0);
    assert_eq!(status.chunk_count, 0);
    assert!(status.wings.is_empty());
    assert!(!status.ready_for_search);
    assert!(!status.ingest_roots_configured);
    assert_eq!(status.embed_dims, dims);

    let doctor = server.doctor_report().expect("doctor");
    assert!(doctor.schema_ok);
    assert!(doctor.embed_ok);
    assert!(doctor.ok);
    assert!(!doctor.ready_for_search);
    assert!(!doctor.ingest_roots_configured);
    assert_eq!(doctor.expected_schema_version, SCHEMA_VERSION);
    // Server start records embedding_manifest from config when missing.
    assert_eq!(doctor.manifest_dims, Some(dims as i32));
}

#[tokio::test]
async fn doctor_repair_rebuilds_missing_chunks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("doctor-repair.duckdb");
    let store = Store::open(&path).expect("open");
    let dims = 16usize;
    let config = test_config(path, Vec::new(), dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);
    let now = Utc::now();
    server
        .store
        .upsert_document(&Document {
            id: "missing-chunks".into(),
            uri: "file://missing.md".into(),
            title: "Missing chunks".into(),
            content: "This document must become searchable after repair.".into(),
            metadata_json: "{}".into(),
            created_at: now,
            updated_at: now,
            layer: "raw".into(),
            kind: "document".into(),
            ..Default::default()
        })
        .unwrap();
    server.store.ensure_fts(&server.config.fts_stemmer).unwrap();
    assert_eq!(server.doctor_report().unwrap().documents_without_chunks, 1);

    server
        .doctor_repair(Parameters(DoctorRepairParams {
            dry_run: Some(true),
            max_docs: Some(10),
        }))
        .await
        .unwrap();
    assert!(server
        .store
        .list_chunks_for_document("missing-chunks")
        .unwrap()
        .is_empty());

    let search_guard = server.store.corpus_mutation_lane().read_owned().await;
    let busy = server
        .doctor_repair(Parameters(DoctorRepairParams {
            dry_run: Some(false),
            max_docs: Some(10),
        }))
        .await
        .unwrap_err();
    assert_eq!(busy.data.as_ref().unwrap()["code"], "STORE_BUSY");
    drop(search_guard);

    server
        .doctor_repair(Parameters(DoctorRepairParams {
            dry_run: Some(false),
            max_docs: Some(10),
        }))
        .await
        .unwrap();
    assert!(!server
        .store
        .list_chunks_for_document("missing-chunks")
        .unwrap()
        .is_empty());
    assert_eq!(server.doctor_report().unwrap().documents_without_chunks, 0);
    let generation_after_repair = {
        let conn = server.store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert!(!generation_after_repair.dirty);
    let hits = crate::db::search::search(
        &server.store,
        &SearchQuery {
            mode: SearchMode::Lex,
            query_text: Some("searchable".into()),
            top_k: 5,
            fts_stemmer: server.config.fts_stemmer.clone(),
            ..SearchQuery::default()
        },
    )
    .unwrap();
    assert_eq!(hits.len(), 1);
    let generation_after_search = {
        let conn = server.store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert_eq!(generation_after_search, generation_after_repair);
}

#[tokio::test]
async fn import_bundle_finishes_with_generation_clean_fts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("import-target.duckdb");
    let bundle_path = dir.path().join("bundle.json");
    let dims = 8usize;
    let config = test_config(path.clone(), vec![dir.path().to_path_buf()], dims);
    let store = Store::open(&path).expect("open");
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);
    server.store.ensure_fts(&server.config.fts_stemmer).unwrap();
    let body = "portable searchable recovery body";
    let bundle = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: Some(crate::db::store::embedding_manifest_from_config(
            &server.config,
        )),
        documents: vec![BundleDocument {
            document: Document {
                id: "imported-document".into(),
                uri: "recovery://imported-document".into(),
                title: "Imported document".into(),
                content: body.into(),
                layer: "wiki".into(),
                kind: "wiki".into(),
                ..Document::default()
            },
            chunks: vec![Chunk {
                id: "imported-chunk".into(),
                document_id: "imported-document".into(),
                chunk_index: 0,
                content: body.into(),
                embedding: vec![0.2; dims],
                char_start: 0,
                char_end: body.len() as i32,
                metadata_json: "{}".into(),
            }],
        }],
    };
    std::fs::write(
        &bundle_path,
        encode_recovery_bundle(&bundle, "json").unwrap(),
    )
    .unwrap();

    let search_guard = server.store.corpus_mutation_lane().read_owned().await;
    let busy = server
        .import_bundle(Parameters(ImportBundleParams {
            path: bundle_path.display().to_string(),
            format: Some("json".into()),
            conflict_policy: Some("error".into()),
            dry_run: Some(false),
            reembed_legacy: None,
        }))
        .await
        .unwrap_err();
    assert_eq!(busy.data.as_ref().unwrap()["code"], "STORE_BUSY");
    drop(search_guard);

    let result = server
        .import_bundle(Parameters(ImportBundleParams {
            path: bundle_path.display().to_string(),
            format: Some("json".into()),
            conflict_policy: Some("error".into()),
            dry_run: Some(false),
            reembed_legacy: None,
        }))
        .await
        .expect("import bundle");
    let result_json: serde_json::Value =
        serde_json::from_str(&result.content[0].as_text().expect("text result").text).unwrap();
    assert_eq!(result_json["success"], true);
    assert_eq!(result_json["durable_mutation_committed"], true);
    assert_eq!(result_json["documents_inserted"], 1);
    let generation_after_import = {
        let conn = server.store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert!(!generation_after_import.dirty);

    let _hits = crate::db::search::search(
        &server.store,
        &SearchQuery {
            mode: SearchMode::Lex,
            query_text: Some("portable".into()),
            top_k: 5,
            fts_stemmer: server.config.fts_stemmer.clone(),
            ..SearchQuery::default()
        },
    )
    .unwrap();
    let generation_after_search = {
        let conn = server.store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert_eq!(generation_after_search, generation_after_import);
}

#[tokio::test]
async fn legacy_bundle_requires_opt_in_and_reembeds_before_import() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("legacy-import-target.duckdb");
    let bundle_path = dir.path().join("legacy-v1.json");
    let dims = 8usize;
    let config = test_config(path.clone(), vec![dir.path().to_path_buf()], dims);
    let store = Store::open(&path).expect("open");
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);
    let body = "legacy vector must be replaced";
    let legacy_vector = vec![42.0; dims];
    let bundle = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: LEGACY_RECOVERY_BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: None,
        documents: vec![BundleDocument {
            document: Document {
                id: "legacy-document".into(),
                uri: "recovery://legacy-document".into(),
                title: "Legacy document".into(),
                content: body.into(),
                ..Document::default()
            },
            chunks: vec![Chunk {
                id: "legacy-chunk".into(),
                document_id: "legacy-document".into(),
                chunk_index: 0,
                content: body.into(),
                embedding: legacy_vector.clone(),
                char_start: 0,
                char_end: body.len() as i32,
                metadata_json: "{}".into(),
            }],
        }],
    };
    std::fs::write(
        &bundle_path,
        encode_recovery_bundle(&bundle, "json").unwrap(),
    )
    .unwrap();

    let refused = server
        .import_bundle(Parameters(ImportBundleParams {
            path: bundle_path.display().to_string(),
            format: Some("json".into()),
            conflict_policy: Some("error".into()),
            dry_run: Some(false),
            reembed_legacy: Some(false),
        }))
        .await
        .unwrap_err();
    assert!(refused.message.contains("reembed_legacy=true"));
    assert!(server
        .store
        .get_document("legacy-document")
        .unwrap()
        .is_none());

    let result = server
        .import_bundle(Parameters(ImportBundleParams {
            path: bundle_path.display().to_string(),
            format: Some("json".into()),
            conflict_policy: Some("error".into()),
            dry_run: Some(false),
            reembed_legacy: Some(true),
        }))
        .await
        .expect("legacy import with explicit re-embed");
    let result_json: serde_json::Value =
        serde_json::from_str(&result.content[0].as_text().expect("text result").text).unwrap();
    assert_eq!(result_json["success"], true);
    assert_eq!(result_json["legacy_bundle_version"], 1);
    assert_eq!(result_json["legacy_reembed_requested"], true);
    assert_eq!(result_json["embeddings_reembed_planned"], 1);
    assert_eq!(result_json["embeddings_reembedded"], 1);
    let chunks = server
        .store
        .list_chunks_for_document("legacy-document")
        .unwrap();
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].embedding.len(), dims);
    assert_ne!(chunks[0].embedding, legacy_vector);
    assert!(server
        .store
        .embedding_manifest_matches_config(&server.config)
        .unwrap());
}

#[tokio::test]
async fn legacy_reembed_preflight_rejects_oversized_vectors_before_provider_call() {
    let root = tempfile::tempdir().unwrap();
    let dimensions =
        usize::try_from(crate::db::recovery::PORTABLE_RECOVERY_MAX_BYTES / 64 + 1).unwrap();
    let config = test_config(root.path().join("unused.duckdb"), Vec::new(), dimensions);
    let calls = Arc::new(AtomicUsize::new(0));
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(CountingEmbedder {
        calls: Arc::clone(&calls),
        dims: dimensions,
    });
    let bundle = RecoveryBundle {
        format: "rag-recovery-bundle".into(),
        version: LEGACY_RECOVERY_BUNDLE_VERSION,
        exported_at: Utc::now(),
        embedding_manifest: None,
        documents: vec![BundleDocument {
            document: Document {
                id: "legacy-large-vector".into(),
                uri: "recovery://legacy-large-vector".into(),
                title: "Legacy".into(),
                content: "body".into(),
                ..Document::default()
            },
            chunks: vec![Chunk {
                id: "legacy-large-vector-chunk".into(),
                document_id: "legacy-large-vector".into(),
                chunk_index: 0,
                content: "body".into(),
                embedding: Vec::new(),
                char_start: 0,
                char_end: 4,
                metadata_json: "{}".into(),
            }],
        }],
    };

    let error =
        match prepare_recovery_bundle_for_import(bundle, true, true, &embedder, &config).await {
            Ok(_) => panic!("oversized prospective vectors must be refused"),
            Err(error) => error,
        };

    assert!(error.to_string().contains("verified DuckDB backup"));
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn bulk_maintenance_apply_paths_refuse_active_guarded_search() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("maintenance-lane.duckdb");
    let dims = 8usize;
    let config = test_config(path.clone(), Vec::new(), dims);
    let store = Store::open(&path).expect("open");
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);
    let search_guard = server.store.corpus_mutation_lane().read_owned().await;

    let plan_error = server
        .apply_maintenance_plan(Parameters(ApplyMaintenancePlanParams {
            actions: vec![crate::mcp::tools::MaintenanceActionParams {
                action: "reindex_fts".into(),
                reason: None,
                target_id: None,
                params: None,
            }],
            dry_run: Some(false),
            max_docs: None,
            agent: None,
        }))
        .await
        .unwrap_err();
    assert_eq!(plan_error.data.as_ref().unwrap()["code"], "STORE_BUSY");

    let organize_error = server
        .maintain_organize(Parameters(MaintainOrganizeParams {
            dry_run: Some(false),
            ..Default::default()
        }))
        .await
        .unwrap_err();
    assert_eq!(organize_error.data.as_ref().unwrap()["code"], "STORE_BUSY");

    let refresh_error = server
        .maintain_refresh(Parameters(MaintainRefreshParams {
            reindex_fts: Some(true),
            dry_run: Some(false),
            ..Default::default()
        }))
        .await
        .unwrap_err();
    assert_eq!(refresh_error.data.as_ref().unwrap()["code"], "STORE_BUSY");

    let compress_error = server
        .maintain_compress(Parameters(MaintainCompressParams {
            level: Some(0),
            dry_run: Some(false),
            ..Default::default()
        }))
        .await
        .unwrap_err();
    assert_eq!(compress_error.data.as_ref().unwrap()["code"], "STORE_BUSY");

    drop(search_guard);

    // The write guard moves into the blocking compression task and must
    // not deadlock or outlive a normally completed request.
    server
        .maintain_compress(Parameters(MaintainCompressParams {
            level: Some(0),
            dry_run: Some(false),
            ..Default::default()
        }))
        .await
        .expect("compression after guarded search");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_blocking_waiter_keeps_transferred_mutation_guard() {
    let lane = Arc::new(tokio::sync::RwLock::new(()));
    let mutation_guard = lane
        .clone()
        .try_write_owned()
        .expect("initial mutation guard");
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();

    let waiter = tokio::spawn(async move {
        retain_mutation_guard_while_blocking(mutation_guard, move || {
            let _ = started_tx.send(());
            release_rx.recv().expect("release blocking tail");
        })
        .await
    });
    started_rx.await.expect("blocking tail started");
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert!(
        lane.clone().try_write_owned().is_err(),
        "detached blocking tail must retain the mutation guard"
    );

    release_tx.send(()).expect("release blocking tail");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if let Ok(guard) = lane.clone().try_write_owned() {
                drop(guard);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("mutation guard released after blocking tail");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_apply_request_leaves_owned_mutation_workflow_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("owned-maintenance.duckdb");
    let dims = 8usize;
    let store = Store::open(&path).expect("open");
    let config = test_config(path, Vec::new(), dims);
    store.ensure_embedding_manifest(&config).unwrap();
    store
        .upsert_document(&Document {
            id: "owned-doc".into(),
            uri: "wiki://owned-doc".into(),
            title: "Owned document".into(),
            content: "owned workflow searchable body".into(),
            layer: "wiki".into(),
            kind: "wiki".into(),
            ..Document::default()
        })
        .expect("seed document");
    store
        .insert_chunks(&[Chunk {
            id: "owned-chunk".into(),
            document_id: "owned-doc".into(),
            chunk_index: 0,
            content: "owned workflow searchable body".into(),
            embedding: vec![0.0; dims],
            char_start: 0,
            char_end: 30,
            metadata_json: "{}".into(),
        }])
        .expect("seed chunk");
    let started = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(GatedEmbedder {
        started: started.clone(),
        release: release.clone(),
        dims,
    });
    let server = RagServer::new(store, embedder, config);
    let request_server = server.clone();
    let waiter = tokio::spawn(async move {
        request_server
            .apply_maintenance_plan(Parameters(ApplyMaintenancePlanParams {
                actions: vec![crate::mcp::tools::MaintenanceActionParams {
                    action: "reembed".into(),
                    reason: None,
                    target_id: Some("owned-doc".into()),
                    params: None,
                }],
                dry_run: Some(false),
                max_docs: None,
                agent: None,
            }))
            .await
    });
    let started_permit = tokio::time::timeout(std::time::Duration::from_secs(2), started.acquire())
        .await
        .expect("embedding started in time")
        .expect("started semaphore open");
    started_permit.forget();

    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    let lane = server.store.corpus_mutation_lane();
    assert!(
        lane.clone().try_write_owned().is_err(),
        "owned child must retain the lane after its request waiter is dropped"
    );

    release.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Ok(guard) = lane.clone().try_write_owned() {
                drop(guard);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("owned maintenance workflow completed");
    let chunks = server
        .store
        .list_chunks_for_document("owned-doc")
        .expect("reembedded chunks");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].embedding, vec![0.25; dims]);
}

#[tokio::test]
async fn retrieval_helpers_multi_get_expand_and_find_similar() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("retrieval-helpers.duckdb");
    let store = Store::open(&path).expect("open");
    let dims = 16usize;
    let config = test_config(path, Vec::new(), dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);
    let first = server
        .ingest_pipeline(IngestCommand {
            text: "alpha architecture and duckdb reliability ".repeat(80),
            title: Some("Alpha".into()),
            uri: Some("file://alpha.md".into()),
            metadata_json: None,
            wing: Some("rag".into()),
            room: Some("docs".into()),
            source_file: None,
            layer: "raw".into(),
            kind: "document".into(),
            immutable: false,
        })
        .await
        .unwrap();
    let second = server
        .ingest_pipeline(IngestCommand {
            text: "alpha architecture with database recovery ".repeat(80),
            title: Some("Beta".into()),
            uri: Some("file://beta.md".into()),
            metadata_json: None,
            wing: Some("rag".into()),
            room: Some("docs".into()),
            source_file: None,
            layer: "raw".into(),
            kind: "document".into(),
            immutable: false,
        })
        .await
        .unwrap();

    server
        .multi_get(Parameters(MultiGetParams {
            document_ids: vec![
                first.document_id.clone(),
                second.document_id.clone(),
                "missing".into(),
            ],
            include_chunks: Some(false),
            chunk_limit: None,
        }))
        .await
        .unwrap();
    server
        .expand_chunks(Parameters(ExpandChunksParams {
            document_id: first.document_id.clone(),
            chunk_index: 0,
            radius: Some(1),
        }))
        .await
        .unwrap();
    server
        .find_similar(Parameters(FindSimilarParams {
            document_id: first.document_id,
            top_k: Some(2),
            wing: Some("rag".into()),
            room: None,
        }))
        .await
        .unwrap();
}

#[tokio::test]
async fn llm_status_reports_config_and_unreachable_when_no_server() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("llm_status.duckdb");
    let store = Store::open(&path).expect("open");
    let dims = 32usize;
    // Closed local port so the probe fails fast without a live Ollama.
    let mut config = test_config(path.clone(), Vec::new(), dims);
    config.llm_enabled = false;
    config.llm_timeout_secs = 2;
    config.llm_base_url = "http://127.0.0.1:1/v1".to_string();
    config.llm_model = "test-model".to_string();
    config.embedding_model = "mock-embed".to_string();
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);

    assert!(server.llm.is_some(), "ChatClient should wire from config");

    let report = server.llm_status_report().await.expect("llm_status");
    assert!(!report.llm_enabled);
    assert_eq!(report.base_url, "http://127.0.0.1:1/v1");
    assert_eq!(report.model, "test-model");
    assert!(!report.reachable);
    assert!(report.error.is_some());
    assert_eq!(report.embed_provider, "mock");
    assert_eq!(report.embed_model, "mock-embed");
    assert_eq!(report.embed_dims, dims);
    assert!(report.embed_base_url.is_none());
}

#[test]
fn doctor_detects_manifest_dim_mismatch_and_roots() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("doctor.duckdb");
    let store = Store::open(&path).expect("open");
    store
        .set_embedding_manifest(&crate::models::EmbeddingManifest {
            id: "default".into(),
            provider: "mock".into(),
            model: "mock".into(),
            dims: 8,
            base_url: None,
            content_fingerprint: None,
            updated_at: Utc::now(),
        })
        .expect("manifest");

    let dims = 32usize;
    let config = test_config(path.clone(), vec![PathBuf::from("/allowed")], dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);

    let doctor = server.doctor_report().expect("doctor");
    assert!(!doctor.embed_ok);
    assert!(!doctor.ok);
    assert_eq!(doctor.manifest_dims, Some(8));
    assert!(doctor.ingest_roots_configured);
    assert!(!doctor.ready_for_search);

    let status = server.status_report().expect("status");
    assert!(status.ingest_roots_configured);
}

#[tokio::test]
async fn status_ready_for_search_after_ingest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ready.duckdb");
    let store = Store::open(&path).expect("open");
    let dims = 32usize;
    let config = test_config(path.clone(), Vec::new(), dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);

    server
        .ingest_pipeline(IngestCommand {
            text: "hello searchable world".into(),
            title: Some("t".into()),
            uri: Some("text://ready".into()),
            metadata_json: None,
            wing: None,
            room: None,
            source_file: None,
            layer: "raw".into(),
            kind: "document".into(),
            immutable: false,
        })
        .await
        .expect("ingest");

    let status = server.status_report().expect("status");
    assert_eq!(status.backend, "duckdb");
    assert!(status.chunk_count > 0);
    assert!(status.ready_for_search);
    assert_eq!(status.document_count, 1);
    assert!(status.node_count >= 1);

    let doctor = server.doctor_report().expect("doctor");
    assert!(doctor.ready_for_search);
    assert!(doctor.ok);
}

#[tokio::test]
async fn list_wings_rooms_taxonomy_and_status_wings() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("taxonomy.duckdb");
    let store = Store::open(&path).expect("open");
    let dims = 32usize;
    let config = test_config(path.clone(), Vec::new(), dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);

    for (text, uri, wing, room) in [
        ("alpha body", "raw://a", "research", "rag"),
        ("beta body", "raw://b", "research", "eval"),
        ("gamma body", "raw://c", "ops", "runbooks"),
    ] {
        server
            .ingest_pipeline(IngestCommand {
                text: text.into(),
                title: Some(uri.into()),
                uri: Some(uri.into()),
                metadata_json: None,
                wing: Some(wing.into()),
                room: Some(room.into()),
                source_file: None,
                layer: "raw".into(),
                kind: "document".into(),
                immutable: true,
            })
            .await
            .expect("ingest");
    }

    let wings = server.store.list_wings().expect("list_wings");
    assert_eq!(wings.len(), 2);
    assert_eq!(wings[0].wing, "ops");
    assert_eq!(wings[0].document_count, 1);
    assert_eq!(wings[1].wing, "research");
    assert_eq!(wings[1].document_count, 2);

    let rooms = server
        .store
        .list_rooms(Some("research"))
        .expect("list_rooms");
    assert_eq!(rooms.len(), 2);
    assert!(rooms
        .iter()
        .any(|r| r.room == "rag" && r.document_count == 1));
    assert!(rooms
        .iter()
        .any(|r| r.room == "eval" && r.document_count == 1));

    let tax = server.store.get_taxonomy().expect("taxonomy");
    assert_eq!(tax.total_documents, 3);
    assert_eq!(tax.unscoped_count, 0);
    assert_eq!(tax.wings.len(), 2);
    let research = tax.wings.iter().find(|w| w.wing == "research").unwrap();
    assert_eq!(research.document_count, 2);
    assert_eq!(research.rooms.len(), 2);

    let status = server.status_report().expect("status");
    assert_eq!(status.backend, "duckdb");
    assert_eq!(status.document_count, 3);
    assert!(status.chunk_count > 0);
    assert!(status.node_count >= 3);
    assert_eq!(status.wings.len(), 2);
    assert!(status.ready_for_search);
}

#[tokio::test]
async fn update_document_meta_skips_reembed_unless_content_changes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("meta.duckdb");
    let store = Store::open(&path).expect("open");
    let dims = 32usize;
    let config = test_config(path.clone(), Vec::new(), dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);

    // Wiki layer so body updates are allowed (raw is immutable).
    let ing = server
        .ingest_pipeline(IngestCommand {
            text: "original wiki body about retrieval".into(),
            title: Some("Wiki note".into()),
            uri: Some("wiki://meta-test".into()),
            metadata_json: Some(r#"{"k":1}"#.into()),
            wing: Some("research".into()),
            room: Some("rag".into()),
            source_file: None,
            layer: "wiki".into(),
            kind: "wiki".into(),
            immutable: false,
        })
        .await
        .expect("ingest");
    let chunks_before = server
        .store
        .list_chunks_for_document(&ing.document_id)
        .expect("chunks")
        .len();
    assert!(chunks_before > 0);
    let emb_before = server
        .store
        .list_chunks_for_document(&ing.document_id)
        .expect("chunks")[0]
        .embedding
        .clone();

    // Meta-only: must not re-embed.
    let meta = server
        .update_document_via_service(UpdateDocumentMetaParams {
            document_id: ing.document_id.clone(),
            wing: Some("ops".into()),
            room: Some("runbooks".into()),
            title: Some("Pinned wiki".into()),
            metadata_json: Some(r#"{"k":2,"tag":"x"}"#.into()),
            pinned: Some(true),
            boost: Some(2.5),
            status: Some("active".into()),
            ..Default::default()
        })
        .await
        .expect("meta update");
    assert!(!meta.content_changed);
    assert!(!meta.reembedded);
    assert_eq!(meta.chunk_count, chunks_before);
    assert_eq!(meta.wing.as_deref(), Some("ops"));
    assert_eq!(meta.room.as_deref(), Some("runbooks"));
    assert_eq!(meta.title, "Pinned wiki");
    assert!(meta.pinned);
    assert!((meta.boost - 2.5).abs() < f64::EPSILON);
    assert_eq!(meta.metadata_json, r#"{"k":2,"tag":"x"}"#);

    let emb_after_meta = server
        .store
        .list_chunks_for_document(&ing.document_id)
        .expect("chunks")[0]
        .embedding
        .clone();
    assert_eq!(
        emb_before, emb_after_meta,
        "meta-only update must not change embeddings"
    );

    // Content change: re-embed + rebuild.
    let body = server
        .update_document_via_service(UpdateDocumentMetaParams {
            document_id: ing.document_id.clone(),
            content: Some("revised wiki body about hybrid search and ranking".into()),
            ..Default::default()
        })
        .await
        .expect("content update");
    assert!(body.content_changed);
    assert!(body.reembedded);
    assert!(body.chunk_count > 0);

    // Raw layer refuses content rewrite.
    let raw = server
        .ingest_pipeline(IngestCommand {
            text: "raw immutable body".into(),
            title: Some("raw".into()),
            uri: Some("raw://meta-raw".into()),
            metadata_json: None,
            wing: Some("research".into()),
            room: Some("rag".into()),
            source_file: None,
            layer: "raw".into(),
            kind: "document".into(),
            immutable: true,
        })
        .await
        .expect("raw ingest");
    let refuse = server
        .update_document_via_service(UpdateDocumentMetaParams {
            document_id: raw.document_id,
            content: Some("mutated".into()),
            ..Default::default()
        })
        .await;
    assert!(refuse.is_err());
}

#[tokio::test]
async fn check_duplicate_and_delete_by_source_tools() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("dedupe.duckdb");
    let store = Store::open(&path).expect("open");
    let dims = 32usize;
    let config = test_config(path.clone(), Vec::new(), dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);

    let body = "duplicate candidate body";
    let src = "/vault/shared.md";
    server
        .ingest_pipeline(IngestCommand {
            text: body.into(),
            title: Some("One".into()),
            uri: Some("raw://one".into()),
            metadata_json: None,
            wing: Some("lab".into()),
            room: Some("a".into()),
            source_file: Some(src.into()),
            layer: "raw".into(),
            kind: "document".into(),
            immutable: true,
        })
        .await
        .expect("ingest one");
    server
        .ingest_pipeline(IngestCommand {
            text: "other".into(),
            title: Some("Two".into()),
            uri: Some("raw://two".into()),
            metadata_json: None,
            wing: Some("lab".into()),
            room: Some("b".into()),
            source_file: Some(src.into()),
            layer: "raw".into(),
            kind: "document".into(),
            immutable: true,
        })
        .await
        .expect("ingest two");

    let dup = server
        .store
        .check_duplicate(Some(body), None, None)
        .expect("check_duplicate");
    assert!(dup.is_duplicate);
    assert_eq!(dup.matches.len(), 1);
    assert_eq!(dup.matches[0].uri, "raw://one");

    let by_uri = server
        .store
        .check_duplicate(None, None, Some("raw://two"))
        .expect("uri probe");
    assert!(by_uri.is_duplicate);
    assert_eq!(by_uri.matches[0].match_reason, "uri");

    let deleted = server
        .store
        .delete_by_source(src)
        .expect("delete_by_source");
    assert_eq!(deleted, 2);
    assert_eq!(server.store.list_documents().expect("list").len(), 0);
    assert_eq!(server.store.delete_by_source(src).expect("again"), 0);
}

#[tokio::test]
async fn delete_by_source_preview_matches_archived_and_tombstone_apply_scope() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("delete-source-preview.duckdb");
    let store = Store::open(&path).expect("open");
    let dims = 8usize;
    let config = test_config(path, Vec::new(), dims);
    let server = RagServer::new(store, Arc::new(MockEmbedder::new(dims)), config);
    let source = "/vault/all-lifecycle-states.md";
    let now = Utc::now();
    for (id, status) in [
        ("active-source", "active"),
        ("archived-source", "archived"),
        ("tombstone-source", "tombstone"),
    ] {
        server
            .store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("raw://{id}"),
                title: id.into(),
                content: format!("body for {id}"),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                source_file: Some(source.into()),
                layer: "raw".into(),
                kind: "document".into(),
                status: status.into(),
                ..Default::default()
            })
            .unwrap();
    }

    let preview = server
        .delete_by_source(Parameters(DeleteBySourceParams {
            source_file: source.into(),
            dry_run: Some(true),
        }))
        .await
        .expect("preview all lifecycle states");
    let preview: serde_json::Value =
        serde_json::from_str(&preview.content[0].as_text().expect("text result").text).unwrap();
    assert_eq!(preview["match_count"], 3);
    let mut preview_ids = preview["document_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    preview_ids.sort();
    assert_eq!(
        preview_ids,
        vec!["active-source", "archived-source", "tombstone-source"]
    );

    let applied = server
        .delete_by_source(Parameters(DeleteBySourceParams {
            source_file: source.into(),
            dry_run: Some(false),
        }))
        .await
        .expect("apply exact preview scope");
    let applied: serde_json::Value =
        serde_json::from_str(&applied.content[0].as_text().expect("text result").text).unwrap();
    assert_eq!(applied["match_count"], 3);
    assert_eq!(applied["deleted"], 3);
    assert!(server.store.list_documents().unwrap().is_empty());
}

#[tokio::test]
async fn delete_by_source_finalization_failure_returns_committed_report() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("delete-source-finalize-failure.duckdb");
    let dims = 8usize;
    let config = test_config(path.clone(), Vec::new(), dims);
    let mut server = RagServer::new(
        Store::open(&path).expect("open"),
        Arc::new(MockEmbedder::new(dims)),
        config,
    );
    let source = "/vault/finalization-failure.md";
    let ingested = server
        .ingest_pipeline(IngestCommand {
            text: "durable deletion before derived-index failure".into(),
            title: Some("Finalization failure".into()),
            uri: Some("raw://finalization-failure".into()),
            metadata_json: None,
            wing: None,
            room: None,
            source_file: Some(source.into()),
            layer: "raw".into(),
            kind: "document".into(),
            immutable: false,
        })
        .await
        .expect("seed document");
    server.store.ensure_fts(&server.config.fts_stemmer).unwrap();
    server.config.fts_stemmer = crate::db::fts::test_fault::fail("facade");

    let result = server
        .delete_by_source(Parameters(DeleteBySourceParams {
            source_file: source.into(),
            dry_run: Some(false),
        }))
        .await
        .expect("committed mutation returns an aggregate report");
    let result: serde_json::Value =
        serde_json::from_str(&result.content[0].as_text().expect("text result").text).unwrap();
    assert_eq!(result["success"], false);
    assert_eq!(result["durable_mutation_committed"], true);
    assert_eq!(result["deleted"], 1);
    assert!(result["errors"][0]
        .as_str()
        .unwrap()
        .contains("FTS_FINALIZATION_FAILED"));
    assert!(server
        .store
        .get_document(&ingested.document_id)
        .unwrap()
        .is_none());
    let generation = {
        let conn = server.store.lock().unwrap();
        crate::db::fts_generation_state(&conn).unwrap()
    };
    assert!(generation.dirty, "next lexical read must retry FTS");
    assert!(server
        .store
        .list_recent_ops(10)
        .unwrap()
        .iter()
        .any(|entry| entry.op == "fts_finalization_failed"));
}

#[tokio::test]
async fn cleanup_source_duplicates_defaults_safe_requires_confirm_and_respects_lane() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("source-dedupe-tool.duckdb");
    let store = Store::open(&path).expect("open");
    let dims = 16usize;
    let config = test_config(path, Vec::new(), dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);
    let source = "/vault/tool.md";
    let body = "tool duplicate";
    let hash = content_hash(body);
    let now = Utc::now();
    for (id, uri) in [
        ("canonical", "file:///vault/tool.md"),
        ("legacy", "project://tool.md"),
    ] {
        server
            .store
            .upsert_document(&Document {
                id: id.into(),
                uri: uri.into(),
                title: id.into(),
                content: body.into(),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                source_file: Some(source.into()),
                content_hash: Some(hash.clone()),
                layer: "raw".into(),
                kind: "document".into(),
                status: "active".into(),
                ..Default::default()
            })
            .unwrap();
    }
    server.store.ensure_fts(&server.config.fts_stemmer).unwrap();
    let generation_before = {
        let conn = server.store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert!(!generation_before.dirty);

    let preview = server
        .cleanup_source_duplicates(Parameters(CleanupSourceDuplicatesParams::default()))
        .await
        .unwrap();
    let preview_json: serde_json::Value =
        serde_json::from_str(&preview.content[0].as_text().expect("text result").text).unwrap();
    assert_eq!(preview_json["dry_run"], true);
    assert_eq!(preview_json["applied"], false);
    assert_eq!(preview_json["candidate_count"], 1);
    assert!(server.store.get_document("legacy").unwrap().is_some());

    let missing_confirm = server
        .cleanup_source_duplicates(Parameters(CleanupSourceDuplicatesParams {
            dry_run: Some(false),
            confirm: Some(false),
            max_candidates: Some(10),
        }))
        .await
        .unwrap_err();
    assert!(missing_confirm.message.contains("confirm=true"));

    let search_guard = server.store.corpus_mutation_lane().read_owned().await;
    let concurrent_preview = server
        .cleanup_source_duplicates(Parameters(CleanupSourceDuplicatesParams::default()))
        .await
        .expect("read-only preview does not own the mutation lane");
    let concurrent_preview: serde_json::Value = serde_json::from_str(
        &concurrent_preview.content[0]
            .as_text()
            .expect("text result")
            .text,
    )
    .unwrap();
    assert_eq!(concurrent_preview["dry_run"], true);
    assert_eq!(concurrent_preview["candidate_count"], 1);

    let busy = server
        .cleanup_source_duplicates(Parameters(CleanupSourceDuplicatesParams {
            dry_run: Some(false),
            confirm: Some(true),
            max_candidates: Some(10),
        }))
        .await
        .unwrap_err();
    assert_eq!(busy.data.as_ref().unwrap()["code"], "STORE_BUSY");
    drop(search_guard);

    let applied = server
        .cleanup_source_duplicates(Parameters(CleanupSourceDuplicatesParams {
            dry_run: Some(false),
            confirm: Some(true),
            max_candidates: Some(10),
        }))
        .await
        .unwrap();
    let applied_json: serde_json::Value =
        serde_json::from_str(&applied.content[0].as_text().expect("text result").text).unwrap();
    assert_eq!(applied_json["applied"], true);
    assert_eq!(applied_json["deleted_documents"], 1);
    assert!(server.store.get_document("legacy").unwrap().is_none());
    let generation_after = {
        let conn = server.store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert!(!generation_after.dirty);
    assert_eq!(
        generation_after.chunks_generation,
        generation_before.chunks_generation + 1
    );
    assert_eq!(
        generation_after.index_generation,
        Some(generation_after.chunks_generation)
    );
    assert_eq!(
        generation_after.rebuild_count,
        generation_before.rebuild_count + 1
    );
}

#[tokio::test]
async fn ingest_raw_list_sources_get_source_immutable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("raw.duckdb");
    let store = Store::open(&path).expect("open");
    let dims = 32usize;
    let config = test_config(path.clone(), Vec::new(), dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);

    let r1 = server
        .ingest_pipeline(IngestCommand {
            text: "verbatim source alpha".into(),
            title: Some("Alpha".into()),
            uri: Some("raw://alpha".into()),
            metadata_json: None,
            wing: Some("projects".into()),
            room: Some("lab".into()),
            source_file: Some("/vault/alpha.md".into()),
            layer: "raw".into(),
            kind: "document".into(),
            immutable: true,
        })
        .await
        .expect("ingest_raw");
    assert!(r1.chunk_count >= 1);
    assert!(!r1.document_id.is_empty());

    // Same uri + content: idempotent no-op (stable id).
    let r2 = server
        .ingest_pipeline(IngestCommand {
            text: "verbatim source alpha".into(),
            title: Some("Alpha".into()),
            uri: Some("raw://alpha".into()),
            metadata_json: None,
            wing: Some("projects".into()),
            room: Some("lab".into()),
            source_file: Some("/vault/alpha.md".into()),
            layer: "raw".into(),
            kind: "document".into(),
            immutable: true,
        })
        .await
        .expect("re-ingest same");
    assert_eq!(r2.document_id, r1.document_id);
    assert_eq!(r2.chunk_count, r1.chunk_count);

    // Same uri + different content: conflict under immutable policy.
    let err = server
        .ingest_pipeline(IngestCommand {
            text: "changed content".into(),
            title: Some("Alpha".into()),
            uri: Some("raw://alpha".into()),
            metadata_json: None,
            wing: None,
            room: None,
            source_file: None,
            layer: "raw".into(),
            kind: "document".into(),
            immutable: true,
        })
        .await
        .expect_err("must refuse content change");
    assert!(
        matches!(err, AppError::Conflict(_)),
        "expected Conflict, got {err:?}"
    );

    let sources = server
        .store
        .list_documents_by_layer("raw")
        .expect("list raw");
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].id, r1.document_id);
    assert_eq!(sources[0].layer, "raw");
    assert_eq!(sources[0].wing.as_deref(), Some("projects"));
    assert_eq!(sources[0].room.as_deref(), Some("lab"));

    let got = server
        .store
        .get_document(&r1.document_id)
        .expect("get")
        .expect("present");
    assert_eq!(got.content, "verbatim source alpha");
    assert_eq!(got.layer, "raw");
    assert!(got.content_hash.is_some());
}

#[tokio::test]
async fn ingest_file_respects_allowlist() {
    let dir = tempfile::tempdir().expect("tempdir");
    let allowed = dir.path().join("allowed");
    std::fs::create_dir_all(&allowed).expect("mkdir");
    let inside = allowed.join("note.md");
    std::fs::write(&inside, "file body under root").expect("write");

    let outside = dir.path().join("secret.md");
    std::fs::write(&outside, "should not ingest").expect("write outside");

    let db_path = dir.path().join("allow.duckdb");
    let store = Store::open(&db_path).expect("open");
    let dims = 32usize;
    let config = test_config(db_path.clone(), vec![allowed.clone()], dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);

    // Outside root refused before read.
    let err = check_path_allowlist(Path::new(&outside), &server.config.ingest_roots)
        .expect_err("outside");
    assert!(
        err.to_string().contains("outside RAG_INGEST_ROOTS")
            || err.to_string().contains("RAG_INGEST_ROOTS"),
        "{err}"
    );

    // Empty roots refuse all.
    let err_empty = check_path_allowlist(Path::new(&inside), &[]).expect_err("empty");
    assert!(err_empty.to_string().contains("RAG_INGEST_ROOTS"));

    // Inside root is allowed and ingests.
    check_path_allowlist(Path::new(&inside), &server.config.ingest_roots).expect("inside allowed");
    let text = std::fs::read_to_string(&inside).expect("read");
    let result = server
        .ingest_pipeline(IngestCommand {
            text,
            title: Some("note.md".into()),
            uri: Some(format!("file://{}", inside.display())),
            metadata_json: None,
            wing: None,
            room: None,
            source_file: Some(inside.display().to_string()),
            layer: "raw".into(),
            kind: "document".into(),
            immutable: false,
        })
        .await
        .expect("ingest inside");
    assert!(result.chunk_count >= 1);
}

#[tokio::test]
async fn add_drawer_check_duplicate_delete_by_source() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("drawer.duckdb");
    let store = Store::open(&path).expect("open");
    let dims = 32usize;
    let config = test_config(path.clone(), Vec::new(), dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);

    let body = "verbatim drawer body for parity";
    let hash = content_hash(body);

    // Before ingest: not a duplicate.
    let miss = server
        .store
        .check_duplicate(Some(body), None, None)
        .expect("check miss");
    assert!(!miss.is_duplicate);
    assert_eq!(miss.content_hash.as_deref(), Some(hash.as_str()));

    // add_drawer path = ingest with required wing/room + optional source_file.
    let ing = server
        .ingest_pipeline(IngestCommand {
            text: body.into(),
            title: Some("Drawer A".into()),
            uri: Some("drawer://a".into()),
            metadata_json: None,
            wing: Some("projects".into()),
            room: Some("notes".into()),
            source_file: Some("/vault/drawer-a.md".into()),
            layer: "raw".into(),
            kind: "document".into(),
            immutable: false,
        })
        .await
        .expect("add_drawer ingest");
    assert!(ing.chunk_count >= 1);

    let doc = server
        .store
        .get_document(&ing.document_id)
        .expect("get")
        .expect("present");
    assert_eq!(doc.wing.as_deref(), Some("projects"));
    assert_eq!(doc.room.as_deref(), Some("notes"));
    assert_eq!(doc.source_file.as_deref(), Some("/vault/drawer-a.md"));
    assert_eq!(doc.content_hash.as_deref(), Some(hash.as_str()));

    // check_duplicate by content and by hash.
    let by_content = server
        .store
        .check_duplicate(Some(body), None, None)
        .expect("by content");
    assert!(by_content.is_duplicate);
    assert_eq!(by_content.matches.len(), 1);
    assert_eq!(by_content.matches[0].id, ing.document_id);

    let by_hash = server
        .store
        .check_duplicate(None, Some(&hash), None)
        .expect("by hash");
    assert!(by_hash.is_duplicate);

    server.store.ensure_fts(&server.config.fts_stemmer).unwrap();

    // dry_run delete_by_source leaves rows and the clean generation intact.
    let dry = server
        .delete_by_source(Parameters(DeleteBySourceParams {
            source_file: "/vault/drawer-a.md".into(),
            dry_run: Some(true),
        }))
        .await
        .expect("delete_by_source preview");
    let dry_json: serde_json::Value =
        serde_json::from_str(&dry.content[0].as_text().expect("text result").text).unwrap();
    assert_eq!(dry_json["match_count"], 1);
    assert_eq!(dry_json["deleted"], 0);
    assert!(server
        .store
        .get_document(&ing.document_id)
        .expect("get")
        .is_some());

    let search_guard = server.store.corpus_mutation_lane().read_owned().await;
    let busy = server
        .delete_by_source(Parameters(DeleteBySourceParams {
            source_file: "/vault/drawer-a.md".into(),
            dry_run: Some(false),
        }))
        .await
        .unwrap_err();
    assert_eq!(busy.data.as_ref().unwrap()["code"], "STORE_BUSY");
    drop(search_guard);

    let deleted = server
        .delete_by_source(Parameters(DeleteBySourceParams {
            source_file: "/vault/drawer-a.md".into(),
            dry_run: Some(false),
        }))
        .await
        .expect("delete_by_source apply");
    let deleted_json: serde_json::Value =
        serde_json::from_str(&deleted.content[0].as_text().expect("text result").text).unwrap();
    assert_eq!(deleted_json["deleted"], 1);
    assert!(server
        .store
        .get_document(&ing.document_id)
        .expect("get after")
        .is_none());
    assert!(
        !server
            .store
            .check_duplicate(Some(body), None, None)
            .expect("after delete")
            .is_duplicate
    );
    let generation_after_delete = {
        let conn = server.store.lock().unwrap();
        crate::db::fts::fts_generation_state(&conn).unwrap()
    };
    assert!(!generation_after_delete.dirty);
}

#[tokio::test]
async fn single_reembed_refuses_identity_migration_until_full_reembed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("reembed.duckdb");
    let store = Store::open(&path).expect("open");
    let dims = 16usize;
    let config = test_config(path.clone(), Vec::new(), dims);
    let calls = Arc::new(AtomicUsize::new(0));
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(CountingEmbedder {
        calls: calls.clone(),
        dims,
    });
    let server = RagServer::new(store, embedder, config);

    let ingest = server
        .ingest_pipeline(IngestCommand {
            text: "reembed me please".into(),
            title: Some("re".into()),
            uri: Some("text://reembed".into()),
            metadata_json: None,
            wing: None,
            room: None,
            source_file: None,
            layer: "raw".into(),
            kind: "document".into(),
            immutable: false,
        })
        .await
        .expect("ingest");

    let calls_after_ingest = calls.load(Ordering::Relaxed);
    let chunks_before = server
        .store
        .list_chunks_for_document(&ingest.document_id)
        .expect("chunks before mismatch");
    let generation_before = {
        let conn = server.store.lock().unwrap();
        crate::db::fts::chunks_generation(&conn).unwrap()
    };

    // Same provider/model/dims is still a mismatch when a persisted
    // fingerprint says the vectors came from another endpoint identity.
    let mut stale_manifest = crate::db::store::embedding_manifest_from_config(&server.config);
    stale_manifest.content_fingerprint = Some("stale-fingerprint".into());
    server
        .store
        .set_embedding_manifest(&stale_manifest)
        .expect("force mismatch");

    let err = server.require_vec_compatible().expect_err("must refuse");
    let msg = err.to_string();
    assert!(
        msg.contains("fingerprint") && msg.contains("uncapped reembed_all"),
        "unexpected: {msg}"
    );

    let error = server
        .reembed_document_pipeline(&ingest.document_id)
        .await
        .expect_err("single-document refresh must not migrate identity");
    assert!(error
        .to_string()
        .contains("reembed_document cannot migrate embedding identity"));
    assert_eq!(calls.load(Ordering::Relaxed), calls_after_ingest);
    assert_eq!(
        server
            .store
            .list_chunks_for_document(&ingest.document_id)
            .unwrap()
            .iter()
            .map(|chunk| &chunk.embedding)
            .collect::<Vec<_>>(),
        chunks_before
            .iter()
            .map(|chunk| &chunk.embedding)
            .collect::<Vec<_>>()
    );
    let generation_after_refusal = {
        let conn = server.store.lock().unwrap();
        crate::db::fts::chunks_generation(&conn).unwrap()
    };
    assert_eq!(generation_after_refusal, generation_before);
    assert_eq!(
        server
            .store
            .get_embedding_manifest()
            .unwrap()
            .unwrap()
            .content_fingerprint,
        stale_manifest.content_fingerprint
    );

    let migrated =
        crate::maintain::reembed_all(&server.store, &server.embedder, &server.config, usize::MAX)
            .await
            .expect("complete reembed migration");
    assert_eq!(migrated.documents_failed, 0);
    assert_eq!(migrated.skipped_cap, 0);
    assert_eq!(migrated.documents_succeeded, 1);

    server
        .require_vec_compatible()
        .expect("compatible after complete reembed");
    let chunks = server
        .store
        .list_chunks_for_document(&ingest.document_id)
        .expect("chunks");
    assert!(chunks.iter().all(|c| c.embedding.len() == dims));
}

#[test]
fn recovery_jsonl_requires_one_typed_manifest_before_documents() {
    let missing = decode_recovery_bundle("", "jsonl").unwrap_err();
    assert!(missing.to_string().contains("missing its manifest"));

    let document_first = serde_json::json!({
        "record_type": "document",
        "value": {"document": Document::default(), "chunks": []}
    })
    .to_string();
    let error = decode_recovery_bundle(&document_first, "jsonl").unwrap_err();
    assert!(error.to_string().contains("before the manifest"));

    let header = serde_json::json!({
        "record_type": "manifest",
        "format": "rag-recovery-bundle",
        "version": 1,
        "exported_at": Utc::now(),
    })
    .to_string();
    let decoded = decode_recovery_bundle(&header, "jsonl").unwrap();
    assert_eq!(decoded.version, LEGACY_RECOVERY_BUNDLE_VERSION);
    assert!(decoded.embedding_manifest.is_none());

    let duplicate = format!("{header}\n{header}\n");
    let error = decode_recovery_bundle(&duplicate, "jsonl").unwrap_err();
    assert!(error.to_string().contains("duplicate JSONL manifest"));

    let overflow = serde_json::json!({
        "record_type": "manifest",
        "format": "rag-recovery-bundle",
        "version": u64::from(u32::MAX) + 1,
        "exported_at": Utc::now(),
    })
    .to_string();
    let error = decode_recovery_bundle(&overflow, "jsonl").unwrap_err();
    assert!(error.to_string().contains("exceeds u32"));
}

#[tokio::test]
async fn recovery_tools_refuse_oversized_io_without_publishing_an_artifact() {
    let root = tempfile::tempdir().unwrap();
    let db_path = root.path().join("bounded.duckdb");
    let store = Store::open(&db_path).unwrap();
    let config = test_config(db_path, vec![root.path().to_path_buf()], 32);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(32));
    let server = RagServer::new(store, embedder, config);

    let oversized_input = root.path().join("oversized.json");
    let file = std::fs::File::create(&oversized_input).unwrap();
    file.set_len(crate::db::recovery::PORTABLE_RECOVERY_MAX_BYTES + 1)
        .unwrap();
    let import_error = server
        .import_bundle(Parameters(ImportBundleParams {
            path: oversized_input.display().to_string(),
            format: Some("json".into()),
            conflict_policy: None,
            dry_run: Some(true),
            reembed_legacy: None,
        }))
        .await
        .unwrap_err();
    assert!(import_error.message.contains("verified DuckDB backup"));

    {
        let conn = server.store.lock().unwrap();
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
            [i64::try_from(crate::db::recovery::PORTABLE_RECOVERY_MAX_DOCUMENTS + 1).unwrap()],
        )
        .unwrap();
    }
    let output = root.path().join("must-not-exist.json");
    let export_error = server
        .export_bundle(Parameters(ExportBundleParams {
            path: output.display().to_string(),
            format: Some("json".into()),
            dry_run: Some(false),
            overwrite: Some(false),
        }))
        .await
        .unwrap_err();
    assert!(export_error.message.contains("contains 10001 documents"));
    assert!(export_error.message.contains("verified DuckDB backup"));
    assert!(!output.exists());
}

#[test]
fn busy_error_is_structured_retryable_and_resource_safe() {
    let mcp = McpError::from(AppError::busy(
        "exclusive corpus mutation is active; retry search after it completes",
    ));
    assert_eq!(mcp.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
    assert_eq!(
        mcp.data,
        Some(serde_json::json!({
            "code": "STORE_BUSY",
            "retryable": true,
            "retry_after_ms": 1_000,
        }))
    );
    assert!(!mcp.message.contains("http"));
    assert!(!mcp.message.contains("/Users/"));
}

#[tokio::test]
async fn multi_query_search_fails_busy_before_embedding_any_rewrite() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("multi-query-busy.duckdb");
    let store = Store::open(&path).expect("open");
    let dims = 2;
    let calls = Arc::new(AtomicUsize::new(0));
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(CountingEmbedder {
        calls: calls.clone(),
        dims,
    });
    let mut config = test_config(path, Vec::new(), dims);
    config.default_search_mode = SearchMode::Hybrid;
    let server = RagServer::new(store, embedder, config);
    let sync_guard = server.store.corpus_mutation_lane().write_owned().await;

    let error = server
        .multi_query_search(Parameters(MultiQuerySearchParams {
            queries: vec!["original".into(), "rewrite".into()],
            top_k: Some(5),
            mode: Some("hybrid".into()),
            wing: Some("project".into()),
            room: None,
            layer: None,
            source_file: None,
            timeout_ms: Some(5_000),
        }))
        .await
        .unwrap_err();
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert_eq!(error.data.as_ref().unwrap()["code"], "STORE_BUSY");
    assert_eq!(error.data.as_ref().unwrap()["retryable"], true);
    drop(sync_guard);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vector_search_holds_corpus_guard_across_async_embedding() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("vector-search-guard.duckdb");
    let store = Store::open(&path).expect("open");
    let dims = 2;
    let started = Arc::new(tokio::sync::Semaphore::new(0));
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(GatedEmbedder {
        started: started.clone(),
        release: release.clone(),
        dims,
    });
    let server = RagServer::new(store, embedder, test_config(path, Vec::new(), dims));
    let request_server = server.clone();
    let search_task = tokio::spawn(async move {
        request_server
            .search(Parameters(SearchParams {
                query: "guarded query".into(),
                top_k: Some(5),
                document_id: None,
                mode: Some("vec".into()),
                min_score: None,
                wing: None,
                room: None,
                layer: None,
                source_file: None,
                include_archived: None,
                diversity: None,
                group_by: None,
                recency_half_life_days: None,
                max_context_tokens: None,
                max_chunks_per_document: None,
                context_expansion: None,
                neighbor_chunks: None,
                timeout_ms: Some(5_000),
                rrf_k: None,
            }))
            .await
    });
    let started_permit = tokio::time::timeout(std::time::Duration::from_secs(2), started.acquire())
        .await
        .expect("embedding started in time")
        .expect("started semaphore open");
    started_permit.forget();

    let mutation_error = match server
        .store
        .try_corpus_mutation_guard("embedding migration")
    {
        Ok(_) => panic!("vector search must retain the corpus read guard while embedding"),
        Err(error) => error,
    };
    assert!(matches!(mutation_error, AppError::Busy(_)));

    release.add_permits(1);
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), search_task)
        .await
        .expect("vector search finished in time")
        .expect("vector search task did not panic");
    assert!(result.is_ok());
}

#[test]
fn kg_store_roundtrip_via_server() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("kg_mcp.duckdb");
    let store = Store::open(&path).expect("open");
    let dims = 16usize;
    let config = test_config(path.clone(), Vec::new(), dims);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
    let server = RagServer::new(store, embedder, config);

    let f = server
        .store
        .kg_add(
            "Alice",
            "works_at",
            "Acme",
            parse_optional_timestamp(Some("2020-01-01")).unwrap(),
            None,
            Some("doc:1"),
            Some(0.95),
            None,
        )
        .expect("kg_add");
    assert_eq!(f.status, "active");

    let hits = server
        .store
        .kg_query(Some("Alice"), Some("works_at"), None, None)
        .expect("kg_query");
    assert_eq!(hits.len(), 1);

    let new_f = server
        .store
        .kg_supersede(
            "Alice",
            "works_at",
            "Acme",
            "BetaCo",
            parse_optional_timestamp(Some("2024-06-01")).unwrap(),
            None,
            None,
        )
        .expect("kg_supersede");
    assert_eq!(new_f.object, "BetaCo");

    let timeline = server.store.kg_timeline("Alice").expect("timeline");
    assert_eq!(timeline.len(), 2);

    let stats = server.store.kg_stats().expect("stats");
    assert_eq!(stats.total_facts, 2);
    assert_eq!(stats.active_facts, 1);
    assert_eq!(stats.superseded_facts, 1);
}
