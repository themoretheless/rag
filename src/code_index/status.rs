//! Versioned, read-only capability and measured-quality diagnostics.
use super::*;
use crate::config::Config;
use crate::db::store::{embedding_manifest_matches_config, get_embedding_manifest_locked};
use std::collections::BTreeMap;
use std::time::Instant;

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct StatusQuery {
    /// Exact wing; selection happens before decoding cached facts or source blobs.
    pub project: Option<String>,
    /// Evidence-window limit (default 50, clamped to 1..200).
    pub limit: Option<usize>,
    /// Deterministic chunk sample (default 128, clamped to 1..512).
    pub sample_limit: Option<usize>,
    /// Estimated token warning threshold; a diagnostic setting, not a calibrated rule.
    pub max_chunk_tokens: Option<usize>,
}
const SAMPLE_BYTES: usize = 4 * 1024 * 1024;
const VECTOR_BYTES: usize = 65536;
const TEXT_CHARS: usize = 16384;

fn languages() -> serde_json::Value {
    let specs = [
        ("rust", "rs", "tree-sitter"),
        ("python", "py", "tree-sitter"),
        ("javascript", "js", "tree-sitter"),
        ("typescript", "ts", "tree-sitter"),
        ("tsx", "tsx", "tree-sitter"),
        ("go", "go", "tree-sitter"),
        ("java", "java", "tree-sitter"),
        ("lua", "lua", "tree-sitter"),
        ("ruby", "rb", "tree-sitter"),
        ("csharp", "cs", "tree-sitter"),
        ("perl", "pl", "tree-sitter"),
        ("powershell", "ps1", "tree-sitter"),
        ("c", "c", "tree-sitter"),
        ("cpp", "cpp", "tree-sitter"),
        ("php", "php", "tree-sitter"),
        ("shell", "sh", "tree-sitter"),
        ("css", "css", "tree-sitter"),
        ("html", "html", "tree-sitter"),
        ("sql", "sql", "sqlparser"),
    ];
    serde_json::json!(specs
        .into_iter()
        .map(|(name, extension, engine)| {
            let probe = format!("probe.{extension}");
            serde_json::json!({"language":name,"engine":engine,"available":indexable(&probe),
            "binding_qualification":"partial_static","compiler_semantics_verified":false})
        })
        .collect::<Vec<_>>())
}

