//! Test fixtures of this crate.

use std::path::{Path, PathBuf};

use rag_store_duckdb::{Config, Result, Store};
use serde_json::Value;

/// [`Store::open`] with this crate's schema extensions registered, as the
/// composition root does before opening a store.
#[allow(dead_code)]
pub(crate) fn open_test_store(path: &Path) -> Result<Store> {
    crate::register_schema_extensions();
    Store::open(path)
}

/// Convenience delegator trait so unit tests in `rag-code-intel` can invoke
/// repository methods directly on `Store` (`store.index_code()`, etc.).
pub(crate) trait StoreCodeIntelTestExt {
    fn analysis_task_set(
        &self,
        session_id: &str,
        q: &crate::analysis_tasks::TaskSet,
    ) -> Result<Value>;
    fn analysis_note(
        &self,
        session_id: &str,
        agent: &str,
        q: &crate::analysis_tasks::Note,
    ) -> Result<Value>;
    fn analysis_notes(&self, session_id: &str, q: &crate::analysis_tasks::Notes) -> Result<Value>;
    fn analysis_call_start(&self, session_id: &str, tool: &str) -> Result<i64>;
    fn analysis_call_finish(&self, seq: i64, ok: bool, evidence: Option<i64>) -> Result<()>;
    fn analysis_task_stats(
        &self,
        session_id: &str,
        q: &crate::analysis_tasks::TaskStats,
    ) -> Result<Value>;
    fn calibrate_confidence(&self, q: &crate::calibration::CalibrationQuery) -> Result<Value>;
    fn calibration_reports(&self, q: &crate::calibration::ReportsQuery) -> Result<Value>;
    fn analysis_gate(
        &self,
        session_id: &str,
        p: &crate::gate::Payload,
        mode: crate::gate::Mode,
        roots: &[PathBuf],
    ) -> Result<Value>;
    fn insert_external_db_snapshot(
        &self,
        id: &str,
        connection_alias: &str,
        captured_ms: i64,
        payload: &str,
    ) -> Result<()>;
    fn external_db_query(&self, operation: &str, q: &crate::external_db::Query) -> Result<Value>;
    fn check_write_plan(&self, q: crate::external_db::write_check::Request) -> Result<Value>;
    fn index_code(&self) -> Result<crate::code_index::IndexReport>;
    fn index_code_with_options(
        &self,
        options: &crate::code_index::IndexOptions,
    ) -> Result<crate::code_index::IndexReport>;
    fn code_body(&self, q: &crate::code_index::CodeQuery) -> Result<Value>;
    fn code_snapshot(&self) -> Result<crate::code_index::CodeSnapshot>;
    fn code_snapshot_in_project(
        &self,
        project: Option<&str>,
    ) -> Result<crate::code_index::CodeSnapshot>;
    fn code_affected_tests(&self, query: &crate::code_index::AffectedTestsQuery) -> Result<Value>;
    fn code_catalog_query(&self, q: &crate::code_index::CatalogQuery) -> Result<Value>;
    fn code_read(&self, q: &crate::code_index::CodeReadQuery) -> Result<Value>;
    fn corpus_files(
        &self,
        project: Option<&str>,
        prefix: Option<&str>,
    ) -> Result<Vec<crate::code_index::corpus::File>>;
    fn search_multi(&self, q: &crate::code_index::corpus::MultiSearch) -> Result<Value>;
    fn duplicates(&self, q: &crate::code_index::corpus::DuplicateQuery) -> Result<Value>;
    fn code_data_slice(&self, q: &crate::code_index::DataSliceQuery) -> Result<Value>;
    fn code_dead_files(&self, q: &crate::code_index::CodeQuery) -> Result<Value>;
    fn code_freshness(
        &self,
        q: &crate::code_index::drift::SourceQuery,
        roots: &[PathBuf],
    ) -> Result<Value>;
    fn code_live_read(
        &self,
        q: &crate::code_index::drift::SourceQuery,
        roots: &[PathBuf],
    ) -> Result<Value>;
    fn code_drift_report(&self, q: &crate::code_index::drift::DriftQuery) -> Result<Value>;
    fn code_drift_ack(&self, q: &crate::code_index::drift::DriftAck) -> Result<Value>;
    fn code_index_history(&self, q: &crate::code_index::history::HistoryQuery) -> Result<Value>;
    fn code_index_diff(&self, q: &crate::code_index::history::DiffQuery) -> Result<Value>;
    fn code_infrastructure(&self, q: &crate::code_index::InfrastructureQuery) -> Result<Value>;
    fn code_references(&self, q: &crate::code_index::ReferenceQuery) -> Result<Value>;
    fn code_rename_preview(
        &self,
        q: &crate::code_index::RenameQuery,
        roots: &[PathBuf],
    ) -> Result<Value>;
    fn code_rename_apply(
        &self,
        q: &crate::code_index::RenameApply,
        roots: &[PathBuf],
    ) -> Result<Value>;
    fn code_routes(&self, q: &crate::code_index::RouteQuery) -> Result<Value>;
    fn code_index_status(
        &self,
        q: &crate::code_index::status::StatusQuery,
        config: &Config,
    ) -> Result<Value>;
    fn code_symbols(&self, q: &crate::code_index::SymbolQuery) -> Result<Value>;
}

