//! Code, SQL, database and analysis operations over the shared server context.
use super::facade::RagServer;
use super::tools::*;
use crate::ingest::{IngestService, IngestUrlCommand};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{service::RequestContext, RoleServer};
use rmcp::{tool, tool_router, ErrorData as McpError};
#[tool_router(router = analysis_router, vis = "pub(super)")]
impl RagServer {
    #[tool(
        name = "catalog",
        description = "List immutable source-manifest pages, corpus-global committed index-run history (view=runs), or a project registry with captured corpus hashes, sync roots and explicit unverified freshness. Cursor pins revision and filters; project pages reject changed observations. No live filesystem reads. RAG_TOOLS=full."
    )]
    pub(super) async fn catalog(
        &self,
        Parameters(params): Parameters<crate::code_index::CatalogQuery>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.store.clone();
        let result =
            crate::util::run_blocking("catalog", move || store.code_catalog_query(&params)).await?;
        Self::json_result(&result)
    }
    #[tool(
        name = "freshness",
        description = "Compare indexed decoded-source checksums to actual allowlisted files using the ingestion decoder (default) or current stored documents. Records observed drift; never refreshes source/index content. RAG_TOOLS=full."
    )]
    pub(super) async fn freshness(
        &self,
        Parameters(params): Parameters<crate::code_index::drift::SourceQuery>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(
            &self
                .store
                .code_freshness(&params, &self.config.ingest_roots)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "live_read",
        description = "Read a bounded decoded current source range and compare it to an immutable index revision; reports original byte/encoding evidence. Filesystem mode requires RAG_INGEST_ROOTS; source=stored is explicit. RAG_TOOLS=full."
    )]
    pub(super) async fn live_read(
        &self,
        Parameters(params): Parameters<crate::code_index::drift::SourceQuery>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(
            &self
                .store
                .code_live_read(&params, &self.config.ingest_roots)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "drift_report",
        description = "Read a keyset-paginated ledger of observed index/source divergences. Unacknowledged records are the default. RAG_TOOLS=full."
    )]
    pub(super) async fn drift_report(
        &self,
        Parameters(params): Parameters<crate::code_index::drift::DriftQuery>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(
            &self
                .store
                .code_drift_report(&params)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "drift_ack",
        description = "Acknowledge known drift event IDs with a note, atomically. Does not alter sources or index facts. RAG_TOOLS=full."
    )]
    pub(super) async fn drift_ack(
        &self,
        Parameters(params): Parameters<crate::code_index::drift::DriftAck>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(&self.store.code_drift_ack(&params).map_err(Self::map_err)?)
    }
    #[tool(
        name = "index_status",
        description = "Versioned index capabilities and measured quality: project-scoped freshness, static bindings and bounded chunk/vector diagnostics. Never treats an empty or unmeasured corpus as healthy; does not repair or read live sources. RAG_TOOLS=full."
    )]
    pub(super) async fn index_status(
        &self,
        Parameters(params): Parameters<crate::code_index::status::StatusQuery>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.store.clone();
        let config = self.config.clone();
        let result = crate::util::run_blocking("index_status", move || {
            store.code_index_status(&params, &config)
        })
        .await?;
        Self::json_result(&result)
    }

    #[tool(
        name = "search_multi",
        description = "Find literal or regex queries over complete stored documents, with all/any matching, independent per-query result windows, line evidence and project-isolated filename families. Family roles carry fresh static call/reference/entrypoint evidence; runtime activity remains unverified. Captured source encodings include detection basis and byte hashes; legacy choices are heuristic. No chunk-boundary misses. RAG_TOOLS=full."
    )]
    pub(super) async fn search_multi(
        &self,
        Parameters(params): Parameters<crate::code_index::corpus::MultiSearch>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(&self.store.search_multi(&params).map_err(Self::map_err)?)
    }
    #[tool(
        name = "duplicates",
        description = "Find exact stored-text copies and similar numbered/name families. Rank primary candidates by observed entrypoint/call evidence; no deletion proof. RAG_TOOLS=full."
    )]
    pub(super) async fn duplicates(
        &self,
        Parameters(params): Parameters<crate::code_index::corpus::DuplicateQuery>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(&self.store.duplicates(&params).map_err(Self::map_err)?)
    }
    #[tool(
        name = "index_revisions",
        description = "List immutable code-index revisions with stable cursors and content fingerprints. Source bodies are content-addressed and stored once. RAG_TOOLS=full."
    )]
    pub(super) async fn index_revisions(
        &self,
        Parameters(params): Parameters<crate::code_index::history::HistoryQuery>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(
            &self
                .store
                .code_index_history(&params)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "diff",
        description = "Compare immutable index revisions: changed files, definitions/body hashes, static calls/value references/inheritance/entrypoints and frozen derived SQL schema metadata. Historical missing snapshots are explicit. No source bodies in the response. RAG_TOOLS=full."
    )]
    pub(super) async fn diff(
        &self,
        Parameters(params): Parameters<crate::code_index::history::DiffQuery>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(&self.store.code_index_diff(&params).map_err(Self::map_err)?)
    }
    #[tool(
        name = "calibrate",
        description = "Fit/report confidence thresholds from labelled training and independent validation IDs, or explicitly record unvalidated parameters. No runtime thresholds are applied. Reports sample size, confusion matrices and uncertainty. RAG_TOOLS=full."
    )]
    pub(super) async fn calibrate(
        &self,
        Parameters(params): Parameters<crate::calibration::CalibrationQuery>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(
            &self
                .store
                .calibrate_confidence(&params)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "calibration_reports",
        description = "Read immutable calibration reports with stable sequence pagination. Reports are not automatically applied to runtime query thresholds. RAG_TOOLS=full."
    )]
    pub(super) async fn calibration_reports(
        &self,
        Parameters(params): Parameters<crate::calibration::ReportsQuery>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(
            &self
                .store
                .calibration_reports(&params)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "gate",
        description = "Evaluate and persist an index-usage allow/ask/deny decision for a proposed tool call. Default warn allows with guidance; strict modes are explicit. Bounded actual source checks and real task/session index-use evidence. Never executes commands or installs hooks. RAG_TOOLS=full."
    )]
    pub(super) async fn gate(
        &self,
        Parameters(params): Parameters<crate::gate::GateQuery>,
    ) -> Result<CallToolResult, McpError> {
        let payload = crate::gate::parse_payload(&params.payload).map_err(Self::map_err)?;
        let session = self.analysis_scope(payload.session_id.as_deref())?;
        let mode = params
            .mode
            .or_else(|| std::env::var("RAG_GATE_MODE").ok())
            .unwrap_or("warn".into());
        let mode = crate::gate::Mode::parse(&mode).map_err(Self::map_err)?;
        Self::json_result(
            &self
                .store
                .analysis_gate(session, &payload, mode, &self.config.ingest_roots)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "task_set",
        description = "Set/clear a durable session-scoped code-analysis task label with TTL and optional generation CAS. Does not create a Codex chat or background goal. RAG_TOOLS=full."
    )]
    pub(super) async fn task_set(
        &self,
        Parameters(params): Parameters<crate::analysis_tasks::TaskSet>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.analysis_scope(params.session_id.as_deref())?;
        Self::json_result(
            &self
                .store
                .analysis_task_set(session, &params)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "note",
        description = "Append an unverified analysis note, separate from code/search facts. Defaults to this session's active task. Limits reject oversize input without silent truncation. RAG_TOOLS=full."
    )]
    pub(super) async fn note(
        &self,
        Parameters(params): Parameters<crate::analysis_tasks::Note>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.analysis_scope(params.session_id.as_deref())?;
        let agent = context
            .peer
            .peer_info()
            .map(|i| i.client_info.name.as_str())
            .unwrap_or("unknown");
        Self::json_result(
            &self
                .store
                .analysis_note(session, agent, &params)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "notes",
        description = "Read unverified task notes with literal substring filtering and stable sequence pagination. all_tasks explicitly includes all labels. Notes are not source evidence. RAG_TOOLS=full."
    )]
    pub(super) async fn notes(
        &self,
        Parameters(params): Parameters<crate::analysis_tasks::Notes>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.analysis_scope(params.session_id.as_deref())?;
        Self::json_result(
            &self
                .store
                .analysis_notes(session, &params)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "task_stats",
        description = "Durable measured index-call outcomes, returned-record counts and note totals by task. Unfinished/unmeasured calls remain explicit; no invented saved-call or correctness metric. RAG_TOOLS=full."
    )]
    pub(super) async fn task_stats(
        &self,
        Parameters(params): Parameters<crate::analysis_tasks::TaskStats>,
    ) -> Result<CallToolResult, McpError> {
        let session = self.analysis_scope(params.session_id.as_deref())?;
        Self::json_result(
            &self
                .store
                .analysis_task_stats(session, &params)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "policy_trace",
        description = "Trace explicit boolean and computed returns with branch/case conditions and static callers. Reports syntactic fallback and ambiguity; does not evaluate runtime authorization. Rust/Python/JS/TS/PHP. RAG_TOOLS=full."
    )]
    pub(super) async fn policy_trace(
        &self,
        Parameters(params): Parameters<crate::code_index::policy::PolicyQuery>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.store.code_snapshot().map_err(Self::map_err)?;
        Self::json_result(&snapshot.policy_trace(&params).map_err(Self::map_err)?)
    }
    #[tool(
        name = "class_usage",
        description = "Find CSS class definitions and static applications in CSS, HTML, JSX/TSX and DOM class APIs. Modes all/missing/unused describe indexed name matches, not runtime cascade or safe deletion. RAG_TOOLS=full."
    )]
    pub(super) async fn class_usage(
        &self,
        Parameters(params): Parameters<crate::code_index::styles::ClassQuery>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.store.code_snapshot().map_err(Self::map_err)?;
        Self::json_result(&snapshot.class_usage(&params).map_err(Self::map_err)?)
    }
    #[tool(
        name = "db_refresh",
        description = "Capture remote PostgreSQL/MySQL metadata from an owner-configured alias in a read-only transaction. Admin only. No caller SQL. Rows are collected only with owner enablement, column allowlists and include_samples."
    )]
    pub(super) async fn db_refresh(
        &self,
        Parameters(params): Parameters<crate::external_db::Refresh>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(
            &self
                .store
                .external_db_refresh(&params)
                .await
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "db_schema",
        description = "Read captured external database metadata. No network call. Exact schema/table filters; reports capture time and scope limitations. RAG_TOOLS=full."
    )]
    pub(super) async fn db_schema(
        &self,
        Parameters(params): Parameters<crate::external_db::Query>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(
            &self
                .store
                .external_db_query("db_schema", &params)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "db_table",
        description = "Read captured external database metadata. No network call. Exact schema/table filters; reports capture time and scope limitations. RAG_TOOLS=full."
    )]
    pub(super) async fn db_table(
        &self,
        Parameters(params): Parameters<crate::external_db::Query>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(
            &self
                .store
                .external_db_query("db_table", &params)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "db_relations",
        description = "Read frozen declared FK/view dependencies and separately labelled naming hypotheses. Guesses include key/type evidence, ambiguity diagnostics and uncalibrated confidence, never constraints. relation_kind filters separate arrays. No network call. RAG_TOOLS=full."
    )]
    pub(super) async fn db_relations(
        &self,
        Parameters(params): Parameters<crate::external_db::Query>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(
            &self
                .store
                .external_db_query("db_relations", &params)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "db_drift",
        description = "Compare captured external metadata: db_drift compares snapshots; db_cross matches static SQL and migration tables to live metadata in the configured project. No network access. Reports uncertainty. RAG_TOOLS=full."
    )]
    pub(super) async fn db_drift(
        &self,
        Parameters(params): Parameters<crate::external_db::Query>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(
            &self
                .store
                .external_db_query("db_drift", &params)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "db_cross",
        description = "Compare captured external metadata: db_drift compares snapshots; db_cross matches static SQL and migration tables to live metadata in the configured project. No network access. Reports uncertainty. RAG_TOOLS=full."
    )]
    pub(super) async fn db_cross(
        &self,
        Parameters(params): Parameters<crate::external_db::Query>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(
            &self
                .store
                .external_db_query("db_cross", &params)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "db_sample",
        description = "Read a bounded previously captured row sample. Sampling requires owner enablement, an explicit column allowlist, and include_samples on administrative db_refresh. Never reads live rows. RAG_TOOLS=full."
    )]
    pub(super) async fn db_sample(
        &self,
        Parameters(params): Parameters<crate::external_db::Query>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(
            &self
                .store
                .external_db_query("db_sample", &params)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "schema",
        description = "Inspect the SQL schema derived from ingested .sql files and SQL literals in migration directories. Reports evidence and incomplete coverage; never contacts a database. RAG_TOOLS=full."
    )]
    pub(super) async fn schema(
        &self,
        Parameters(params): Parameters<crate::code_index::sql::SqlQuery>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.store.code_snapshot().map_err(Self::map_err)?;
        Self::json_result(
            &snapshot
                .sql_query("schema", &params)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "table_usage",
        description = "Find static SQL table read/write sites in ingested SQL, Rust, Python, JS/TS, C/C++ literals (including C++ raw strings) and ordinary Java quoted strings, including joins and subqueries. Java text blocks and unsupported/dynamic SQL are reported as unverified. RAG_TOOLS=full."
    )]
    pub(super) async fn table_usage(
        &self,
        Parameters(params): Parameters<crate::code_index::sql::SqlQuery>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.store.code_snapshot().map_err(Self::map_err)?;
        Self::json_result(
            &snapshot
                .sql_query("table_usage", &params)
                .map_err(Self::map_err)?,
        )
    }
    #[tool(
        name = "write_check",
        description = "Check an insert/update/delete plan against ingested migrations or an explicitly selected captured database schema. Supports literal enum/domain checks, DEFAULT/identity rules and value counts. Returns valid/invalid/indeterminate with unverified conditions. Never connects remotely or executes writes. RAG_TOOLS=full."
    )]
    pub(super) async fn write_check(
        &self,
        Parameters(params): Parameters<crate::external_db::write_check::Request>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(&self.store.check_write_plan(params).map_err(Self::map_err)?)
    }
    #[tool(
        name = "entrypoints",
        description = "List conventional main, exported/public API, test and HTTP route entrypoint candidates with syntax evidence. RAG_TOOLS=full; reports incomplete coverage."
    )]
    pub(super) async fn entrypoints(
        &self,
        Parameters(params): Parameters<crate::code_index::CodeQuery>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.store.code_snapshot().map_err(Self::map_err)?;
        Self::json_result(&snapshot.query("entrypoints", &params))
    }
    #[tool(
        name = "routes",
        description = "List static Python/Express/Axum/PHP registrations separately from literal markup/fetch/jQuery/Axios endpoint references. References do not prove handlers or runtime requests; request_method and its explicit-option or client-name candidate basis are separate. Includes source revision/hash/offset; exact project/path, literal URL and separate method filters; pagination requires a view token after offset 0. RAG_TOOLS=full."
    )]
    pub(super) async fn routes(
        &self,
        Parameters(params): Parameters<crate::code_index::RouteQuery>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.store.clone();
        let result =
            crate::util::run_blocking("routes", move || store.code_routes(&params)).await?;
        Self::json_result(&result)
    }
    #[tool(
        name = "dead_code",
        description = "dead_code_view=routines (default) lists routine candidates; files checks known incoming links/entrypoints plus bounded literal path mentions in stored source documents, with refuted and not_checked evidence. Routines outside possible static reachability, preserving bound values, project-scoped unresolved-name roots and inherited method candidates. Candidates only; never proof that deletion is safe. RAG_TOOLS=full; reports incomplete coverage."
    )]
    pub(super) async fn dead_code(
        &self,
        Parameters(params): Parameters<crate::code_index::CodeQuery>,
    ) -> Result<CallToolResult, McpError> {
        match params.dead_code_view.as_deref().unwrap_or("routines") {
            "files" => {
                let store = self.store.clone();
                let result = crate::util::run_blocking("dead_code_files", move || {
                    store.code_dead_files(&params)
                })
                .await?;
                return Self::json_result(&result);
            }
            "routines" => {
                if params.dead_code_project.is_some()
                    || params.dead_code_max_candidates.is_some()
                    || params.dead_code_max_documents.is_some()
                    || params.dead_code_max_bytes.is_some()
                {
                    return Err(McpError::invalid_params(
                        "project/scan budgets require dead_code_view=files",
                        None,
                    ));
                }
            }
            _ => {
                return Err(McpError::invalid_params(
                    "dead_code_view must be routines or files",
                    None,
                ))
            }
        }
        let snapshot = self.store.code_snapshot().map_err(Self::map_err)?;
        Self::json_result(&snapshot.query("dead_code", &params))
    }
    #[tool(
        name = "slice",
        description = "Return a bounded structural call subgraph with path/line evidence and no function bodies. direction=back|forward|both; static graph only, not data-flow slicing. RAG_TOOLS=full; reports incomplete coverage."
    )]
    pub(super) async fn slice(
        &self,
        Parameters(params): Parameters<crate::code_index::CodeQuery>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.store.code_snapshot().map_err(Self::map_err)?;
        Self::json_result(&snapshot.query("slice", &params))
    }
    #[tool(
        name = "trace_path",
        description = "Find bounded simple symbol-to-symbol paths over captured calls, bound references or inheritance candidates. Exact project and endpoint paths disambiguate names; forward/back/both traversal, per-step source evidence and explicit work/depth/result limits. Fresh static graph only; absence and runtime reachability are never proven. RAG_TOOLS=full."
    )]
    pub(super) async fn trace_path(
        &self,
        Parameters(params): Parameters<crate::code_index::PathQuery>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.store.clone();
        let result = crate::util::run_blocking("trace_path", move || {
            let snapshot = store.code_snapshot_in_project(params.project.as_deref())?;
            Ok::<_, crate::error::AppError>(snapshot.trace_path(&params))
        })
        .await?;
        Self::json_result(&result)
    }
    #[tool(
        name = "detect_changes",
        description = "Read-only allowlisted Git change impact: working_tree=index to working files (plus untracked supported sources), staged=HEAD to index, all=HEAD to working files, commits=explicit before/after. Pins Git objects, builds two bounded native source graphs and maps added/removed/modified/configuration/binding-changed symbols with two-sided incoming evidence paths. Revalidates index and working source hashes; no checkout, filters, source/index mutation or stored-index freshness assumption. RAG_TOOLS=full; static supported-source scope and incomplete runtime impact are explicit."
    )]
    pub(super) async fn detect_changes(
        &self,
        Parameters(params): Parameters<crate::code_index::GitImpactQuery>,
    ) -> Result<CallToolResult, McpError> {
        let roots = self.config.ingest_roots.clone();
        let result = crate::util::run_blocking("detect_changes", move || {
            crate::code_index::git_diff_impact(&params, &roots)
        })
        .await?;
        Self::json_result(&result)
    }
    #[tool(
        name = "infrastructure",
        description = "Read a bounded captured configuration graph: Compose build/image declarations, Kubernetes namespace/selector/workload/container/image candidates, Docker stages/COPY/literal startup/Cargo targets and literal GitHub Actions build/script commands. Exact source revision/hash/byte evidence and bounded directed paths; latest view excludes stale stored inputs, explicit revision reads frozen sources. Cross-project image links require explicit linked_projects. No filesystem, cluster, Docker, shell or compiler execution; no runtime/coverage/absence proof. RAG_TOOLS=full."
    )]
    pub(super) async fn infrastructure(
        &self,
        Parameters(params): Parameters<crate::code_index::InfrastructureQuery>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.store.clone();
        let result =
            crate::util::run_blocking("infrastructure", move || store.code_infrastructure(&params))
                .await?;
        Self::json_result(&result)
    }
    #[tool(
        name = "rename",
        description = "Preview exact source-bound Rust symbol rename edits without writing. Requires a project, fresh captured graph and allowlisted UTF-8 sources. Unknown occurrences, aliases, trait families, collisions and unsupported forms block application. Returns a token pinning all captured input hashes; external/generated/dynamic references are not proven complete. RAG_TOOLS=full."
    )]
    pub(super) async fn rename(
        &self,
        Parameters(params): Parameters<crate::code_index::RenameQuery>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.store.clone();
        let roots = self.config.ingest_roots.clone();
        let result =
            crate::util::run_blocking("rename", move || store.code_rename_preview(&params, &roots))
                .await?;
        Self::json_result(&result)
    }
    #[tool(
        name = "apply_rename",
        description = "ADMIN filesystem mutation: recompute a reviewed Rust rename preview and require its exact plan_token before applying. Revalidates all captured source/provider/manifest hashes, preserves same-directory original backups and a recovery journal, rolls back ordinary failures without overwriting detected concurrent edits. Multi-file writes are not crash-atomic. Does not ingest, compile or refresh the index. RAG_TOOLS=full."
    )]
    pub(super) async fn apply_rename(
        &self,
        Parameters(params): Parameters<crate::code_index::RenameApply>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.store.clone();
        let roots = self.config.ingest_roots.clone();
        let result = crate::util::run_blocking("apply_rename", move || {
            store.code_rename_apply(&params, &roots)
        })
        .await?;
        Self::json_result(&result)
    }
    #[tool(
        name = "affected_tests",
        description = "Find statically related test entrypoints from unique changed symbol selectors in a fresh project-scoped cfg(test) graph overlay. Retains per-step source witnesses and command argv suggestions for captured Cargo targets/Python candidates. Explicit depth/result/work bounds, ambiguity and stale inputs. Coverage is incomplete: unlisted tests are never safe to skip; full-suite candidates are recommended. Read-only source/index; RAG_TOOLS=full."
    )]
    pub(super) async fn affected_tests(
        &self,
        Parameters(params): Parameters<crate::code_index::AffectedTestsQuery>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.store.clone();
        let result =
            crate::util::run_blocking("affected_tests", move || store.code_affected_tests(&params))
                .await?;
        Self::json_result(&result)
    }
    #[tool(
        name = "inheritance",
        description = "List indexed extends/implements relations plus separate Python/JS/TS same-name inherited method candidates; semantic overrides are not proven. Base targets retain unresolved/ambiguous evidence. Class method lookup uses conservative C3 order; dynamic dispatch is not guaranteed. RAG_TOOLS=full."
    )]
    pub(super) async fn inheritance(
        &self,
        Parameters(params): Parameters<crate::code_index::CodeQuery>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.store.code_snapshot().map_err(Self::map_err)?;
        Self::json_result(&snapshot.query("inheritance", &params))
    }
    #[tool(
        name = "code",
        description = "Read stored source by symbol, exact path/document or symbol/chunk IDs. Project disambiguation, inclusive line/exclusive byte windows, nested-span partitioning, shared character/segment budgets and view-checked continuation cursors. entry_ids accepts arrays or comma/whitespace text. One read transaction; no filesystem reads. Requires index_code and RAG_TOOLS=full."
    )]
    pub(super) async fn code(
        &self,
        Parameters(params): Parameters<crate::code_index::CodeReadQuery>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(&self.store.code_read(&params).map_err(Self::map_err)?)
    }
    #[tool(
        name = "repo_map",
        description = "Return a compact ranked definition map scoped by project and component-aware path_prefix, within budget_chars and optional estimated budget_tokens. Only fresh bindings affect ranking; reports omissions and scoped coverage. Requires index_code and RAG_TOOLS=full."
    )]
    pub(super) async fn repo_map(
        &self,
        Parameters(params): Parameters<crate::code_index::RepoMapQuery>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.store.clone();
        let result = crate::util::run_blocking("repo_map", move || {
            let snapshot = store.code_snapshot_in_project(params.project.as_deref())?;
            Ok::<_, crate::error::AppError>(snapshot.repo_map(&params))
        })
        .await?;
        Self::json_result(&result)
    }
    #[tool(
        name = "impact",
        description = "Traverse incoming dependencies of selected symbols up to depth. impact_relations=calls (default), references (also bound value/type uses), or all (also inheritance and override candidates). Optional impact_file or impact_document_id selects a file dependency graph, including ownerless bound occurrences (default references). Optional impact_table/impact_project/impact_table_mode selects direct SQL readers/writers plus their incoming code dependencies. Optional impact_key or impact_route selects literal key/URL occurrences plus separate multi-root code and file dependencies (default references); impact_route_kind filters registration/endpoint/all. Handler candidates never become source owners or seeds. Ambiguous table identities require disambiguation. Each result retains its relation/evidence; runtime impact is not proven. Requires index_code and RAG_TOOLS=full."
    )]
    pub(super) async fn impact(
        &self,
        Parameters(params): Parameters<crate::code_index::CodeQuery>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.store.clone();
        let result = crate::util::run_blocking("impact", move || {
            let snapshot = store.code_snapshot_in_project(params.impact_project.as_deref())?;
            Ok::<_, crate::error::AppError>(snapshot.query("impact", &params))
        })
        .await?;
        Self::json_result(&result)
    }
    #[tool(
        name = "index_code",
        description = "Build or refresh the persistent tree-sitter symbol and call index from ingested Rust/Python/JS/TS/Go/Java/C/C++/Bash/sh/Lua/Ruby/C#/Perl/PowerShell and basic PHP sources, plus supported SQL/markup inputs. Captured go.mod and Cargo.toml provide partial module/crate/target/path-dependency boundaries. Rust AST typing adds declared-return chains, await, fields, collection elements and standard pointer wrappers; full traits/features/cfg/macros remain unverified. No filesystem reads; unchanged parses are reused and limitations reported. RAG_TOOLS=full."
    )]
    pub(super) async fn index_code(
        &self,
        Parameters(options): Parameters<crate::code_index::IndexOptions>,
    ) -> Result<CallToolResult, McpError> {
        let store = self.store.clone();
        let report = crate::util::run_blocking("index_code", move || {
            store.index_code_with_options(&options)
        })
        .await?;
        Self::json_result(&report)
    }
    #[tool(
        name = "symbol",
        description = "Find code definitions (default), or set symbol_kind=translation_key/table/route/all for shared captured identities. project scopes facts; symbol or name selects exact ID/name/FQN, match_mode=contains enables literal substring. Shared nodes have no fake definition path; bounded source proofs and reference_target provide navigation. Requires index_code and RAG_TOOLS=full."
    )]
    pub(super) async fn symbol(
        &self,
        Parameters(params): Parameters<crate::code_index::SymbolQuery>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(&self.store.code_symbols(&params).map_err(Self::map_err)?)
    }
    #[tool(
        name = "references",
        description = "Find static symbol references, or select a SQL table, exact route URL or translation key. Table references retain readers/writers and ownerless SQL; route registrations and client URL mentions stay separate. project scopes identity and freshness; ambiguous schemas/projects require disambiguation. Symbol reference_mode=lexical retains name-only matches. Translation key candidates link literal PHP translator calls and template placeholders without proving runtime evaluation. No runtime binding proof. Requires index_code and RAG_TOOLS=full."
    )]
    pub(super) async fn references(
        &self,
        Parameters(params): Parameters<crate::code_index::ReferenceQuery>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(&self.store.code_references(&params).map_err(Self::map_err)?)
    }
    #[tool(
        name = "callers",
        description = "Find call sites targeting indexed symbols using conservative lexical/import/receiver-type binding. Returns resolution_basis; ambiguous and unresolved cases remain explicit. Requires index_code and RAG_TOOLS=full. Stale documents are omitted and reported."
    )]
    pub(super) async fn callers(
        &self,
        Parameters(params): Parameters<crate::code_index::CodeQuery>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.store.code_snapshot().map_err(Self::map_err)?;
        Self::json_result(&snapshot.query("callers", &params))
    }
    #[tool(
        name = "callees",
        description = "List calls inside a symbol with source evidence and conservative lexical/import/receiver-type resolution, including ambiguous and unresolved targets. Requires index_code and RAG_TOOLS=full. Stale documents are omitted and reported."
    )]
    pub(super) async fn callees(
        &self,
        Parameters(params): Parameters<crate::code_index::CodeQuery>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.store.code_snapshot().map_err(Self::map_err)?;
        Self::json_result(&snapshot.query("callees", &params))
    }
    #[tool(
        name = "ingest_url",
        description = "Fetch one web page over HTTP(S), extract main content (readability-lite), and ingest it. Disabled unless RAG_INGEST_URL_ALLOW allowlists the host; upserts by final URL."
    )]
    pub(super) async fn ingest_url(
        &self,
        Parameters(params): Parameters<IngestUrlParams>,
    ) -> Result<CallToolResult, McpError> {
        let fetcher = crate::web_ingest::ReqwestFetcher::new(self.config.ingest_url_timeout_secs);
        let result = IngestService::new(&self.store, &self.embedder, &self.config)
            .ingest_url(
                &fetcher,
                IngestUrlCommand {
                    url: params.url,
                    title: params.title,
                    uri: params.uri,
                    metadata_json: params.metadata_json,
                    wing: params.wing,
                    room: params.room,
                },
            )
            .await
            .map_err(Self::map_err)?;
        Self::json_result(&result)
    }
    #[tool(
        name = "reembed_status",
        description = "Embedding identity status: whether vector search is open, stored vs configured identity, and resumable migration progress (documents done/pending, batch size). Read-only."
    )]
    pub(super) async fn reembed_status(&self) -> Result<CallToolResult, McpError> {
        let body =
            crate::maintain::reembed_status(&self.store, &self.config).map_err(Self::map_err)?;
        Self::json_result(&body)
    }
}
