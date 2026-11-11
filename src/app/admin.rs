//! `AdminUseCase`: read-only admin and catalog operations shared by MCP tools
//! and HTTP handlers (taxonomy, wings, rooms, temporal KG reads, wiki lint,
//! embedding identity and LLM status).
//!
//! Every store read runs on the blocking pool. Filters are normalized here
//! once: blank strings mean "no filter" on both transports.

use std::sync::Arc;

use crate::config::Config;
use crate::db::Store;
use crate::diagnostics::LlmStatusReport;
use crate::error::AppError;
use crate::llm::ChatClient;
use crate::maintain::ReembedStatus;
use crate::models::{KgFact, KgStats, RoomCount, Taxonomy, WingCount};
use crate::util::{non_blank, parse_optional_timestamp, run_blocking};
use crate::wiki::LintReport;

/// Typed temporal KG query (MCP `kg_query`, HTTP `GET /v1/kg`).
#[derive(Debug, Clone, Default)]
pub struct KgQueryCommand {
    pub subject: Option<String>,
    pub predicate: Option<String>,
    pub object: Option<String>,
    /// RFC 3339 instant; absent → active facts only.
    pub at_time: Option<String>,
    /// Return at most this many facts; the total stays the full match count.
    pub limit: Option<usize>,
}

/// One page of KG facts plus the full match count.
#[derive(Debug, Clone)]
pub struct KgQueryPage {
    pub facts: Vec<KgFact>,
    pub total: usize,
}

/// Admin read use cases over one store and config.
#[derive(Clone)]
pub struct AdminUseCase {
    store: Arc<Store>,
    config: Arc<Config>,
}

impl AdminUseCase {
    pub fn new(store: Arc<Store>, config: Arc<Config>) -> Self {
        Self { store, config }
    }

    async fn read<T, F>(&self, operation: &'static str, work: F) -> Result<T, AppError>
    where
        T: Send + 'static,
        F: FnOnce(&Store) -> Result<T, AppError> + Send + 'static,
    {
        let store = self.store.clone();
        run_blocking(operation, move || work(&store)).await
    }

    /// Full wing → room tree with counts.
    pub async fn taxonomy(&self) -> Result<Taxonomy, AppError> {
        self.read("taxonomy", |store| store.documents().get_taxonomy())
            .await
    }

    /// Distinct wings with document counts.
    pub async fn wings(&self) -> Result<Vec<WingCount>, AppError> {
        self.read("list wings", |store| store.documents().list_wings())
            .await
    }

    /// Distinct rooms with counts. `wing` is trimmed; blank lists every
    /// wing's rooms.
    pub async fn rooms(&self, wing: Option<String>) -> Result<Vec<RoomCount>, AppError> {
        let wing = non_blank(wing).map(|wing| wing.trim().to_owned());
        self.read("list rooms", move |store| {
            store.documents().list_rooms(wing.as_deref())
        })
        .await
    }

    /// Temporal KG facts by optional subject / predicate / object.
    pub async fn kg_query(&self, command: KgQueryCommand) -> Result<KgQueryPage, AppError> {
        let at_time = parse_optional_timestamp(command.at_time.as_deref())?;
        let subject = non_blank(command.subject);
        let predicate = non_blank(command.predicate);
        let object = non_blank(command.object);
        let limit = command.limit;
        self.read("kg query", move |store| {
            let (facts, total) = store.kg().kg_query_page(
                subject.as_deref(),
                predicate.as_deref(),
                object.as_deref(),
                at_time,
                limit,
            )?;
            Ok(KgQueryPage { facts, total })
        })
        .await
    }

    /// Chronological facts for one subject (trimmed; blank → `Invalid`).
    pub async fn kg_timeline(&self, subject: String) -> Result<Vec<KgFact>, AppError> {
        self.read("kg timeline", move |store| {
            store.kg().kg_timeline(subject.trim())
        })
        .await
    }

    /// Temporal KG aggregates.
    pub async fn kg_stats(&self) -> Result<KgStats, AppError> {
        self.read("kg stats", |store| store.kg().kg_stats()).await
    }

    /// Wiki and graph link health.
    pub async fn lint_wiki(&self) -> Result<LintReport, AppError> {
        self.read("lint wiki", crate::wiki::lint_wiki).await
    }

    /// Embedding identity status and resumable migration progress.
    pub async fn reembed_status(&self) -> Result<ReembedStatus, AppError> {
        let config = self.config.clone();
        self.read("reembed_status", move |store| {
            crate::maintain::reembed_status(store, &config)
        })
        .await
    }

    /// Chat LLM and embedding configuration with a short reachability probe.
    pub async fn llm_status(&self, llm: Option<&ChatClient>) -> LlmStatusReport {
        crate::diagnostics::llm_status(&self.config, llm).await
    }
}