impl StoreCodeIntelTestExt for Store {
    fn analysis_task_set(
        &self,
        session_id: &str,
        q: &crate::analysis_tasks::TaskSet,
    ) -> Result<Value> {
        crate::analysis_tasks::AnalysisTasks::new(self).analysis_task_set(session_id, q)
    }
    fn analysis_note(
        &self,
        session_id: &str,
        agent: &str,
        q: &crate::analysis_tasks::Note,
    ) -> Result<Value> {
        crate::analysis_tasks::AnalysisTasks::new(self).analysis_note(session_id, agent, q)
    }
    fn analysis_notes(&self, session_id: &str, q: &crate::analysis_tasks::Notes) -> Result<Value> {
        crate::analysis_tasks::AnalysisTasks::new(self).analysis_notes(session_id, q)
    }
    fn analysis_call_start(&self, session_id: &str, tool: &str) -> Result<i64> {
        crate::analysis_tasks::AnalysisTasks::new(self).analysis_call_start(session_id, tool)
    }
    fn analysis_call_finish(&self, seq: i64, ok: bool, evidence: Option<i64>) -> Result<()> {
        crate::analysis_tasks::AnalysisTasks::new(self).analysis_call_finish(seq, ok, evidence)
    }
    fn analysis_task_stats(
        &self,
        session_id: &str,
        q: &crate::analysis_tasks::TaskStats,
    ) -> Result<Value> {
        crate::analysis_tasks::AnalysisTasks::new(self).analysis_task_stats(session_id, q)
    }
    fn calibrate_confidence(&self, q: &crate::calibration::CalibrationQuery) -> Result<Value> {
        crate::calibration::Calibrations::new(self).calibrate_confidence(q)
    }
    fn calibration_reports(&self, q: &crate::calibration::ReportsQuery) -> Result<Value> {
        crate::calibration::Calibrations::new(self).calibration_reports(q)
    }
    fn analysis_gate(
        &self,
        session_id: &str,
        p: &crate::gate::Payload,
        mode: crate::gate::Mode,
        roots: &[PathBuf],
    ) -> Result<Value> {
        crate::gate::Gate::new(self).analysis_gate(session_id, p, mode, roots)
    }
    fn insert_external_db_snapshot(
        &self,
        id: &str,
        connection_alias: &str,
        captured_ms: i64,
        payload: &str,
    ) -> Result<()> {
        crate::external_db::ExternalDb::new(self).insert_external_db_snapshot(
            id,
            connection_alias,
            captured_ms,
            payload,
        )
    }
    fn external_db_query(&self, operation: &str, q: &crate::external_db::Query) -> Result<Value> {
        crate::external_db::ExternalDb::new(self).external_db_query(operation, q)
    }
    fn check_write_plan(&self, q: crate::external_db::write_check::Request) -> Result<Value> {
        crate::external_db::ExternalDb::new(self).check_write_plan(q)
    }
    fn index_code(&self) -> Result<crate::code_index::IndexReport> {
        crate::code_index::CodeIndex::new(self).index_code()
    }
    fn index_code_with_options(
        &self,
        options: &crate::code_index::IndexOptions,
    ) -> Result<crate::code_index::IndexReport> {
        crate::code_index::CodeIndex::new(self).index_code_with_options(options)
    }
    fn code_body(&self, q: &crate::code_index::CodeQuery) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_body(q)
    }
    fn code_snapshot(&self) -> Result<crate::code_index::CodeSnapshot> {
        crate::code_index::CodeIndex::new(self).code_snapshot()
    }
    fn code_snapshot_in_project(
        &self,
        project: Option<&str>,
    ) -> Result<crate::code_index::CodeSnapshot> {
        crate::code_index::CodeIndex::new(self).code_snapshot_in_project(project)
    }
    fn code_affected_tests(&self, query: &crate::code_index::AffectedTestsQuery) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_affected_tests(query)
    }
    fn code_catalog_query(&self, q: &crate::code_index::CatalogQuery) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_catalog_query(q)
    }
    fn code_read(&self, q: &crate::code_index::CodeReadQuery) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_read(q)
    }
    fn corpus_files(
        &self,
        project: Option<&str>,
        prefix: Option<&str>,
    ) -> Result<Vec<crate::code_index::corpus::File>> {
        crate::code_index::CodeIndex::new(self).corpus_files(project, prefix)
    }
    fn search_multi(&self, q: &crate::code_index::corpus::MultiSearch) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).search_multi(q)
    }
    fn duplicates(&self, q: &crate::code_index::corpus::DuplicateQuery) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).duplicates(q)
    }
    fn code_data_slice(&self, q: &crate::code_index::DataSliceQuery) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_data_slice(q)
    }
    fn code_dead_files(&self, q: &crate::code_index::CodeQuery) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_dead_files(q)
    }
    fn code_freshness(
        &self,
        q: &crate::code_index::drift::SourceQuery,
        roots: &[PathBuf],
    ) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_freshness(q, roots)
    }
    fn code_live_read(
        &self,
        q: &crate::code_index::drift::SourceQuery,
        roots: &[PathBuf],
    ) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_live_read(q, roots)
    }
    fn code_drift_report(&self, q: &crate::code_index::drift::DriftQuery) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_drift_report(q)
    }
    fn code_drift_ack(&self, q: &crate::code_index::drift::DriftAck) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_drift_ack(q)
    }
    fn code_index_history(&self, q: &crate::code_index::history::HistoryQuery) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_index_history(q)
    }
    fn code_index_diff(&self, q: &crate::code_index::history::DiffQuery) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_index_diff(q)
    }
    fn code_infrastructure(&self, q: &crate::code_index::InfrastructureQuery) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_infrastructure(q)
    }
    fn code_references(&self, q: &crate::code_index::ReferenceQuery) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_references(q)
    }
    fn code_rename_preview(
        &self,
        q: &crate::code_index::RenameQuery,
        roots: &[PathBuf],
    ) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_rename_preview(q, roots)
    }
    fn code_rename_apply(
        &self,
        q: &crate::code_index::RenameApply,
        roots: &[PathBuf],
    ) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_rename_apply(q, roots)
    }
    fn code_routes(&self, q: &crate::code_index::RouteQuery) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_routes(q)
    }
    fn code_index_status(
        &self,
        q: &crate::code_index::status::StatusQuery,
        config: &Config,
    ) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_index_status(q, config)
    }
    fn code_symbols(&self, q: &crate::code_index::SymbolQuery) -> Result<Value> {
        crate::code_index::CodeIndex::new(self).code_symbols(q)
    }
}
