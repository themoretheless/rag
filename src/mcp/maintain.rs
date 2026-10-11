//! Corpus analysis and maintenance tools.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{tool, tool_router, ErrorData as McpError};

use super::facade::RagServer;
use super::tools::{
    AnalyzeCorpusParams, ApplyMaintenancePlanParams, CleanupSourceDuplicatesParams,
    DoctorRepairParams, MaintainCompressParams, MaintainOrganizeParams, MaintainRefreshParams,
    PlanMaintenanceParams,
};
use crate::diagnostics::DiagnosticsService;
use crate::error::AppError;
use crate::maintain::{
    self, ApplyPlanOptions, CompressOptions, MaintainRefreshFlags, MaintenancePlanItem,
};
use crate::models::VacuumStoreReport;
use crate::util::{run_blocking, run_detached};

#[tool_router(router = maintain_router, vis = "pub(super)")]
impl RagServer {
    #[tool(
        name = "cleanup_source_duplicates",
        description = "Safely preview or remove legacy active raw source duplicates. A candidate must share the exact non-empty source_file and content_hash with one canonical file://<source_file> survivor. dry_run defaults true; apply requires dry_run=false and confirm=true. The operation is atomic, capped, rewires graph/KG/manifest references, and skips groups with wiki or collection references."
    )]
    pub(super) async fn cleanup_source_duplicates(
        &self,
        Parameters(params): Parameters<CleanupSourceDuplicatesParams>,
    ) -> Result<CallToolResult, McpError> {
        let dry_run = params.dry_run.unwrap_or(true);
        let mut mutation_guard = if dry_run {
            None
        } else {
            Some(self.store.try_corpus_mutation_guard("duplicate cleanup")?)
        };
        let configured_cap = self.config.maint_max_docs.max(1);
        let max_candidates = params
            .max_candidates
            .map(|value| value as usize)
            .unwrap_or(configured_cap)
            .max(1)
            .min(configured_cap);
        let mut report = self.store.cleanup_source_duplicates(
            dry_run,
            params.confirm.unwrap_or(false),
            max_candidates,
        )?;
        if report.applied {
            // Keep the exclusive source-mutation lane until the derived lexical
            // index covers the committed deletions. Otherwise the first user
            // search after a successful cleanup pays the full BM25 rebuild.
            let guard = mutation_guard
                .take()
                .expect("applied duplicate cleanup owns the mutation lane");
            let outcome = self
                .refresh_fts_after_mutation("cleanup_source_duplicates", guard)
                .await;
            if let Some(error) = outcome.error {
                report.success = false;
                report.errors.push(error);
            }
            mutation_guard = outcome.guard;
        }
        let _mutation_guard = mutation_guard;
        Self::json_result(&report)
    }

    #[tool(
        name = "doctor_repair",
        description = "Preview or repair doctor findings. Defaults to dry_run=true. Reingests non-schema documents missing chunks, prunes orphan chunks/nodes/edges transactionally, checkpoints, and returns before/after doctor reports."
    )]
    pub(super) async fn doctor_repair(
        &self,
        Parameters(params): Parameters<DoctorRepairParams>,
    ) -> Result<CallToolResult, McpError> {
        let report = DiagnosticsService::new(&self.store, &self.config)
            .repair(
                &self.embedder,
                params.dry_run.unwrap_or(true),
                Some(clamp_maintenance_max_docs(
                    self.config.maint_max_docs,
                    params.max_docs,
                )),
            )
            .await?;
        Self::json_result(&report)
    }

    #[tool(
        name = "analyze_corpus",
        description = "Deterministic corpus health report (no LLM): counts by layer/kind/status/wing, exact dups, optional near-dups, orphan nodes/wiki, unresolved/aging stubs, stale wiki, embed mismatch, FTS, size, archive candidates. Returns AnalysisReport JSON; logs to ops_log."
    )]
    pub(super) async fn analyze_corpus(
        &self,
        Parameters(params): Parameters<AnalyzeCorpusParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut opts = maintain::AnalyzeOptions::from_config(&self.config);
        if let Some(d) = params.stub_age_days {
            opts.stub_age_days = d;
        }
        if let Some(v) = params.include_near_dups {
            opts.include_near_dups = v;
        }
        if let Some(t) = params.near_dup_threshold {
            opts.near_dup_threshold = t;
        }
        if let Some(d) = params.archive_min_age_days {
            opts.archive_min_age_days = d;
        }
        if let Some(v) = params.log_ops {
            opts.log_ops = v;
        }
        let report = maintain::analyze_corpus(&self.store, &self.config, &opts)?;
        Self::json_result(&report)
    }

    #[tool(
        name = "plan_maintenance",
        description = "Build an ordered maintenance plan from AnalysisReport. When RAG_LLM_ENABLED, calls local chat with analysis JSON + system prompt, parses JSON actions, validates whitelist. When LLM disabled (or force_heuristic), returns deterministic heuristic plan. Optional analysis JSON; else runs analyze_corpus. Logs to ops_log."
    )]
    pub(super) async fn plan_maintenance(
        &self,
        Parameters(params): Parameters<PlanMaintenanceParams>,
    ) -> Result<CallToolResult, McpError> {
        let report = if let Some(value) = params.analysis {
            serde_json::from_value::<maintain::AnalysisReport>(value).map_err(|e| {
                McpError::from(AppError::invalid(format!(
                    "plan_maintenance analysis must be an AnalysisReport JSON object: {e}"
                )))
            })?
        } else {
            let mut opts = maintain::AnalyzeOptions::from_config(&self.config);
            opts.log_ops = false;
            if let Some(v) = params.include_near_dups {
                opts.include_near_dups = v;
            }
            maintain::analyze_corpus(&self.store, &self.config, &opts)?
        };

        let mut plan_opts = maintain::PlanOptions::from_config(&self.config);
        if let Some(n) = params.max_actions {
            plan_opts.max_actions = (n as usize).max(1);
        }
        if let Some(v) = params.force_heuristic {
            plan_opts.force_heuristic = v;
        }
        if let Some(v) = params.log_ops {
            plan_opts.log_ops = v;
        }

        let llm = self.completer();
        let plan = maintain::plan_maintenance(&report, &self.config, llm, &plan_opts).await?;

        if plan_opts.log_ops {
            maintain::log_plan(&self.store, &plan)?;
        }

        Self::json_result(&plan)
    }

    #[tool(
        name = "apply_maintenance_plan",
        description = "Execute a whitelist-only maintenance plan (refile/pin/archive/set_tags/rebuild_*/reembed/compile_source/refresh_stale_wiki/merge_exact_dup/vacuum/…). dry_run defaults to true (preview + ops_log, no mutation). Set dry_run=false to apply. Document-scoped ops capped by max_docs / RAG_MAINT_MAX_DOCS. Returns applied/skipped/errors; ops_log each action. Never hard-deletes layer=raw without params.allow_raw_delete."
    )]
    pub(super) async fn apply_maintenance_plan(
        &self,
        Parameters(params): Parameters<ApplyMaintenancePlanParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut actions: Vec<MaintenancePlanItem> = Vec::with_capacity(params.actions.len());
        for a in params.actions {
            let kind = maintain::MaintenanceAction::parse(&a.action).ok_or_else(|| {
                McpError::from(AppError::forbidden(format!(
                    "maintenance action '{}' is not on the whitelist; allowed: {}",
                    a.action.trim(),
                    maintain::ALLOWED_ACTIONS.join(", ")
                )))
            })?;
            actions.push(MaintenancePlanItem {
                action: kind,
                reason: a.reason,
                target_id: a.target_id,
                params: a.params.unwrap_or_else(|| serde_json::json!({})),
            });
        }

        let opts = ApplyPlanOptions {
            dry_run: params.dry_run.unwrap_or(true),
            max_docs: Some(clamp_maintenance_max_docs(
                self.config.maint_max_docs,
                params.max_docs.map(|n| n as usize),
            )),
            agent: params.agent.filter(|s| !s.trim().is_empty()),
        };

        let report = if opts.dry_run {
            let llm = self.completer();
            maintain::apply_maintenance_plan(
                &self.store,
                &self.embedder,
                &self.config,
                llm,
                actions,
                &opts,
            )
            .await
        } else {
            let mutation_guard = self.store.try_corpus_mutation_guard("maintenance plan")?;
            let server = self.clone();
            run_detached("apply_maintenance_plan", async move {
                // This owned task intentionally survives a dropped MCP request:
                // mutations and their terminal FTS refresh keep one lane lease.
                let scoped_store = server.store.with_corpus_mutation_guard(mutation_guard)?;
                let llm = server.completer();
                maintain::apply_maintenance_plan(
                    &scoped_store,
                    &server.embedder,
                    &server.config,
                    llm,
                    actions,
                    &opts,
                )
                .await
            })
            .await
        };
        let report = report?;
        Self::json_result(&report)
    }

    #[tool(
        name = "maintain_organize",
        description = "Suggest refiles for documents missing wing (heuristic path/title/embedding; mode=llm|auto uses local chat when enabled). dry_run default true; dry_run=false applies whitelist refile via placement meta only. Optional rebuild_index. Cap: max_docs / RAG_MAINT_MAX_DOCS. Logs ops_log."
    )]
    pub(super) async fn maintain_organize(
        &self,
        Parameters(params): Parameters<MaintainOrganizeParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut opts = maintain::OrganizeOptions::from_config(&self.config);
        opts.dry_run = params.dry_run.unwrap_or(true);
        if let Some(ref mode) = params.mode {
            opts.mode = maintain::OrganizeMode::parse(mode)?;
        }
        if let Some(n) = params.max_docs {
            opts.max_docs =
                clamp_maintenance_max_docs(self.config.maint_max_docs, Some(n as usize));
        }
        if let Some(c) = params.min_confidence {
            opts.min_confidence = c;
        }
        if let Some(v) = params.rebuild_index {
            opts.rebuild_index = v;
        }
        opts.agent = params.agent.filter(|s| !s.trim().is_empty());

        let report = if opts.dry_run {
            let llm = self.completer();
            maintain::maintain_organize(&self.store, &self.config, llm, &opts).await
        } else {
            let server = self.clone();
            run_detached("maintain_organize", async move {
                // The maintenance service acquires the exclusive lease inside
                // this owned task. A disconnected MCP client therefore cannot
                // release it while an LLM-derived plan is still being applied.
                let llm = server.completer();
                maintain::maintain_organize(&server.store, &server.config, llm, &opts).await
            })
            .await
        };
        let report = report?;
        Self::json_result(&report)
    }

    #[tool(
        name = "maintain_refresh",
        description = "Actualize store after bulk edits: reindex_fts, rebuild_graph (dirty or all), rebuild_wiki_index, optional reembed_all. Whitelist actions only; logs to ops_log. dry_run previews without mutation. Defaults (no flags): fts+dirty graph+wiki index. Cap: RAG_MAINT_MAX_DOCS / max_docs."
    )]
    pub(super) async fn maintain_refresh(
        &self,
        Parameters(params): Parameters<MaintainRefreshParams>,
    ) -> Result<CallToolResult, McpError> {
        let max_docs = clamp_maintenance_max_docs(
            self.config.maint_max_docs,
            params.max_docs.map(|n| n as usize),
        );
        let flags = MaintainRefreshFlags::from_options(
            params.reindex_fts,
            params.rebuild_graph,
            params.graph_dirty_only,
            params.rebuild_wiki_index,
            params.reembed_all,
            params.dry_run,
            max_docs,
        );
        let report = if flags.dry_run {
            maintain::maintain_refresh(&self.store, &self.embedder, &self.config, flags).await
        } else {
            let mutation_guard = self
                .store
                .try_corpus_mutation_guard("maintenance refresh")?;
            let server = self.clone();
            run_detached("maintain_refresh", async move {
                // See apply_maintenance_plan: cancellation detaches this owned
                // workflow instead of releasing its corpus lease mid-refresh.
                let scoped_store = server.store.with_corpus_mutation_guard(mutation_guard)?;
                maintain::maintain_refresh(&scoped_store, &server.embedder, &server.config, flags)
                    .await
            })
            .await
        };
        let report = report?;
        Self::json_result(&report)
    }

    #[tool(
        name = "maintain_compress",
        description = "Compress store: L0=CHECKPOINT+FTS reindex; L1=exact content_hash merge (keep canonical); L2=near-dup list (merge only with confirm=true). dry_run default true. Never hard-deletes layer=raw without allow_raw_delete (tombstones). Cap: max_docs / RAG_MAINT_MAX_DOCS. Logs ops_log."
    )]
    pub(super) async fn maintain_compress(
        &self,
        Parameters(params): Parameters<MaintainCompressParams>,
    ) -> Result<CallToolResult, McpError> {
        let mut opts = CompressOptions::from_config(&self.config);
        if let Some(level) = params.level {
            opts.level = level.min(u32::from(u8::MAX)) as u8;
        }
        opts.dry_run = params.dry_run.unwrap_or(true);
        opts.confirm = params.confirm.unwrap_or(false);
        opts.allow_raw_delete = params.allow_raw_delete.unwrap_or(false);
        if let Some(t) = params.near_dup_threshold {
            opts.near_dup_threshold = t;
        }
        if let Some(n) = params.max_docs {
            opts.max_docs =
                clamp_maintenance_max_docs(self.config.maint_max_docs, Some(n as usize));
        }
        opts.validate()?;
        let mutation_guard = if opts.dry_run {
            None
        } else {
            Some(
                self.store
                    .try_corpus_mutation_guard("maintenance compression")?,
            )
        };
        let store = self.store.clone();
        let config = self.config.clone();
        let report = run_blocking("maintain_compress", move || {
            // Keep the exclusive lane in the blocking task itself so request
            // cancellation cannot release it while compression continues.
            let _mutation_guard = mutation_guard;
            maintain::maintain_compress(&store, &config, &opts)
        })
        .await?;
        Self::json_result(&report)
    }

    #[tool(
        name = "vacuum_store",
        description = "Safe L0 maintenance: DuckDB CHECKPOINT (flush WAL into main file). Returns db_path and file size bytes_before/bytes_after/bytes_delta when readable. Logs to ops_log. Does not delete rows."
    )]
    pub(super) async fn vacuum_store(&self) -> Result<CallToolResult, McpError> {
        let report: VacuumStoreReport = self.store.vacuum_store()?;
        Self::json_result(&report)
    }
}

pub(super) fn clamp_maintenance_max_docs(configured: usize, requested: Option<usize>) -> usize {
    let configured = configured.max(1);
    requested.unwrap_or(configured).max(1).min(configured)
}