#[derive(Default, Serialize)]
struct ChunkQuality {
    total: usize,
    sampled: usize,
    tokenized: usize,
    oversized_estimated_tokens: usize,
    long_text_unmeasured: usize,
    large_vectors_unmeasured: usize,
    valid_vectors: usize,
    invalid_vectors: usize,
    zero_vectors: usize,
    dimension_mismatches: usize,
    unit_norm_vectors: usize,
    max_content_chars: usize,
    max_estimated_tokens: Option<usize>,
    norm_min: Option<f64>,
    norm_p50: Option<f64>,
    norm_max: Option<f64>,
    dimensions: BTreeMap<usize, usize>,
    bytes_inspected: usize,
    byte_budget_exhausted: bool,
    sample_truncated: bool,
    examples: Vec<serde_json::Value>,
    problem_rows: usize,
    examples_truncated: bool,
}
fn chunks(conn: &duckdb::Connection, q: &StatusQuery, dimensions: usize) -> Result<ChunkQuality> {
    let mut out = ChunkQuality { total: conn.query_row(
        "SELECT COUNT(*) FROM chunks c JOIN documents d ON c.document_id=d.id WHERE COALESCE(d.status,'active') NOT IN ('archived','tombstone','consolidated') AND (CAST(? AS VARCHAR) IS NULL OR d.wing=CAST(? AS VARCHAR))",
        params![q.project, q.project], |r| r.get(0))?, ..Default::default() };
    let sample_limit = q.sample_limit.unwrap_or(128).clamp(1, 512);
    let threshold = q.max_chunk_tokens.unwrap_or(4096).clamp(1, 100000);
    let mut stmt = conn.prepare("SELECT substring(c.id,1,128), length(c.id)>128, length(c.content), strlen(c.embedding_json), CASE WHEN length(c.content)<=? THEN c.content ELSE NULL END, CASE WHEN strlen(c.embedding_json)<=? THEN c.embedding_json ELSE NULL END FROM chunks c JOIN documents d ON c.document_id=d.id WHERE COALESCE(d.status,'active') NOT IN ('archived','tombstone','consolidated') AND (CAST(? AS VARCHAR) IS NULL OR d.wing=CAST(? AS VARCHAR)) ORDER BY c.id LIMIT ?")?;
    let mut rows = stmt.query(params![
        TEXT_CHARS as i64,
        VECTOR_BYTES as i64,
        q.project,
        q.project,
        sample_limit as i64
    ])?;
    let mut norms = Vec::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let id_truncated: bool = row.get(1)?;
        let chars: usize = row.get(2)?;
        let vector_bytes: usize = row.get(3)?;
        let text: Option<String> = row.get(4)?;
        let vector: Option<String> = row.get(5)?;
        let cost = text.as_ref().map_or(0, String::len) + vector.as_ref().map_or(0, String::len);
        if out.bytes_inspected + cost > SAMPLE_BYTES {
            out.byte_budget_exhausted = true;
            break;
        }
        out.bytes_inspected += cost;
        out.sampled += 1;
        out.max_content_chars = out.max_content_chars.max(chars);
        let mut issues = Vec::new();
        if let Some(text) = text {
            let count = crate::search_pack::estimate_tokens(&text);
            out.tokenized += 1;
            out.max_estimated_tokens = Some(out.max_estimated_tokens.unwrap_or(0).max(count));
            if count > threshold {
                out.oversized_estimated_tokens += 1;
                issues.push("CHUNK_TOKEN_ESTIMATE_OVER_THRESHOLD");
            }
        } else {
            out.long_text_unmeasured += 1;
        }
        if vector_bytes > VECTOR_BYTES {
            out.large_vectors_unmeasured += 1;
        }
        if let Some(vector) = vector {
            match serde_json::from_str::<Vec<f32>>(&vector) {
                Ok(values) if !values.is_empty() && values.iter().all(|v| v.is_finite()) => {
                    let norm = values
                        .iter()
                        .fold(0.0_f64, |sum, v| sum.hypot(f64::from(*v)));
                    *out.dimensions.entry(values.len()).or_default() += 1;
                    if norm.is_finite() {
                        out.valid_vectors += 1;
                        if norm == 0.0 {
                            out.zero_vectors += 1;
                            issues.push("ZERO_VECTOR");
                        }
                        if (norm - 1.0).abs() <= 0.01 {
                            out.unit_norm_vectors += 1;
                        }
                        norms.push(norm);
                    } else {
                        out.invalid_vectors += 1;
                        issues.push("NONFINITE_VECTOR_NORM");
                    }
                    if values.len() != dimensions {
                        out.dimension_mismatches += 1;
                        issues.push("VECTOR_DIMENSION_MISMATCH");
                    }
                }
                _ => {
                    out.invalid_vectors += 1;
                    issues.push("INVALID_VECTOR");
                }
            }
        }
        if !issues.is_empty() {
            out.problem_rows += 1;
        }
        if !issues.is_empty() && out.examples.len() < q.limit.unwrap_or(50).clamp(1, 200) {
            out.examples.push(serde_json::json!({"chunk_id":id,"chunk_id_truncated":id_truncated,"issues":issues}));
        }
    }
    norms.sort_by(f64::total_cmp);
    out.norm_min = norms.first().copied();
    out.norm_max = norms.last().copied();
    out.norm_p50 = norms.get(norms.len() / 2).copied();
    out.sample_truncated = out.sampled < out.total;
    out.examples_truncated = out.problem_rows > out.examples.len();
    Ok(out)
}

