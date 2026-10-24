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
        description = "List an immutable revision source manifest with checksums and syntax coverage. RAG_TOOLS=full."
    )]
    pub(super) async fn catalog(
        &self,
        Parameters(params): Parameters<crate::code_index::drift::SourceQuery>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(&self.store.code_catalog(&params).map_err(Self::map_err)?)
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
        description = "Code-index revision, graph/SQL coverage and explicit static-analysis degradation. RAG_TOOLS=full."
    )]
    pub(super) async fn index_status(&self) -> Result<CallToolResult, McpError> {
        let snapshot = self.store.code_snapshot().map_err(Self::map_err)?;
        Self::json_result(
            &serde_json::json!({"index_rev":snapshot.index_revision,"symbols":snapshot.symbols.len(),"calls":snapshot.calls.len(),"unresolved_calls":snapshot.calls.iter().filter(|c|c.resolution!="resolved_static").count(),"sql_tables":snapshot.sql.tables.len(),"stale_documents":snapshot.stale_documents,"unindexed_documents":snapshot.unindexed_documents,"parse_errors":snapshot.parse_errors,"capability":"static_syntax_bindings","runtime_dispatch_verified":false,"data_flow_slicing":false}),
        )
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
        description = "List statically recognized Flask/FastAPI decorators, Express-style registrations and Axum route/get(handler) candidates. RAG_TOOLS=full; reports incomplete coverage."
    )]
    pub(super) async fn routes(
        &self,
        Parameters(params): Parameters<crate::code_index::CodeQuery>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.store.code_snapshot().map_err(Self::map_err)?;
        Self::json_result(&snapshot.query("routes", &params))
    }
    #[tool(
        name = "dead_code",
        description = "List routines not reachable from known roots, excluding value references. Candidates only; never proof that deletion is safe. RAG_TOOLS=full; reports incomplete coverage."
    )]
    pub(super) async fn dead_code(
        &self,
        Parameters(params): Parameters<crate::code_index::CodeQuery>,
    ) -> Result<CallToolResult, McpError> {
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
        name = "inheritance",
        description = "List indexed extends/implements relations with base targets and unresolved/ambiguous evidence. Class method lookup uses conservative C3 order; dynamic dispatch is not guaranteed. RAG_TOOLS=full."
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
        description = "Explicitly read the body of one indexed symbol. Ambiguous names return candidate definitions without bodies. Requires index_code and RAG_TOOLS=full."
    )]
    pub(super) async fn code(
        &self,
        Parameters(params): Parameters<crate::code_index::CodeQuery>,
    ) -> Result<CallToolResult, McpError> {
        Self::json_result(&self.store.code_body(&params).map_err(Self::map_err)?)
    }
    #[tool(
        name = "repo_map",
        description = "Return a map of indexed definitions within budget_chars; reports truncation and incomplete coverage. Requires index_code and RAG_TOOLS=full."
    )]
    pub(super) async fn repo_map(
        &self,
        Parameters(params): Parameters<crate::code_index::CodeQuery>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.store.code_snapshot().map_err(Self::map_err)?;
        Self::json_result(&snapshot.query("repo_map", &params))
    }
    #[tool(
        name = "impact",
        description = "Traverse callers of selected symbols up to depth. Links use conservative static bindings; unresolved and dynamic dependencies are not proven. Requires index_code and RAG_TOOLS=full."
    )]
    pub(super) async fn impact(
        &self,
        Parameters(params): Parameters<crate::code_index::CodeQuery>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.store.code_snapshot().map_err(Self::map_err)?;
        Self::json_result(&snapshot.query("impact", &params))
    }
    #[tool(
        name = "index_code",
        description = "Build or refresh the persistent tree-sitter symbol and call index from ingested Rust/Python/JS/TS/Go/Java/C/C++/Bash/sh and basic PHP sources, plus supported SQL/markup inputs. Go module facts use captured go.mod. No filesystem reads; unchanged parses are reused and limitations reported. RAG_TOOLS=full."
    )]
    pub(super) async fn index_code(&self) -> Result<CallToolResult, McpError> {
        Self::json_result(&self.store.index_code().map_err(Self::map_err)?)
    }
    #[tool(
        name = "symbol",
        description = "Find syntax definitions with source path, lines, signature, document revision and content hash. Requires index_code and RAG_TOOLS=full. Stale documents are omitted and reported."
    )]
    pub(super) async fn symbol(
        &self,
        Parameters(params): Parameters<crate::code_index::CodeQuery>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.store.code_snapshot().map_err(Self::map_err)?;
        Self::json_result(&snapshot.query("symbol", &params))
    }
    #[tool(
        name = "references",
        description = "Find references bound to indexed definitions through static imports, aliases, lexical scopes and known receiver types, including callbacks and type uses. Ambiguous candidates remain explicit. reference_mode=lexical retains name-only search. Requires index_code and RAG_TOOLS=full. Stale documents are omitted and reported."
    )]
    pub(super) async fn references(
        &self,
        Parameters(params): Parameters<crate::code_index::CodeQuery>,
    ) -> Result<CallToolResult, McpError> {
        let snapshot = self.store.code_snapshot().map_err(Self::map_err)?;
        Self::json_result(&snapshot.query("references", &params))
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