impl Store {
    /// Observe one connection-locked corpus view; never reindex or repair on read.
    pub fn code_index_status(&self, q: &StatusQuery, config: &Config) -> Result<serde_json::Value> {
        let started = Instant::now();
        let mut connection = self.lock()?;
        let transaction = connection.transaction()?;
        let conn = &*transaction;
        let snapshot_result = snapshot_locked(conn, q.project.as_deref());
        let snapshot_error = snapshot_result.as_ref().err().map(AppError::code);
        let snapshot = snapshot_result.ok();
        let revision: Option<i64> =
            conn.query_row("SELECT MAX(id) FROM code_index_revisions", [], |r| r.get(0))?;
        let schema_version: Option<i32> =
            conn.query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))?;
        let (eligible, source_documents, extensions, unsupported) = {
            let mut stmt = conn.prepare("SELECT d.id, COALESCE(d.source_file,d.uri), d.source_file IS NOT NULL FROM documents d WHERE COALESCE(d.status,'active') NOT IN ('archived','tombstone','consolidated') AND (CAST(? AS VARCHAR) IS NULL OR d.wing=CAST(? AS VARCHAR)) ORDER BY d.id")?;
            let mut rows = stmt.query(params![q.project, q.project])?;
            let mut eligible = 0usize;
            let mut source_documents = 0usize;
            let mut extensions = BTreeMap::<String, usize>::new();
            let mut unsupported = BTreeMap::<String, usize>::new();
            while let Some(row) = rows.next()? {
                let path: String = row.get(1)?;
                let source_file: bool = row.get(2)?;
                if source_file {
                    source_documents += 1;
                }
                let ext = std::path::Path::new(&path)
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("(none)")
                    .to_string();
                if indexable(&path) {
                    eligible += 1;
                    *extensions.entry(ext).or_default() += 1;
                } else if source_file {
                    *unsupported.entry(ext).or_default() += 1;
                }
            }
            (eligible, source_documents, extensions, unsupported)
        };
        let manifest_result = get_embedding_manifest_locked(conn, "default");
        let manifest_error = manifest_result.as_ref().err().map(AppError::code);
        let manifest = manifest_result.ok().flatten();
        let identity_match = manifest
            .as_ref()
            .map(|m| embedding_manifest_matches_config(m, config));
        let chunk_quality = chunks(conn, q, config.embedding_dims)?;
        let fts = crate::db::fts::fts_status(conn)?;
        let fts_generation = crate::db::fts::fts_generation_state(conn)?;
        let mut problems = Vec::new();
        if snapshot_error.is_some() {
            problems.push("CODE_SNAPSHOT_UNAVAILABLE");
        }
        if revision.is_none() {
            problems.push("NO_PUBLISHED_CODE_INDEX");
        }
        if eligible == 0 {
            problems.push("NO_INDEXABLE_DOCUMENTS_IN_SCOPE");
        }
        if let Some(s) = &snapshot {
            if !s.stale_documents.is_empty() {
                problems.push("STALE_DOCUMENTS");
            }
            if !s.unindexed_documents.is_empty() {
                problems.push("UNINDEXED_DOCUMENTS");
            }
            if s.file_resource_targets.iter().any(|t| !t.fresh()) {
                problems.push("RESOURCE_TARGETS_UNAVAILABLE");
            }
            if !s.parse_errors.is_empty() {
                problems.push("PARSE_ERRORS");
            }
            if eligible > 0
                && s.symbols.is_empty()
                && s.references.is_empty()
                && s.sql.tables.is_empty()
                && s.sql.usages.is_empty()
                && s.policies.is_empty()
                && s.styles.fact_count() == 0
                && s.entrypoints.is_empty()
            {
                problems.push("NO_FRESH_STRUCTURAL_FACTS");
            }
        }
        if chunk_quality.total > 0 && identity_match != Some(true) {
            problems.push("EMBEDDING_IDENTITY_UNVERIFIED");
        }
        if manifest_error.is_some() {
            problems.push("EMBEDDING_MANIFEST_UNAVAILABLE");
        }
        if chunk_quality.zero_vectors > 0 {
            problems.push("ZERO_VECTORS_IN_SAMPLE");
        }
        if chunk_quality.invalid_vectors > 0 {
            problems.push("INVALID_VECTORS_IN_SAMPLE");
        }
        if chunk_quality.dimension_mismatches > 0 {
            problems.push("VECTOR_DIMENSION_MISMATCH_IN_SAMPLE");
        }
        if chunk_quality.oversized_estimated_tokens > 0 {
            problems.push("CHUNK_TOKEN_ESTIMATE_OVER_THRESHOLD");
        }
        let state = if snapshot_error.is_some() {
            "unavailable"
        } else if revision.is_none() {
            "not_indexed"
        } else if eligible == 0 {
            "empty_scope"
        } else if !problems.is_empty() {
            "degraded"
        } else {
            "observed_checks_passed"
        };
        let limit = q.limit.unwrap_or(50).clamp(1, 200);
        let window = |items: &[String]| items.iter().take(limit).cloned().collect::<Vec<_>>();
        let empty = Vec::new();
        let stale = snapshot.as_ref().map_or(&empty, |s| &s.stale_documents);
        let unindexed = snapshot.as_ref().map_or(&empty, |s| &s.unindexed_documents);
        let parse_errors = snapshot.as_ref().map_or(&empty, |s| &s.parse_errors);
        let files_without_definitions = snapshot.as_ref().map(|s| {
            let unavailable: HashSet<_> = s
                .stale_documents
                .iter()
                .chain(s.unindexed_documents.iter())
                .map(String::as_str)
                .collect();
            let defined: HashSet<_> = s
                .symbols
                .iter()
                .map(|symbol| symbol.document_id.as_str())
                .collect();
            s.document_scopes
                .iter()
                .filter(|(id, scope)| {
                    language(&scope.path).is_some()
                        && !unavailable.contains(id.as_str())
                        && !defined.contains(id.as_str())
                })
                .count()
        });
        let binding_diagnostics = snapshot.as_ref().map(|s| {
            let mut codes = BTreeMap::<String, usize>::new();
            for diagnostic in &s.binding_diagnostics {
                let code = diagnostic
                    .get("code")
                    .and_then(|v| v.as_str())
                    .unwrap_or("UNCLASSIFIED");
                *codes.entry(code.into()).or_default() += 1;
            }
            codes
        });
        let mut resolution = BTreeMap::<String, usize>::new();
        if let Some(s) = &snapshot {
            for edge in &s.bound_references {
                *resolution.entry(edge.resolution.clone()).or_default() += 1;
            }
        }
        let signature_lengths: Vec<_> = snapshot
            .as_ref()
            .map(|s| {
                s.symbols
                    .iter()
                    .map(|s| s.signature.chars().count())
                    .collect()
            })
            .unwrap_or_default();
        let stage_timings = super::timings::status(conn, revision);
        let mut unmeasured = vec![
            "semantic_precision_recall",
            "runtime_dispatch",
            "full_docblock_coverage",
            "complete_parameter_types",
            "index_commit_timing",
            "index_failure_runs",
            "per_project_index_timings",
        ];
        if stage_timings["available"] != true {
            unmeasured.push("index_stage_timings");
        }
        if stage_timings["publication_run"].is_null() {
            unmeasured.push("revision_publication_run_timings");
        }
        let docblocks = super::documentation::quality(snapshot.as_ref());
        if docblocks["available"] != true {
            unmeasured.push("supported_docblock_coverage");
        }
        let quality_report = serde_json::json!({"healthy":if problems.is_empty(){None}else{Some(false)},"observed_checks_passed":problems.is_empty(),"exhaustive":false,"problems":problems,
                "snapshot_error_code":snapshot_error,"manifest_error_code":manifest_error,
                "binding_diagnostic_counts":binding_diagnostics.as_ref().map(|codes|codes.iter().take(limit).collect::<BTreeMap<_,_>>()),
                "binding_diagnostic_code_count":binding_diagnostics.as_ref().map(BTreeMap::len),
                "binding_diagnostic_codes_truncated":binding_diagnostics.as_ref().map(|codes|codes.len()>limit),
                "reference_resolution_counts":snapshot.as_ref().map(|_|resolution),"chunks":chunk_quality,
                "sample_policy":{"order":"chunk_id","limit":q.sample_limit.unwrap_or(128).clamp(1,512),"byte_budget":SAMPLE_BYTES,"max_text_chars":TEXT_CHARS,"max_vector_bytes":VECTOR_BYTES,"max_chunk_tokens":q.max_chunk_tokens.unwrap_or(4096).clamp(1,100000),"token_counts_estimated":true,"threshold_calibrated":false},
                "fresh_code_files_without_definitions":files_without_definitions,
                "signatures":{"available":snapshot.is_some(),"nonempty":snapshot.as_ref().map(|_|signature_lengths.iter().filter(|n|**n>0).count()),"empty":snapshot.as_ref().map(|_|signature_lengths.iter().filter(|n|**n==0).count()),"max_chars":signature_lengths.iter().max(),"truncation_verified":false,"parameter_type_coverage":null},
                "docblocks":docblocks,
                "embedding_identity":{"available":manifest.is_some(),"matches_config":identity_match,"scope":"corpus_global","provider":manifest.as_ref().map(|m|&m.provider),"model":manifest.as_ref().map(|m|&m.model),"dims":manifest.as_ref().map(|m|m.dims),"configured_dims":config.embedding_dims},
                "unmeasured_checks":unmeasured});
        let rust_trait_typing = serde_json::json!({"backend":"captured_ast","patterns":["cross_file_impl_owner","generic_impl_substitution","explicit_ufcs","generic_and_where_bounds","dyn_and_impl_trait_declarations","associated_type_projections","default_methods","inherent_precedence","explicit_ambiguity"],"bound_receiver_targets":"trait_declarations_only","runtime_dispatch_verified":false,"compiler_verified":false,"limitations":["full_coherence_and_specialization","higher_ranked_and_generic_associated_types","complete_supertrait_and_trait_argument_inference","macro_cfg_and_target_configuration"]});
        let capabilities = serde_json::json!({"languages":languages(),"calls_and_value_references":"conservative_static_candidates","inheritance":"partial_static",
                "rust_receiver_typing":{"backend":"captured_ast","patterns":["declared_returns","method_chains","async_await","named_fields","arrays_slices_vec","standard_box_arc_rc","local_factory_initializers","explicit_function_type_arguments"],"return_type_context":"declaration_document","runtime_dispatch_verified":false,"compiler_verified":false,"limitations":["complete_trait_solver_and_gats","custom_deref_and_index","macro_and_cfg_expansion","full_compiler_type_inference"]},
                "rust_trait_typing":rust_trait_typing,
                "sql_schema":"captured_migration_ddl_subset","slice":"bounded_structural_call_graph","cpg":{"available":false,"reason":"backend_not_implemented"},
                "runtime_dispatch_verified":false,"data_flow_slicing":false,"representative_precision_recall_verified":false,
                "graph_compute":{"scope":"structural_graph_operations","backend":"native_cpu","gpu_available":false,"reason":"no_graph_gpu_backend"}});
        let report = serde_json::json!({
            "contract_version":1,"project":q.project,"index_rev":revision,"state":state,
            "versions":{"index_format":INDEX_FORMAT_VERSION,"store_schema":schema_version,"package":env!("CARGO_PKG_VERSION")},
            "view":"current_captured_sources_and_fresh_subset_of_revision_inputs","observation_consistency":"single_read_transaction","filesystem_verified":false,
            "symbols":snapshot.as_ref().map(|s|s.symbols.len()),"calls":snapshot.as_ref().map(|s|s.calls.len()),
            "cargo_targets":snapshot.as_ref().map(|s|s.cargo_targets.iter().take(limit).collect::<Vec<_>>()),
            "cargo_targets_total":snapshot.as_ref().map(|s|s.cargo_targets.len()),
            "cargo_targets_truncated":snapshot.as_ref().map(|s|s.cargo_targets.len()>limit),
            "rust_build_contexts":snapshot.as_ref().map(|s|s.rust_build_contexts.iter().take(limit).collect::<Vec<_>>()),
            "rust_build_contexts_total":snapshot.as_ref().map(|s|s.rust_build_contexts.len()),
            "rust_build_contexts_truncated":snapshot.as_ref().map(|s|s.rust_build_contexts.len()>limit),
            "resource_targets":snapshot.as_ref().map(|s|serde_json::json!({"inventory_available":s.resource_target_inventory_available,"syntax_indexed":false,"diagnostics":resource_targets::diagnostics(&s.file_resource_targets,limit)})),
            "unresolved_calls":snapshot.as_ref().map(|s|s.calls.iter().filter(|c|c.resolution!="resolved_static").count()),
            "sql_tables":snapshot.as_ref().map(|s|s.sql.tables.len()),
            "sql_usages":snapshot.as_ref().map(|s|s.sql.usages.len()),"style_facts":snapshot.as_ref().map(|s|s.styles.fact_count()),"policy_decisions":snapshot.as_ref().map(|s|s.policies.len()),
            "stale_documents":window(stale),"unindexed_documents":window(unindexed),"parse_errors":window(parse_errors),
            "coverage":{"available":snapshot.is_some(),"source_documents":source_documents,"indexable_documents":eligible,
                "fresh_documents":snapshot.as_ref().map(|_|eligible.saturating_sub(stale.len()+unindexed.len())),
                "stale_count":snapshot.as_ref().map(|_|stale.len()),"unindexed_count":snapshot.as_ref().map(|_|unindexed.len()),"parse_error_count":snapshot.as_ref().map(|_|parse_errors.len()),
                "evidence_limit":limit,"evidence_truncated":snapshot.as_ref().map(|_|stale.len()>limit||unindexed.len()>limit||parse_errors.len()>limit),
                "extensions":extensions,"unsupported_source_extensions":unsupported.iter().take(limit).collect::<BTreeMap<_,_>>(),"unsupported_extension_categories":unsupported.len(),"unsupported_extensions_truncated":unsupported.len()>limit,"unsupported_is_not_parse_failure":true},
            "capability":"static_syntax_bindings","runtime_dispatch_verified":false,"data_flow_slicing":false,
            "capabilities":capabilities,
            "quality":quality_report,
            "retrieval":{"lexical":{"scope":"corpus_global","configured":fts.is_some(),"backend":fts.as_ref().map(|s|s.backend.as_str()),"dirty":fts_generation.dirty,"chunks_generation":fts_generation.chunks_generation,"index_generation":fts_generation.index_generation,"generation_gap":crate::db::fts::fts_generation_gap(&fts_generation),"rebuild_count":fts_generation.rebuild_count}},
            "stage_timings":stage_timings,"observation_ms":started.elapsed().as_secs_f64()*1000.0
        });
        transaction.commit()?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Chunk, Document};

    fn setup() -> (tempfile::TempDir, Store, Config) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("status.duckdb");
        let store = Store::open(&path).unwrap();
        let config = Config {
            db_path: path,
            embedding_dims: 2,
            llm_enabled: false,
            ..Config::for_tests()
        };
        (root, store, config)
    }
    fn put(store: &Store, id: &str, project: &str, path: &str, text: &str) {
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("doc://{id}"),
                source_file: Some(path.into()),
                wing: Some(project.into()),
                content: text.into(),
                ..Default::default()
            })
            .unwrap();
    }
    fn chunk(store: &Store, id: &str, document: &str, content: &str, embedding: Vec<f32>) {
        store
            .insert_chunks(&[Chunk {
                id: id.into(),
                document_id: document.into(),
                chunk_index: 0,
                content: content.into(),
                embedding,
                char_start: 0,
                char_end: content.chars().count() as i32,
                metadata_json: "{}".into(),
            }])
            .unwrap();
    }
    #[test]
    fn absent_empty_and_uninspected_are_never_healthy() {
        let (_root, store, config) = setup();
        let status = store
            .code_index_status(&StatusQuery::default(), &config)
            .unwrap();
        assert_eq!(status["contract_version"], 1);
        assert_eq!(status["state"], "not_indexed");
        assert_eq!(status["quality"]["healthy"], false);
        assert_eq!(
            status["quality"]["chunks"]["norm_p50"],
            serde_json::Value::Null
        );
        store.index_code().unwrap();
        let status = store
            .code_index_status(&StatusQuery::default(), &config)
            .unwrap();
        assert_eq!(status["state"], "empty_scope");
        assert_eq!(status["quality"]["healthy"], false);
        put(&store, "a", "a", "/p/a.py", "def work(): pass\n");
        store.index_code().unwrap();
        let status = store
            .code_index_status(&StatusQuery::default(), &config)
            .unwrap();
        assert_eq!(status["state"], "observed_checks_passed");
        assert!(status["quality"]["healthy"].is_null());
        assert_eq!(status["quality"]["chunks"]["sampled"], 0);
        assert_eq!(status["quality"]["exhaustive"], false);
        assert_eq!(status["capabilities"]["cpg"]["available"], false);
    }
    #[test]
    fn measured_vectors_and_tokens_are_scoped_bounded_and_do_not_repair() {
        let (_root, store, config) = setup();
        put(&store, "a", "a", "/p/a.py", "def work(): pass\n");
        put(&store, "b", "b", "/p/b.py", "def foreign(): pass\n");
        store.write_embedding_manifest_from_config(&config).unwrap();
        store.index_code().unwrap();
        chunk(
            &store,
            &"a".repeat(256),
            "a",
            &"word ".repeat(200),
            vec![0.0, 0.0],
        );
        chunk(&store, "b-size", "a", "short", vec![1.0]);
        chunk(&store, "c-invalid", "a", "short", vec![1.0, 0.0]);
        chunk(&store, "z-foreign", "b", "foreign", vec![0.0, 0.0]);
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE chunks SET embedding_json='broken JSON' WHERE id='c-invalid'",
                [],
            )
            .unwrap();
        let q = StatusQuery {
            project: Some("a".into()),
            limit: Some(1),
            max_chunk_tokens: Some(8),
            ..Default::default()
        };
        let status = store.code_index_status(&q, &config).unwrap();
        let quality = &status["quality"]["chunks"];
        assert_eq!(quality["total"], 3);
        assert_eq!(quality["sampled"], 3);
        assert_eq!(quality["zero_vectors"], 1);
        assert_eq!(quality["dimension_mismatches"], 1);
        assert_eq!(quality["invalid_vectors"], 1);
        assert_eq!(quality["oversized_estimated_tokens"], 1);
        assert_eq!(quality["examples"].as_array().unwrap().len(), 1);
        assert_eq!(quality["examples_truncated"], true);
        assert_eq!(
            quality["examples"][0]["chunk_id"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            128
        );
        assert_eq!(quality["examples"][0]["chunk_id_truncated"], true);
        assert_eq!(status["quality"]["healthy"], false);
        assert_eq!(
            status["quality"]["embedding_identity"]["matches_config"],
            true
        );
        let again = store.code_index_status(&q, &config).unwrap();
        assert_eq!(status["quality"], again["quality"]);
        assert_eq!(status["retrieval"], again["retrieval"]);
        assert!(!status.to_string().contains("word word"));
        assert!(!status.to_string().contains("broken JSON"));
        let limited = store
            .code_index_status(
                &StatusQuery {
                    sample_limit: Some(1),
                    ..q
                },
                &config,
            )
            .unwrap();
        assert_eq!(limited["quality"]["chunks"]["sampled"], 1);
        assert_eq!(limited["quality"]["chunks"]["sample_truncated"], true);
    }
    #[test]
    fn stale_windows_unsupported_sources_and_corrupt_cache_are_explicit() {
        let (_root, store, config) = setup();
        for id in ["a", "b"] {
            put(
                &store,
                id,
                "a",
                &format!("/p/{id}.py"),
                "def work(): pass\n",
            );
        }
        put(
            &store,
            "unsupported",
            "a",
            "/p/a.not_source",
            "unparsed fixture",
        );
        store.index_code().unwrap();
        for id in ["a", "b"] {
            put(
                &store,
                id,
                "a",
                &format!("/p/{id}.py"),
                "def changed(): pass\n",
            );
        }
        let q = StatusQuery {
            project: Some("a".into()),
            limit: Some(1),
            ..Default::default()
        };
        let status = store.code_index_status(&q, &config).unwrap();
        assert_eq!(status["coverage"]["stale_count"], 2);
        assert_eq!(status["stale_documents"], serde_json::json!(["a"]));
        assert_eq!(status["coverage"]["evidence_truncated"], true);
        assert_eq!(
            status["coverage"]["unsupported_source_extensions"]["not_source"],
            1
        );
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE code_index_documents SET payload='broken JSON' WHERE document_id='a'",
                [],
            )
            .unwrap();
        let broken = store.code_index_status(&q, &config).unwrap();
        assert_eq!(broken["state"], "unavailable");
        assert!(broken["symbols"].is_null());
        assert!(broken["coverage"]["fresh_documents"].is_null());
        assert!(broken["quality"]["reference_resolution_counts"].is_null());
        assert_eq!(broken["quality"]["healthy"], false);
        assert!(!broken.to_string().contains("broken JSON"));
        let foreign = store
            .code_index_status(
                &StatusQuery {
                    project: Some("other".into()),
                    ..Default::default()
                },
                &config,
            )
            .unwrap();
        assert_eq!(foreign["state"], "empty_scope");
        assert_eq!(foreign["coverage"]["indexable_documents"], 0);
    }
    #[test]
    fn oversized_sample_cells_are_reported_as_unmeasured() {
        let (_root, store, config) = setup();
        put(&store, "a", "a", "/p/a.py", "def work(): pass\n");
        store.index_code().unwrap();
        chunk(
            &store,
            "a",
            "a",
            &"界".repeat(TEXT_CHARS + 1),
            vec![1.0, 0.0],
        );
        store
            .lock()
            .unwrap()
            .execute(
                "UPDATE chunks SET embedding_json=repeat(' ', ?)",
                [VECTOR_BYTES as i64 + 1],
            )
            .unwrap();
        let status = store
            .code_index_status(&StatusQuery::default(), &config)
            .unwrap();
        let quality = &status["quality"]["chunks"];
        assert_eq!(quality["long_text_unmeasured"], 1);
        assert_eq!(quality["large_vectors_unmeasured"], 1);
        assert_eq!(quality["valid_vectors"], 0);
        assert_eq!(quality["tokenized"], 0);
        assert_eq!(quality["bytes_inspected"], 0);
        assert!(quality["norm_p50"].is_null());
    }
    #[test]
    fn sampling_budget_stops_inspection_and_nonunit_vectors_are_not_invalid() {
        let (_root, store, config) = setup();
        put(&store, "a", "a", "/p/a.py", "def work(): pass\n");
        store.index_code().unwrap();
        store.write_embedding_manifest_from_config(&config).unwrap();
        let content = "界".repeat(TEXT_CHARS);
        let rows: Vec<_> = (0..100)
            .map(|index| Chunk {
                id: format!("c{index:03}"),
                document_id: "a".into(),
                chunk_index: index,
                content: content.clone(),
                embedding: vec![3.0, 4.0],
                char_start: 0,
                char_end: TEXT_CHARS as i32,
                metadata_json: "{}".into(),
            })
            .collect();
        store.insert_chunks(&rows).unwrap();
        let status = store
            .code_index_status(
                &StatusQuery {
                    sample_limit: Some(512),
                    max_chunk_tokens: Some(100000),
                    ..Default::default()
                },
                &config,
            )
            .unwrap();
        let quality = &status["quality"]["chunks"];
        assert_eq!(quality["total"], 100);
        assert!(quality["sampled"].as_u64().unwrap() < 100);
        assert_eq!(quality["byte_budget_exhausted"], true);
        assert!(quality["bytes_inspected"].as_u64().unwrap() <= SAMPLE_BYTES as u64);
        assert_eq!(quality["norm_min"], 5.0);
        assert_eq!(quality["norm_max"], 5.0);
        assert_eq!(quality["unit_norm_vectors"], 0);
        assert_eq!(quality["invalid_vectors"], 0);
        assert!(status["quality"]["healthy"].is_null());
    }
    #[test]
    fn sql_read_evidence_without_definitions_is_not_an_empty_structural_index() {
        let (_root, store, config) = setup();
        put(&store, "sql", "a", "/p/read.sql", "SELECT id FROM users;");
        store.index_code().unwrap();
        let status = store
            .code_index_status(&StatusQuery::default(), &config)
            .unwrap();
        assert_eq!(status["symbols"], 0);
        assert_eq!(status["sql_tables"], 0);
        assert_eq!(status["sql_usages"], 1);
        assert_eq!(status["state"], "observed_checks_passed");
        assert!(status["quality"]["healthy"].is_null());
    }
    #[test]
    fn markup_endpoint_evidence_without_definitions_is_not_empty() {
        let (_root, store, config) = setup();
        put(
            &store,
            "markup",
            "a",
            "/p/view.html",
            "<form action='/save'></form>",
        );
        store.index_code().unwrap();
        let status = store
            .code_index_status(&StatusQuery::default(), &config)
            .unwrap();
        assert_eq!(status["symbols"], 0);
        assert_eq!(status["state"], "observed_checks_passed", "{status}");
        assert!(status["quality"]["healthy"].is_null());
    }
}
